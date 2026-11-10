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
//!
//! The store assigns every item's key itself, so an item has the same key in memory and
//! in IndexedDB from the moment it is appended. Each write is issued to IndexedDB during
//! the call that causes it, so writes take effect in call order; see [`idb`].

mod idb;

use crate::{batch, DataResult, DataStore, Envelope, Equivalent};
use chrono::Utc;
use serde_json::Value;
use std::any::Any;
use std::collections::{HashSet, VecDeque};
use std::io::{Error, Result};
use std::rc::Rc;
use web_sys::IdbDatabase;

/// Configuration for the web-based data store.
#[derive(Clone)]
pub struct WebConfig {
	/// The JSON object each fetched batch wraps its items in; see [`Envelope`]. IndexedDB
	/// holds bare items, so a store fetches whatever was persisted under the envelope it is
	/// configured with now.
	pub envelope: Envelope,
	/// Name of the IndexedDB database.
	/// Different stores should use different database names to avoid collisions.
	pub database_name: String,
	/// Total capacity: the maximum number of items the store holds, or `None` for no cap.
	///
	/// `None` means unbounded: nothing is ever dropped for capacity, and items leave the
	/// store only through `remove`, `reset`, or `fetch` dropping a persisted item too large
	/// to fetch (see `max_fetch_size`). The store, in memory and in IndexedDB, then grows
	/// without limit until items are removed, so memory use is the caller's responsibility.
	///
	/// `Some(n)` caps the store at `n` items: once an append takes it past `n`, the oldest
	/// items are dropped, silently, and deleted from IndexedDB. Hydration does not apply the
	/// cap; a store opened over more than `n` persisted items holds them all until its next
	/// append. Must not be `Some(0)`.
	///
	/// This does not limit how many items a single fetch returns; see `max_fetch_items`.
	pub max_items: Option<usize>,
	/// Maximum size in bytes of the batch a single fetch returns, unless `fetch` is given
	/// its own `max_bytes`.
	///
	/// This is a hard ceiling on the whole batch serialized compactly with
	/// `serde_json::to_string`: the envelope, every item, and every separator count. An
	/// item too large to fit in a batch by itself under this limit is rejected by `append`
	/// with `ErrorKind::InvalidInput`. Such an item persisted earlier (under a larger limit)
	/// is dropped by `fetch` when it reaches the head of the queue.
	///
	/// Must be at least 100 bytes, and must leave room for at least a 1-byte item once the
	/// envelope is counted.
	pub max_fetch_size: usize,
	/// Maximum number of items returned by a single fetch (the size of one batch).
	/// When `fetch` is also given a `count`, the smaller of the two applies.
	/// This is unrelated to store capacity; see `max_items`.
	/// `None` means fetches are limited only by `count` and bytes. Must not be `Some(0)`.
	pub max_fetch_items: Option<usize>,
}

/// A queued item, the key it is stored under in IndexedDB, and its batch number.
#[derive(Clone, Debug)]
struct StoredEvent {
	/// The item's key, assigned by the store at append; the same in IndexedDB.
	key: u32,
	/// The batch number the item was appended under; the same in IndexedDB.
	batch: u32,
	/// The item, exactly as appended
	value: Value,
}

/// A removable from `WebStore::fetch`: the identity of the store instance that issued
/// it and the key of one fetched item.
#[derive(Debug, PartialEq)]
struct WebKey {
	store: u64,
	key: u32,
}

