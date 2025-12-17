//! WebStore - Browser-based persistent storage using IndexedDB with in-memory caching
//!
//! This module provides a DataStore implementation for WASM targets that:
//! - Uses an in-memory VecDeque as the source of truth for sync operations
//! - Persists to IndexedDB via fire-and-forget async writes
//! - Hydrates from IndexedDB on initialization
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │                    WebStore                          │
//! │                                                      │
//! │  append() ──► VecDeque ──► spawn_local() ──► IndexedDB
//! │                  │              (fire & forget)      │
//! │  fetch()  ◄──────┘                                   │
//! │                                                      │
//! │  On init: IndexedDB ──► hydrate ──► VecDeque        │
//! └─────────────────────────────────────────────────────┘
//! ```

use crate::{DataResult, DataStore, Equivalent};
use serde_json::{json, Value};
use std::any::Any;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{Error, Result};
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{IdbDatabase, IdbRequest};

const DB_VERSION: u32 = 1;
const STORE_NAME: &str = "events";

/// Configuration for the web-based data store.
#[derive(Clone)]
pub struct WebConfig {
	/// Key used to identify writes to this store.
	/// Included in batch metadata and used as IndexedDB key prefix.
	pub write_key: String,
	/// Name of the IndexedDB database.
	/// Different stores should use different database names to avoid collisions.
	pub database_name: String,
	/// Maximum number of items to keep in memory.
	/// Oldest items are dropped when this limit is exceeded.
	pub max_items: usize,
	/// Maximum size in bytes for a single fetch operation.
	pub max_fetch_size: usize,
}

/// Internal representation of a stored event with its IndexedDB key
#[derive(Clone, Debug)]
struct StoredEvent {
	/// Auto-generated IndexedDB key
	idb_key: Option<u32>,
	/// The actual event data
	value: Value,
}

impl Equivalent for StoredEvent {
	fn equals(&self, other: &dyn Equivalent) -> bool {
		if let Some(other_event) = other.as_any().downcast_ref::<StoredEvent>() {
			// Compare by idb_key if both have one, otherwise by value
			match (&self.idb_key, &other_event.idb_key) {
				(Some(a), Some(b)) => a == b,
				_ => self.value == other_event.value,
			}
		} else {
			false
		}
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}

/// Indicates the persistence state of the WebStore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceState {
	/// IndexedDB is available and working. Events persist across page refreshes.
	Persisted,
	/// IndexedDB is unavailable (private browsing, blocked by browser policy, etc).
	/// Events are held in memory only and will be lost on page refresh.
	///
	/// When in this state, callers should consider:
	/// - Increasing flush frequency to minimize data loss window
	/// - Logging/alerting if this is unexpected
	MemoryOnly,
}

/// A browser-based data store using IndexedDB for persistence.
///
/// Events are stored in an in-memory queue for fast synchronous access,
/// with asynchronous fire-and-forget writes to IndexedDB for persistence
/// across page refreshes.
///
/// # Persistence Behavior
///
/// The store attempts to use IndexedDB for durable storage. If IndexedDB is
/// unavailable (private browsing, third-party context, storage blocked, etc),
/// the store falls back to memory-only mode and logs a warning to the console.
///
/// Use [`persistence_state()`](Self::persistence_state) to check the current
/// mode and adjust behavior accordingly (e.g., flush more aggressively when
/// in memory-only mode to minimize the data loss window).
///
/// # Third-Party Context Warning
///
/// Modern browsers increasingly restrict storage APIs in third-party contexts
/// (iframes on different domains). Safari ITP, Firefox strict mode, and Chrome's
/// upcoming changes all affect this. In third-party contexts, expect memory-only
/// mode to be common.
pub struct WebStore {
	config: WebConfig,
	/// In-memory queue - this is the source of truth for sync operations
	items: VecDeque<StoredEvent>,
	/// IndexedDB database handle (None if unavailable/blocked)
	db: Option<Rc<IdbDatabase>>,
	/// Counter for generating temporary keys before IndexedDB assigns real ones
	temp_key_counter: u32,
	/// Current persistence state
	persistence_state: PersistenceState,
}

impl WebStore {
	/// Creates a new WebStore with IndexedDB persistence.
	///
	/// This opens (or creates) the IndexedDB database and hydrates
	/// in-memory state from any previously persisted events.
	///
	/// If IndexedDB is unavailable (private browsing, third-party context,
	/// storage blocked, etc), the store falls back to memory-only mode
	/// and logs a warning to the console. Check [`persistence_state()`](Self::persistence_state)
	/// to detect this condition.
	///
	/// # Panics
	/// * If max_fetch_size is less than 100 bytes
	/// * If max_items is 0
	pub async fn new(config: WebConfig) -> Self {
		if config.max_fetch_size < 100 {
			panic!("max_fetch_size < 100 bytes? What are you even trying to fetch, empty arrays?");
		}
		if config.max_items == 0 {
			panic!("max_items = 0? So... you want a store that stores nothing? That's what /dev/null is for.");
		}

		let mut store = Self {
			config,
			items: VecDeque::new(),
			db: None,
			temp_key_counter: 0,
			persistence_state: PersistenceState::MemoryOnly,
		};

		// Attempt to open IndexedDB - fall back to memory-only if it fails
		match store.open_database().await {
			Ok(db) => {
				store.db = Some(Rc::new(db));
				store.persistence_state = PersistenceState::Persisted;

				// Hydrate from IndexedDB
				if let Err(e) = store.hydrate().await {
					web_sys::console::warn_1(
						&format!("Failed to hydrate from IndexedDB, starting fresh: {:?}", e)
							.into(),
					);
				}
			}
			Err(e) => {
				web_sys::console::warn_1(
					&format!(
						"IndexedDB unavailable ({}), falling back to memory-only storage. \
                         Events will not persist across page refreshes. \
                         Consider increasing flush frequency.",
						e
					)
					.into(),
				);
				// persistence_state already set to MemoryOnly
			}
		}

		store
	}

	/// Returns the current persistence state of the store.
	///
	/// Use this to detect when the store is operating in degraded mode
	/// and adjust behavior accordingly (e.g., flush more frequently).
	///
	/// # Example
	///
	/// ```ignore
	/// let store = WebStore::new(config).await?;
	///
	/// let flush_interval = match store.persistence_state() {
	///     PersistenceState::Persisted => Duration::from_secs(30),
	///     PersistenceState::MemoryOnly => Duration::from_secs(5), // Flush aggressively
	/// };
	/// ```
	pub fn persistence_state(&self) -> PersistenceState {
		self.persistence_state
	}

	/// Returns `true` if IndexedDB persistence is available.
	///
	/// Convenience method equivalent to checking if
	/// `persistence_state() == PersistenceState::Persisted`.
	pub fn is_persisted(&self) -> bool {
		self.persistence_state == PersistenceState::Persisted
	}

	/// Opens or creates the IndexedDB database
	async fn open_database(&self) -> Result<IdbDatabase> {
		let window = web_sys::window().ok_or_else(|| Error::other("No window object"))?;

		let idb_factory = window
			.indexed_db()
			.map_err(|e| Error::other(format!("IndexedDB error: {:?}", e)))?
			.ok_or_else(|| Error::other("IndexedDB not available"))?;

		// Create open request
		let open_request = idb_factory
			.open_with_f64(&self.config.database_name, DB_VERSION as f64)
			.map_err(|e| Error::other(format!("Failed to open DB: {:?}", e)))?;

		// Set up upgrade handler for first-time creation
		let on_upgrade = Closure::once(move |event: web_sys::IdbVersionChangeEvent| {
			let target = event.target().unwrap();
			let request: IdbRequest = target.unchecked_into();
			let db: IdbDatabase = request.result().unwrap().unchecked_into();

			// Create object store if it doesn't exist
			if !db.object_store_names().contains(STORE_NAME) {
				let params = web_sys::IdbObjectStoreParameters::new();
				params.set_auto_increment(true);
				params.set_key_path(&JsValue::from_str("_idb_key"));

				db.create_object_store_with_optional_parameters(STORE_NAME, &params)
					.expect("Failed to create object store");
			}
		});
		open_request.set_onupgradeneeded(Some(on_upgrade.as_ref().unchecked_ref()));
		on_upgrade.forget(); // Prevent closure from being dropped

		// Wait for success/error
		let db = Self::await_request::<IdbDatabase>(&open_request).await?;

		Ok(db)
	}