impl Equivalent for WebKey {
	fn equals(&self, other: &dyn Equivalent) -> bool {
		other.as_any().downcast_ref::<WebKey>() == Some(self)
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
/// across page refreshes. Any JSON value can be stored; it is persisted and
/// restored unmodified. Items round-trip exactly: each is persisted as JSON text, so one
/// restored after a reload equals the appended value and serializes to the same bytes.
///
/// When items are fetched, they are packaged into one JSON object shaped by
/// [`WebConfig::envelope`]: the items in an array under its items key, its fields, and its
/// timestamp if it has one. The batch is a `Value`, whose key order is not meaningful.
///
/// Items are stored until a fetched batch is passed to `remove`. If `max_items` sets a
/// cap, appending past it also drops the oldest items, in memory and in IndexedDB; with
/// no cap, nothing is dropped for capacity.
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
/// A database written by an earlier version of this store, in the old record
/// format, is emptied when first opened: its records are discarded.
///
/// # Third-Party Context Warning
///
/// Modern browsers increasingly restrict storage APIs in third-party contexts
/// (iframes on different domains). Safari ITP, Firefox strict mode, and Chrome's
/// upcoming changes all affect this. In third-party contexts, expect memory-only
/// mode to be common.
pub struct WebStore {
	config: WebConfig,
	/// This instance's identity, unique within the process, carried in its removables
	id: u64,
	/// In-memory queue - this is the source of truth for sync operations
	items: VecDeque<StoredEvent>,
	/// IndexedDB database handle (None if unavailable/blocked)
	db: Option<Rc<IdbDatabase>>,
	/// The key the next appended item gets; above every key in the queue and in IndexedDB
	next_key: u32,
	/// The batch number the next appended item gets; `end_batch` advances it
	current_batch: u32,
	/// Current persistence state
	persistence_state: PersistenceState,
	/// Length of the configured envelope with no items, the same for every batch
	envelope_len: usize,
	/// The largest serialized item that fits alone in a batch under `max_fetch_size`
	max_item_size: usize,
}

impl WebStore {
	/// Creates a new WebStore with IndexedDB persistence.
	///
	/// This opens (or creates) the IndexedDB database and hydrates
	/// in-memory state from any previously persisted events.
	///
	/// If IndexedDB is unavailable (private browsing, third-party context,
	/// storage blocked, etc), or its contents cannot be read, the store falls
	/// back to memory-only mode and logs a warning to the console. Check
	/// [`persistence_state()`](Self::persistence_state) to detect this condition.
	///
	/// # Panics
	/// * If max_fetch_size is less than 100 bytes
	/// * If max_items is `Some(0)`
	/// * If max_fetch_items is `Some(0)`
	/// * If max_fetch_size leaves no room for even a 1-byte item once the batch
	///   envelope is counted
	/// * If the envelope has an empty key, uses a key twice, or does not read back as
	///   JSON (see [`Envelope`])
	pub async fn new(config: WebConfig) -> Self {
		if config.max_fetch_size < 100 {
			panic!("max_fetch_size < 100 bytes? What are you even trying to fetch, empty arrays?");
		}
		if config.max_items == Some(0) {
			panic!("max_items = 0? So... you want a store that stores nothing? That's what /dev/null is for.");
		}
		if config.max_fetch_items == Some(0) {
			panic!("max_fetch_items = Some(0)? Fetching nothing, efficiently, forever. Bold.");
		}
		config.envelope.validate();
		let envelope_len = config.envelope.byte_len();
		if envelope_len + 1 > config.max_fetch_size {
			panic!("max_fetch_size is all envelope and no room for an item? That's packaging, not a batch.");
		}

		let mut store = Self {
			envelope_len,
			max_item_size: config.max_fetch_size - envelope_len,
			config,
			id: crate::next_store_id(),
			items: VecDeque::new(),
			db: None,
			next_key: 0,
			current_batch: 0,
			persistence_state: PersistenceState::MemoryOnly,
		};

		// Attempt to open IndexedDB - fall back to memory-only if it fails
		match idb::open(&store.config.database_name).await {
			Ok(db) => match store.hydrate(&db).await {
				Ok(()) => {
					store.db = Some(Rc::new(db));
					store.persistence_state = PersistenceState::Persisted;
				}
				Err(e) => {
					// Without the stored keys, new keys could collide with them, so
					// writing to this database is not safe. Leave it untouched.
					store.items.clear();
					store.next_key = 0;
					store.current_batch = 0;
					web_sys::console::warn_1(
						&format!(
							"Failed to hydrate from IndexedDB ({:?}), falling back to \
							 memory-only storage.",
							e
						)
						.into(),
					);
				}
			},
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

	/// Loads every stored item from IndexedDB into memory, in key (append) order, and
	/// sets the next key above every key seen.
	///
	/// Each item keeps its batch number. The current batch number becomes the highest
	/// one stored, on an item or on the boundary record, so a reload is not a boundary
	/// by itself, but every `end_batch` that reached IndexedDB still is.
	///
	/// A record without the wrapper shape, including one whose value is not a string of
	/// JSON text (or a boundary record without a valid batch number), is skipped with a
	/// warning; its key, if it has one, still counts, so no new
	/// item is given it.
	async fn hydrate(&mut self, db: &IdbDatabase) -> Result<()> {
		for record in idb::read_all(db).await? {
			if let Some(key) = idb::record_key(&record) {
				self.next_key = self.next_key.max(key.saturating_add(1));
			}
			let batch = idb::record_batch(&record);
			if idb::is_boundary(&record) {
				match batch {
					Some(batch) => self.current_batch = self.current_batch.max(batch),
					None => web_sys::console::warn_1(
						&"Skipping an IndexedDB boundary record without a batch number".into(),
					),
				}
				continue;
			}
			match (idb::record_key(&record), batch, idb::record_value(&record)) {
				(Some(key), Some(batch), Some(value)) => {
					self.current_batch = self.current_batch.max(batch);
					self.items.push_back(StoredEvent { key, batch, value });
				}
				_ => web_sys::console::warn_1(
					&"Skipping an IndexedDB record that is not a stored item".into(),
				),
			}
		}
		Ok(())
	}

	/// Fire-and-forget add of `event` to IndexedDB
	fn persist_event(&self, event: &StoredEvent) {
		let Some(db) = &self.db else { return };
		idb::fire_and_forget("write", idb::add(db, event.key, event.batch, &event.value));
	}

	/// Fire-and-forget delete of `keys` from IndexedDB
	fn remove_from_idb(&self, keys: &[u32]) {
		let Some(db) = &self.db else { return };
		if keys.is_empty() {
			return;
		}
		idb::fire_and_forget("delete", idb::delete(db, keys));
	}

	fn get_item_size(item: &StoredEvent) -> usize {
		item.value.to_string().len()
	}

	/// Drops items at the head of the queue that cannot fit in a batch by themselves
	/// under `max_fetch_size`, deleting them from IndexedDB.
	///
	/// `append` rejects such items, but IndexedDB may still hold some persisted under a
	/// larger `max_fetch_size`; left at the head they would block every fetch. Only the
	/// head is examined, when a fetch reaches it.
	fn drop_oversized_head(&mut self) {
		let oversized = batch::leading_oversized(
			self.items.iter().map(Self::get_item_size),
			self.max_item_size,
		);
		let dropped: Vec<StoredEvent> = self.items.drain(..oversized).collect();
		for event in &dropped {
			web_sys::console::warn_1(
				&format!(
					"Dropping a stored item of {} bytes: the largest item that fits under \
					 max_fetch_size = {} bytes is {} bytes.",
					Self::get_item_size(event),
					self.config.max_fetch_size,
					self.max_item_size
				)
				.into(),
			);
		}
		let keys: Vec<u32> = dropped.iter().map(|e| e.key).collect();
		self.remove_from_idb(&keys);
	}

	/// The oldest items that fit as one batch, wrapped in the configured envelope.
	fn fetch_batch(
		&mut self,
		count: Option<usize>,
		max_bytes: Option<usize>,
	) -> Option<DataResult<Value>> {
		// Unlike MemoryStore, the queue can hold items too large for max_fetch_size:
		// IndexedDB may hydrate them from an earlier configuration.
		self.drop_oversized_head();

		let max_bytes = max_bytes.unwrap_or(self.config.max_fetch_size);
		let count = batch::effective_count(count, self.config.max_fetch_items);
		let num_items = batch::leading_items_that_fit(
			batch::head_batch(&self.items, |e| u64::from(e.batch)).map(Self::get_item_size),
			self.envelope_len,
			max_bytes,
			count,
		);

		if num_items == 0 {
			return None;
		}

		let items: Vec<&StoredEvent> = self.items.iter().take(num_items).collect();

		let removable: Vec<Box<dyn Equivalent>> = items
			.iter()
			.map(|item| {
				Box::new(WebKey {
					store: self.id,
					key: item.key,
				}) as Box<dyn Equivalent>
			})
			.collect();

		let values: Vec<Value> = items.iter().map(|e| e.value.clone()).collect();
		Some(DataResult {
			data: Some(self.config.envelope.batch(values, Utc::now())),
			removable: Some(removable),
		})
	}
}

impl DataStore for WebStore {
	type Output = Value;

	fn has_data(&self) -> bool {
		!self.items.is_empty()
	}

	/// Removes every item, in memory and in IndexedDB.
	fn reset(&mut self) {
		self.items.clear();
		if let Some(db) = &self.db {
			idb::fire_and_forget("clear", idb::clear(db));
		}
	}

	/// Appends an item, dropping the oldest items beyond `max_items` if it sets a cap, and
	/// deleting them from IndexedDB.
	///
	/// # Errors
	/// Returns an `InvalidInput` error, leaving the store and IndexedDB unchanged, if the
	/// serialized item cannot fit in a batch by itself under `max_fetch_size`, or if it is
	/// nested more than 125 levels deep in arrays and objects, too deep to read back from
	/// a batch or from IndexedDB.
	fn append(&mut self, data: Value) -> Result<()> {
		// First: serializing a value nested too deeply could overflow the stack.
		let data = batch::within_depth(data)?;
		let item_size = data.to_string().len();
		if item_size > self.max_item_size {
			return Err(batch::item_too_large(
				item_size,
				self.max_item_size,
				"max_fetch_size",
				self.config.max_fetch_size,
			));
		}
		let next_key = self
			.next_key
			.checked_add(1)
			.ok_or_else(|| Error::other("WebStore item keys exhausted"))?;

		let event = StoredEvent {
			key: self.next_key,
			batch: self.current_batch,
			value: data,
		};
		self.next_key = next_key;

		// Fire-and-forget persist to IndexedDB
		self.persist_event(&event);
		self.items.push_back(event);

		// Enforce max_items, if it sets a cap
		if let Some(max_items) = self.config.max_items {
			let excess = self.items.len().saturating_sub(max_items);
			let evicted: Vec<u32> = self.items.drain(..excess).map(|e| e.key).collect();
			self.remove_from_idb(&evicted);
		}

		Ok(())
	}

	/// Returns the oldest items as one batch, without removing them.
	///
	/// The batch, serialized compactly, is at most `max_bytes` (or `max_fetch_size` if
	/// `None`) bytes, envelope included, and holds at most `count` and `max_fetch_items`
	/// items. A `max_bytes` too small for the oldest item returns `Ok(None)` even though
	/// the store has data; a fetch at the configured `max_fetch_size` always returns it.
	///
	/// The batch also stops at the first [`end_batch`](DataStore::end_batch) boundary:
	/// it holds only items appended under the oldest item's batch, so it can be smaller
	/// than every limit allows while more items are stored. Once those are removed, the
	/// next fetch returns the items after the boundary.
	///
	/// Items at the head of the queue too large for any batch under `max_fetch_size`
	/// (persisted under a larger `max_fetch_size`) are dropped, deleted from IndexedDB, and
	/// logged with `console.warn`; if that empties the queue, this returns `Ok(None)`.
	/// The boundary is then the one of the new oldest item.
	fn fetch(
		&mut self,
		count: Option<usize>,
		max_bytes: Option<usize>,
	) -> Result<Option<DataResult<Self::Output>>> {
		Ok(self.fetch_batch(count, max_bytes))
	}

	/// Removes exactly the items a previous `fetch` returned, in memory and in IndexedDB.
	///
	/// Items already gone (removed before, or dropped since the fetch) are skipped.
	///
	/// Only removables this instance issued count; anything else is ignored, which is not
	/// an error. That includes the removables of another `WebStore` opened on the same
	/// `database_name`: each instance has its own identity, even when both hold the same
	/// items under the same keys, so items fetched through one are removed through that one.
	fn remove(&mut self, data: &[Box<dyn Equivalent>]) -> Result<()> {
		let keys: HashSet<u32> = data
			.iter()
			.filter_map(|removable| removable.as_any().downcast_ref::<WebKey>())
			.filter(|key| key.store == self.id)
			.map(|key| key.key)
			.collect();

		let mut removed = Vec::new();
		self.items.retain(|item| {
			let fetched = keys.contains(&item.key);
			if fetched {
				removed.push(item.key);
			}
			!fetched
		});

		// Fire-and-forget delete from IndexedDB
		self.remove_from_idb(&removed);

		Ok(())
	}

	/// Ends the current batch: items appended after this call are never fetched in the
	/// same batch as items appended before it, and a `fetch` stops at the boundary.
	///
	/// Only a batch that holds a queued item is ended. With none (nothing appended since
	/// the store was opened, since the last `end_batch`, or since those items were all
	/// removed, dropped or reset) this does nothing and returns `Ok(())`, so repeated
	/// calls are the same as one, and no empty batch ever exists.
	///
	/// The boundary survives a reload: every item is persisted with its batch number, and
	/// the new current number is written to IndexedDB too (fire-and-forget, like every
	/// write), so a boundary set after the last append is kept as well. A reload by itself
	/// is not a boundary: items appended after it may share a batch with those before.
	///
	/// # Errors
	/// Returns an error, changing nothing, if batch numbers are exhausted: they never
	/// wrap, so two batches can never share one.
	fn end_batch(&mut self) -> Result<()> {
		// Batch numbers only grow along the queue, so the newest item is the one to check.
		if self.items.back().map(|e| e.batch) != Some(self.current_batch) {
			return Ok(());
		}
		self.current_batch = self
			.current_batch
			.checked_add(1)
			.ok_or_else(|| Error::other("WebStore batch numbers exhausted"))?;
		if let Some(db) = &self.db {
			idb::fire_and_forget("boundary write", idb::put_boundary(db, self.current_batch));
		}
		Ok(())
	}
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
	use super::*;
	use serde_json::json;
	use wasm_bindgen::prelude::*;
	use wasm_bindgen_test::*;

	wasm_bindgen_test_configure!(run_in_browser);

	mod awkward_envelope;
	mod capacity;
	mod ceiling;
	mod depth;
	mod duplicates;
	mod end_batch;
	mod end_batch_limits;
	mod end_batch_reload;
	mod envelope;
	mod exact_items;
	mod foreign_removables;
	mod head_drop;
	mod isolation;
	mod oversized;
	mod persistence;
	mod reopen;
	mod schema;
	mod support;
	mod write_failure;

	fn test_config(db_name: &str) -> WebConfig {
		WebConfig {
			envelope: Envelope::default(),
			database_name: db_name.to_string(),
			max_items: Some(1000),
			max_fetch_size: 1024,
			max_fetch_items: None,
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
			let items = batch["items"].as_array().unwrap();
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
			envelope: Envelope::default(),
			database_name: "test-fifo".to_string(),
			max_items: Some(3), // Small limit to test FIFO
			max_fetch_size: 1024,
			max_fetch_items: None,
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
			let items = batch["items"].as_array().unwrap();
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
			let items = batch["items"].as_array().unwrap();
			assert_eq!(items.len(), 3, "Count limit not respected");
		}
	}

	#[wasm_bindgen_test]
	async fn test_fetch_byte_limit() {
		let config = WebConfig {
			envelope: Envelope::default(),
			database_name: "test-fetch-bytes".to_string(),
			max_items: Some(100),
			max_fetch_size: 1000,
			max_fetch_items: None,
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
			let items = batch["items"].as_array().unwrap();
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
			let items = batch["items"].as_array().unwrap();
			assert_eq!(
				items.len(),
				7,
				"All JSON types should be stored and retrieved"
			);
		}
	}

	#[wasm_bindgen_test]
	async fn test_batch_metadata() {
		let mut store = WebStore::new(WebConfig {
			envelope: support::with_source("test-key"),
			..test_config("test-batch-metadata")
		})
		.await;

		store.append(json!({"event": "test"})).unwrap();

		if let Some(result) = store.fetch(None, None).unwrap() {
			let batch: Value = result.data.unwrap();

			// Verify batch structure
			assert!(batch.get("items").is_some(), "Missing 'items' field");
			assert!(batch.get("sentAt").is_some(), "Missing 'sentAt' field");
			assert_eq!(
				batch.get("source").and_then(Value::as_str),
				Some("test-key"),
				"Wrong source"
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
				envelope: Envelope::default(),
				database_name: db_name.to_string(),
				max_items: Some(1000),
				max_fetch_size: 1024,
				max_fetch_items: None,
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
				envelope: Envelope::default(),
				database_name: db_name.to_string(),
				max_items: Some(1000),
				max_fetch_size: 1024,
				max_fetch_items: None,
			})
			.await;

			assert!(store.has_data(), "Data should be hydrated from IndexedDB");

			if let Some(result) = store.fetch(None, None).unwrap() {
				let batch: Value = result.data.unwrap();
				let items = batch["items"].as_array().unwrap();
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
			envelope: Envelope::default(),
			database_name: "test-isolated-a".to_string(),
			max_items: Some(1000),
			max_fetch_size: 1024,
			max_fetch_items: None,
		})
		.await;

		let mut store_b = WebStore::new(WebConfig {
			envelope: Envelope::default(),
			database_name: "test-isolated-b".to_string(),
			max_items: Some(1000),
			max_fetch_size: 1024,
			max_fetch_items: None,
		})
		.await;

		// Add different data to each
		store_a.append(json!({"store": "a"})).unwrap();
		store_b.append(json!({"store": "b"})).unwrap();

		// Verify isolation
		if let Some(result) = store_a.fetch(None, None).unwrap() {
			let batch: Value = result.data.unwrap();
			let items = batch["items"].as_array().unwrap();
			assert_eq!(items.len(), 1);
			assert_eq!(items[0]["store"], "a");
		}

		if let Some(result) = store_b.fetch(None, None).unwrap() {
			let batch: Value = result.data.unwrap();
			let items = batch["items"].as_array().unwrap();
			assert_eq!(items.len(), 1);
			assert_eq!(items[0]["store"], "b");
		}
	}

	#[wasm_bindgen_test]
	#[should_panic(expected = "max_fetch_size < 100 bytes?")]
	async fn test_rejects_tiny_max_fetch_size() {
		let config = WebConfig {
			envelope: Envelope::default(),
			database_name: "test-panic".to_string(),
			max_items: Some(1000),
			max_fetch_size: 50,
			max_fetch_items: None,
		};

		let _store = WebStore::new(config).await;
	}

	#[wasm_bindgen_test]
	#[should_panic(expected = "max_items = 0?")]
	async fn test_rejects_zero_max_items() {
		let config = WebConfig {
			envelope: Envelope::default(),
			database_name: "test-panic".to_string(),
			max_items: Some(0),
			max_fetch_size: 1024,
			max_fetch_items: None,
		};

		let _store = WebStore::new(config).await;
	}

	async fn item_limit_store(db_name: &str, max_fetch_items: Option<usize>) -> WebStore {
		let mut store = WebStore::new(WebConfig {
			envelope: Envelope::default(),
			database_name: db_name.to_string(),
			max_items: Some(1000),
			max_fetch_size: 1024 * 1024,
			max_fetch_items,
		})
		.await;
		// Discard anything hydrated from a previous run of the same test.
		store.reset();
		store
	}

	fn append_indexed(store: &mut WebStore, range: std::ops::Range<i64>) {
		for i in range {
			store.append(json!({"index": i})).unwrap();
		}
	}

	/// Fetches and returns the indexes in the batch, asserting removable matches the batch.
	fn fetch_indexes(
		store: &mut WebStore,
		count: Option<usize>,
		max_bytes: Option<usize>,
	) -> Vec<i64> {
		let Some(result) = store.fetch(count, max_bytes).unwrap() else {
			return Vec::new();
		};
		let batch = result.data.unwrap();
		let items = batch["items"].as_array().unwrap();
		assert_eq!(items.len(), result.removable.unwrap().len());
		items.iter().map(|v| v["index"].as_i64().unwrap()).collect()
	}

	#[wasm_bindgen_test]
	async fn test_fetch_item_limit_returns_oldest_n_in_order() {
		let mut store = item_limit_store("test-item-limit-oldest", Some(4)).await;
		append_indexed(&mut store, 0..10);
		assert_eq!(fetch_indexes(&mut store, None, None), vec![0, 1, 2, 3]);
	}

	#[wasm_bindgen_test]
	async fn test_fetch_item_limit_fewer_than_n_returns_all() {
		let mut store = item_limit_store("test-item-limit-fewer", Some(8)).await;
		append_indexed(&mut store, 0..3);
		assert_eq!(fetch_indexes(&mut store, None, None), vec![0, 1, 2]);
	}

	#[wasm_bindgen_test]
	async fn test_fetch_item_limit_combines_with_count() {
		let mut store = item_limit_store("test-item-limit-count", Some(5)).await;
		append_indexed(&mut store, 0..20);

		// count below the limit wins
		assert_eq!(fetch_indexes(&mut store, Some(2), None), vec![0, 1]);
		// count equal to the limit
		assert_eq!(fetch_indexes(&mut store, Some(5), None).len(), 5);
		// count above the limit is clamped silently
		assert_eq!(
			fetch_indexes(&mut store, Some(100), None),
			vec![0, 1, 2, 3, 4]
		);
	}

	#[wasm_bindgen_test]
	async fn test_fetch_item_limit_byte_limit_wins() {
		let mut store = item_limit_store("test-item-limit-bytes", Some(8)).await;
		for i in 0..20 {
			store
				.append(json!({"index": i, "padding": "x".repeat(50)}))
				.unwrap();
		}
		let item_size = json!({"index": 0, "padding": "x".repeat(50)})
			.to_string()
			.len();

		// Room for exactly 3 items and their 2 separators by bytes, envelope included,
		// fewer than the item limit.
		let envelope = crate::envelope::samples::expected_len(&Envelope::default());
		let indexes = fetch_indexes(&mut store, None, Some(envelope + item_size * 3 + 2));
		assert_eq!(indexes, vec![0, 1, 2]);

		// Byte limit large enough for all 20: the item limit still caps it.
		let indexes = fetch_indexes(&mut store, None, Some(item_size * 20));
		assert_eq!(indexes.len(), 8);
	}

	#[wasm_bindgen_test]
	async fn test_fetch_item_limit_walks_store_in_batches() {
		let mut store = item_limit_store("test-item-limit-walk", Some(8)).await;
		append_indexed(&mut store, 0..20);

		let mut sizes = Vec::new();
		let mut seen = Vec::new();
		while let Some(result) = store.fetch(None, None).unwrap() {
			let batch = result.data.unwrap();
			let items = batch["items"].as_array().unwrap().clone();
			let removable = result.removable.unwrap();
			assert_eq!(items.len(), removable.len());
			sizes.push(items.len());
			seen.extend(items.iter().map(|v| v["index"].as_i64().unwrap()));
			store.remove(&removable).unwrap();
		}

		assert_eq!(sizes, vec![8, 8, 4]);
		assert_eq!(seen, (0..20).collect::<Vec<_>>());
		assert!(!store.has_data());
	}

	#[wasm_bindgen_test]
	async fn test_fetch_no_item_limit_unchanged() {
		let mut store = item_limit_store("test-item-limit-none", None).await;
		append_indexed(&mut store, 0..50);
		assert_eq!(
			fetch_indexes(&mut store, None, None),
			(0..50).collect::<Vec<_>>()
		);
		assert_eq!(fetch_indexes(&mut store, Some(7), None).len(), 7);
	}

	#[wasm_bindgen_test]
	async fn test_fetch_item_limit_of_one() {
		let mut store = item_limit_store("test-item-limit-one", Some(1)).await;
		append_indexed(&mut store, 0..3);
		assert_eq!(fetch_indexes(&mut store, None, None), vec![0]);
		assert_eq!(fetch_indexes(&mut store, Some(10), None), vec![0]);
	}

	#[wasm_bindgen_test]
	async fn test_fetch_item_limit_with_max_items_drop_oldest() {
		let mut store = WebStore::new(WebConfig {
			envelope: Envelope::default(),
			database_name: "test-item-limit-max-items".to_string(),
			max_items: Some(10),
			max_fetch_size: 1024 * 1024,
			max_fetch_items: Some(4),
		})
		.await;
		store.reset();
		append_indexed(&mut store, 0..15);

		// Capacity dropped 0..5; the fetch limit still applies to what remains.
		assert_eq!(store.items.len(), 10);
		assert_eq!(fetch_indexes(&mut store, None, None), vec![5, 6, 7, 8]);
	}

	#[wasm_bindgen_test]
	#[should_panic(
		expected = "max_fetch_items = Some(0)? Fetching nothing, efficiently, forever. Bold."
	)]
	async fn test_rejects_zero_max_fetch_items() {
		let _store = item_limit_store("test-panic", Some(0)).await;
	}
}