	/// Loads all existing events from IndexedDB into memory
	async fn hydrate(&mut self) -> Result<()> {
		let db = match &self.db {
			Some(db) => db.clone(),
			None => return Ok(()), // No db, nothing to hydrate
		};

		let transaction = db
			.transaction_with_str_and_mode(STORE_NAME, web_sys::IdbTransactionMode::Readonly)
			.map_err(|e| Error::other(format!("Transaction error: {:?}", e)))?;

		let store = transaction
			.object_store(STORE_NAME)
			.map_err(|e| Error::other(format!("Object store error: {:?}", e)))?;

		let request = store
			.get_all()
			.map_err(|e| Error::other(format!("GetAll error: {:?}", e)))?;

		let result = Self::await_request::<JsValue>(&request).await?;

		// Parse results into our items queue
		if let Ok(array) = result.dyn_into::<js_sys::Array>() {
			for item in array.iter() {
				if let Ok(obj) = js_sys::JSON::stringify(&item) {
					if let Ok(s) = obj.as_string().ok_or(()) {
						if let Ok(mut value) = serde_json::from_str::<Value>(&s) {
							// Extract the idb_key and remove it from the value
							let idb_key = value
								.get("_idb_key")
								.and_then(|k| k.as_u64())
								.map(|k| k as u32);

							if let Some(obj) = value.as_object_mut() {
								obj.remove("_idb_key");
							}

							self.items.push_back(StoredEvent { idb_key, value });
						}
					}
				}
			}
		}

		// Update temp_key_counter to be higher than any existing key
		if let Some(max_key) = self.items.iter().filter_map(|e| e.idb_key).max() {
			self.temp_key_counter = max_key + 1;
		}

		Ok(())
	}

	/// Fire-and-forget write to IndexedDB
	fn persist_event(&self, event: StoredEvent) {
		let Some(db) = &self.db else { return };
		let db = db.clone();
		let write_key = self.config.write_key.clone();

		spawn_local(async move {
			if let Err(e) = Self::write_to_idb(&db, &write_key, &event).await {
				// Log but don't fail - we still have it in memory
				web_sys::console::warn_1(&format!("IndexedDB write failed: {:?}", e).into());
			}
		});
	}

	/// Actual IndexedDB write operation
	async fn write_to_idb(db: &IdbDatabase, _write_key: &str, event: &StoredEvent) -> Result<()> {
		let transaction = db
			.transaction_with_str_and_mode(STORE_NAME, web_sys::IdbTransactionMode::Readwrite)
			.map_err(|e| Error::other(format!("Transaction error: {:?}", e)))?;

		let store = transaction
			.object_store(STORE_NAME)
			.map_err(|e| Error::other(format!("Object store error: {:?}", e)))?;

		// Convert to JsValue
		let json_str = serde_json::to_string(&event.value)
			.map_err(|e| Error::other(format!("JSON error: {:?}", e)))?;

		let js_value = js_sys::JSON::parse(&json_str)
			.map_err(|e| Error::other(format!("JS JSON parse error: {:?}", e)))?;

		let request = store
			.add(&js_value)
			.map_err(|e| Error::other(format!("Add error: {:?}", e)))?;

		Self::await_request::<JsValue>(&request).await?;

		Ok(())
	}

	/// Fire-and-forget delete from IndexedDB
	fn remove_from_idb(&self, idb_key: u32) {
		let Some(db) = &self.db else { return };
		let db = db.clone();

		spawn_local(async move {
			if let Err(e) = Self::delete_from_idb(&db, idb_key).await {
				web_sys::console::warn_1(&format!("IndexedDB delete failed: {:?}", e).into());
			}
		});
	}

	/// Actual IndexedDB delete operation
	async fn delete_from_idb(db: &IdbDatabase, idb_key: u32) -> Result<()> {
		let transaction = db
			.transaction_with_str_and_mode(STORE_NAME, web_sys::IdbTransactionMode::Readwrite)
			.map_err(|e| Error::other(format!("Transaction error: {:?}", e)))?;

		let store = transaction
			.object_store(STORE_NAME)
			.map_err(|e| Error::other(format!("Object store error: {:?}", e)))?;

		let request = store
			.delete(&JsValue::from(idb_key))
			.map_err(|e| Error::other(format!("Delete error: {:?}", e)))?;

		Self::await_request::<JsValue>(&request).await?;

		Ok(())
	}

	/// Helper to await an IdbRequest and extract the result
	async fn await_request<T: JsCast>(request: &IdbRequest) -> Result<T> {
		let (sender, receiver) = futures_channel::oneshot::channel();
		let sender = Rc::new(RefCell::new(Some(sender)));

		let success_sender = sender.clone();
		let onsuccess = Closure::once(move |_event: web_sys::Event| {
			if let Some(sender) = success_sender.borrow_mut().take() {
				let _ = sender.send(Ok(()));
			}
		});

		let error_sender = sender.clone();
		let onerror = Closure::once(move |_event: web_sys::Event| {
			if let Some(sender) = error_sender.borrow_mut().take() {
				let _ = sender.send(Err(Error::other("IndexedDB request failed")));
			}
		});

		request.set_onsuccess(Some(onsuccess.as_ref().unchecked_ref()));
		request.set_onerror(Some(onerror.as_ref().unchecked_ref()));

		onsuccess.forget();
		onerror.forget();

		receiver
			.await
			.map_err(|_| Error::other("Channel closed"))??;

		request
			.result()
			.map_err(|e| Error::other(format!("Result error: {:?}", e)))?
			.dyn_into::<T>()
			.map_err(|_| Error::other("Type cast failed"))
	}

	/// Creates a JSON batch object containing the provided items and metadata.
	fn create_batch(&self, items: &[StoredEvent]) -> Value {
		let values: Vec<&Value> = items.iter().map(|e| &e.value).collect();
		json!({
			"batch": values,
			"sentAt": Self::now_rfc3339(),
			"writeKey": self.config.write_key
		})
	}

	/// Get current timestamp in RFC3339 format using js_sys::Date
	fn now_rfc3339() -> String {
		let date = js_sys::Date::new_0();
		date.to_iso_string().into()
	}

	fn get_item_size(item: &StoredEvent) -> usize {
		item.value.to_string().len()
	}
}

impl DataStore for WebStore {
	type Output = Value;

	fn has_data(&self) -> bool {
		!self.items.is_empty()
	}

	fn reset(&mut self) {
		// Clear memory
		let items: Vec<StoredEvent> = self.items.drain(..).collect();

		// Fire-and-forget clear from IndexedDB
		for item in items {
			if let Some(key) = item.idb_key {
				self.remove_from_idb(key);
			}
		}
	}

	fn append(&mut self, data: Value) -> Result<()> {
		let event = StoredEvent {
			idb_key: Some(self.temp_key_counter),
			value: data,
		};
		self.temp_key_counter += 1;

		// Add to memory (sync)
		self.items.push_back(event.clone());

		// Enforce max_items
		while self.items.len() > self.config.max_items {
			if let Some(removed) = self.items.pop_front() {
				if let Some(key) = removed.idb_key {
					self.remove_from_idb(key);
				}
			}
		}

		// Fire-and-forget persist to IndexedDB
		self.persist_event(event);

		Ok(())
	}

	fn fetch(
		&mut self,
		count: Option<usize>,
		max_bytes: Option<usize>,
	) -> Result<Option<DataResult<Self::Output>>> {
		let max_bytes = max_bytes.unwrap_or(self.config.max_fetch_size);
		let mut accumulated_size = 0;
		let mut num_items = 0;

		for item in self.items.iter() {
			let item_size = Self::get_item_size(item);
			if accumulated_size + item_size > max_bytes {
				break;
			}
			if let Some(count) = count {
				if num_items >= count {
					break;
				}
			}
			accumulated_size += item_size;
			num_items += 1;
		}

		if num_items == 0 {
			return Ok(None);
		}

		let items: Vec<StoredEvent> = self.items.iter().take(num_items).cloned().collect();

		let removable: Vec<Box<dyn Equivalent>> = items
			.iter()
			.map(|item| Box::new(item.clone()) as Box<dyn Equivalent>)
			.collect();

		let batch = self.create_batch(&items);

		Ok(Some(DataResult {
			data: Some(batch),
			removable: Some(removable),
		}))
	}

	fn remove(&mut self, data: &[Box<dyn Equivalent>]) -> Result<()> {
		// First, collect keys to remove from IndexedDB
		let keys_to_remove: Vec<u32> = self
			.items
			.iter()
			.filter(|item| data.iter().any(|removable| removable.equals(*item)))
			.filter_map(|item| item.idb_key)
			.collect();

		// Remove from memory
		self.items
			.retain(|item| !data.iter().any(|removable| removable.equals(item)));

		// Fire-and-forget delete from IndexedDB
		for key in keys_to_remove {
			self.remove_from_idb(key);
		}

		Ok(())
	}
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
	use super::*;
	use wasm_bindgen_test::*;

	wasm_bindgen_test_configure!(run_in_browser);

	fn test_config(db_name: &str) -> WebConfig {
		WebConfig {
			write_key: "test-key".to_string(),
			database_name: db_name.to_string(),
			max_items: 1000,
			max_fetch_size: 1024,
		}
	}

	#[wasm_bindgen_test]
	async fn test_basic_operations() {
		let mut store = WebStore::new(test_config("test-basic-ops")).await;

		// Test empty state
		assert!(!store.has_data());

		// Test append
		let event = json!({"event": "test", "value": 123});
		store.append(event.clone()).unwrap();
		assert!(store.has_data());

		// Test fetch - data should still be there after fetch
		if let Some(result) = store.fetch(None, None).unwrap() {
			let batch: Value = result.data.unwrap();
			let items = batch["batch"].as_array().unwrap();
			assert_eq!(items.len(), 1);
			assert_eq!(items[0]["value"], 123);

			// Verify items are still in store after fetch
			assert!(store.has_data());

			// Now remove the items
			if let Some(removable) = result.removable {
				store.remove(&removable).unwrap();
				// Verify items were removed
				assert!(!store.has_data());
			} else {
				panic!("Expected removable items but got none");
			}
		} else {
			panic!("Expected data but got none");
		}
	}

	#[wasm_bindgen_test]
	async fn test_fifo_behavior() {
		let config = WebConfig {
			write_key: "test-key".to_string(),
			database_name: "test-fifo".to_string(),
			max_items: 3, // Small limit to test FIFO
			max_fetch_size: 1024,
		};

		let mut store = WebStore::new(config).await;

		// Add more items than max_items
		for i in 0..5 {
			store.append(json!({"index": i})).unwrap();
		}

		// Should only have last 3 items
		assert!(store.has_data());

		// Verify they're the right items (2,3,4)
		if let Some(result) = store.fetch(None, None).unwrap() {
			let batch: Value = result.data.unwrap();
			let items = batch["batch"].as_array().unwrap();
			assert_eq!(items.len(), 3);
			assert_eq!(items[0]["index"], 2);
			assert_eq!(items[1]["index"], 3);
			assert_eq!(items[2]["index"], 4);
		}
	}

	#[wasm_bindgen_test]
	async fn test_fetch_count_limit() {
		let mut store = WebStore::new(test_config("test-fetch-count")).await;

		// Add 10 items
		for i in 0..10 {
			store.append(json!({"index": i})).unwrap();
		}

		// Test count limit
		if let Some(result) = store.fetch(Some(3), None).unwrap() {
			let batch: Value = result.data.unwrap();
			let items = batch["batch"].as_array().unwrap();
			assert_eq!(items.len(), 3, "Count limit not respected");
		}
	}

	#[wasm_bindgen_test]
	async fn test_fetch_byte_limit() {
		let config = WebConfig {
			write_key: "test-key".to_string(),
			database_name: "test-fetch-bytes".to_string(),
			max_items: 100,
			max_fetch_size: 1000,
		};

		let mut store = WebStore::new(config).await;

		// Add items with predictable sizes
		for i in 0..10 {
			let padding = "x".repeat(50); // Each item will be roughly ~70 bytes
			store
				.append(json!({
					"index": i,
					"padding": padding
				}))
				.unwrap();
		}

		// Test byte limit (200 bytes should get us about 2-3 items)
		if let Some(result) = store.fetch(None, Some(200)).unwrap() {
			let batch: Value = result.data.unwrap();
			let items = batch["batch"].as_array().unwrap();
			assert!(items.len() <= 3, "Too many items for byte limit");
		}
	}

	#[wasm_bindgen_test]
	async fn test_reset() {
		let mut store = WebStore::new(test_config("test-reset")).await;

		// Add some items
		for i in 0..5 {
			store.append(json!({"index": i})).unwrap();
		}
		assert!(store.has_data());

		// Reset and verify
		store.reset();
		assert!(!store.has_data());
	}

	#[wasm_bindgen_test]
	async fn test_json_types() {
		let mut store = WebStore::new(test_config("test-json-types")).await;

		// Test all JSON types
		store.append(json!(null)).unwrap();
		store.append(json!(true)).unwrap();
		store.append(json!(42)).unwrap();
		store.append(json!(42.5)).unwrap();
		store.append(json!("string")).unwrap();
		store.append(json!([1, 2, 3])).unwrap();
		store.append(json!({"key": "value"})).unwrap();

		if let Some(result) = store.fetch(None, None).unwrap() {
			let batch: Value = result.data.unwrap();
			let items = batch["batch"].as_array().unwrap();
			assert_eq!(
				items.len(),
				7,
				"All JSON types should be stored and retrieved"
			);
		}
	}

	#[wasm_bindgen_test]
	async fn test_batch_metadata() {
		let mut store = WebStore::new(test_config("test-batch-metadata")).await;

		store.append(json!({"event": "test"})).unwrap();

		if let Some(result) = store.fetch(None, None).unwrap() {
			let batch: Value = result.data.unwrap();

			// Verify batch structure
			assert!(batch.get("batch").is_some(), "Missing 'batch' field");
			assert!(batch.get("sentAt").is_some(), "Missing 'sentAt' field");
			assert_eq!(
				batch.get("writeKey").and_then(Value::as_str),
				Some("test-key"),
				"Wrong writeKey"
			);
		}
	}

	#[wasm_bindgen_test]
	async fn test_persistence_state() {
		let store = WebStore::new(test_config("test-persistence-state")).await;

		// In a normal browser environment, IndexedDB should be available
		// This test verifies the API works - actual persistence depends on browser
		let state = store.persistence_state();
		assert!(
			state == PersistenceState::Persisted || state == PersistenceState::MemoryOnly,
			"Invalid persistence state"
		);

		// Convenience method should match
		assert_eq!(store.is_persisted(), state == PersistenceState::Persisted);
	}

	#[wasm_bindgen_test]
	async fn test_hydration_across_instances() {
		let db_name = "test-hydration";

		// First instance - add some data
		{
			let mut store = WebStore::new(WebConfig {
				write_key: "test-key".to_string(),
				database_name: db_name.to_string(),
				max_items: 1000,
				max_fetch_size: 1024,
			})
			.await;

			// Only test hydration if persistence is available
			if !store.is_persisted() {
				web_sys::console::log_1(&"Skipping hydration test - no persistence".into());
				return;
			}

			store
				.append(json!({"event": "persisted_event", "value": 42}))
				.unwrap();

			// Give fire-and-forget write time to complete
			// In real code you'd flush, but for testing we wait a bit
			gloo_timers::future::TimeoutFuture::new(100).await;
		}

		// Second instance - should hydrate the data
		{
			let mut store = WebStore::new(WebConfig {
				write_key: "test-key".to_string(),
				database_name: db_name.to_string(),
				max_items: 1000,
				max_fetch_size: 1024,
			})
			.await;

			assert!(store.has_data(), "Data should be hydrated from IndexedDB");

			if let Some(result) = store.fetch(None, None).unwrap() {
				let batch: Value = result.data.unwrap();
				let items = batch["batch"].as_array().unwrap();
				assert_eq!(items.len(), 1);
				assert_eq!(items[0]["event"], "persisted_event");
				assert_eq!(items[0]["value"], 42);

				// Clean up
				if let Some(removable) = result.removable {
					store.remove(&removable).unwrap();
				}
			}
		}
	}

	#[wasm_bindgen_test]
	async fn test_multiple_stores_isolated() {
		// Create two stores with different database names
		let mut store_a = WebStore::new(WebConfig {
			write_key: "key-a".to_string(),
			database_name: "test-isolated-a".to_string(),
			max_items: 1000,
			max_fetch_size: 1024,
		})
		.await;

		let mut store_b = WebStore::new(WebConfig {
			write_key: "key-b".to_string(),
			database_name: "test-isolated-b".to_string(),
			max_items: 1000,
			max_fetch_size: 1024,
		})
		.await;

		// Add different data to each
		store_a.append(json!({"store": "a"})).unwrap();
		store_b.append(json!({"store": "b"})).unwrap();

		// Verify isolation
		if let Some(result) = store_a.fetch(None, None).unwrap() {
			let batch: Value = result.data.unwrap();
			let items = batch["batch"].as_array().unwrap();
			assert_eq!(items.len(), 1);
			assert_eq!(items[0]["store"], "a");
		}

		if let Some(result) = store_b.fetch(None, None).unwrap() {
			let batch: Value = result.data.unwrap();
			let items = batch["batch"].as_array().unwrap();
			assert_eq!(items.len(), 1);
			assert_eq!(items[0]["store"], "b");
		}
	}

	#[wasm_bindgen_test]
	#[should_panic(expected = "max_fetch_size < 100 bytes?")]
	async fn test_rejects_tiny_max_fetch_size() {
		let config = WebConfig {
			write_key: "test-key".to_string(),
			database_name: "test-panic".to_string(),
			max_items: 1000,
			max_fetch_size: 50,
		};

		let _store = WebStore::new(config).await;
	}

	#[wasm_bindgen_test]
	#[should_panic(expected = "max_items = 0?")]
	async fn test_rejects_zero_max_items() {
		let config = WebConfig {
			write_key: "test-key".to_string(),
			database_name: "test-panic".to_string(),
			max_items: 0,
			max_fetch_size: 1024,
		};

		let _store = WebStore::new(config).await;
	}
}
