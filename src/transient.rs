use crate::{DataResult, DataStore, Equivalent};
use serde_json::Value;
use std::io::Result;
use std::sync::Mutex;

/// A thread-safe wrapper around a DataStore implementation that provides temporary data storage
/// with batch processing capabilities.
///
/// TransientDB uses interior mutability through a Mutex to allow concurrent access to the
/// underlying data store. It's designed for scenarios where data needs to be temporarily
/// stored and processed in batches, such as queuing events or logs.
pub struct TransientDB<T> {
	#[cfg(not(target_arch = "wasm32"))]
	store: Mutex<Box<dyn DataStore<Output = T> + Send>>,

	#[cfg(target_arch = "wasm32")]
	store: Mutex<Box<dyn DataStore<Output = T>>>,
}

// SAFETY: On WASM32, there are no threads. Send and Sync are vacuously satisfied
// because there's nowhere to send to and nothing to synchronize with.
//
// This allows types like WebStore (which contains Rc<IdbDatabase>) to be used
// with TransientDB on WASM targets without requiring complex trait gymnastics
// that would propagate through the entire codebase.
//
// NOTE: If WASM gains real threading support (wasm32 + atomics + shared memory),
// this will need to be revisited. However, that would likely be a different
// compilation target requiring explicit opt-in.
#[cfg(target_arch = "wasm32")]
unsafe impl<T> Send for TransientDB<T> {}

#[cfg(target_arch = "wasm32")]
unsafe impl<T> Sync for TransientDB<T> {}

impl<T> TransientDB<T> {
	/// Creates a new TransientDB instance with the provided data store implementation.
	///
	/// # Arguments
	/// * `store` - Any implementation of DataStore that is Send + 'static (on native)
	///   or just DataStore + 'static (on WASM)
	///
	/// # Examples
	/// ```
	/// use transientdb::{TransientDB, MemoryConfig, MemoryStore};
	///
	/// let config = MemoryConfig {
	///     write_key: "my-store".into(),
	///     max_items: 1000,
	///     max_fetch_size: 1024 * 1024, // 1MB
	/// };
	/// let store = MemoryStore::new(config);
	/// let db = TransientDB::new(store);
	/// ```
	#[cfg(not(target_arch = "wasm32"))]
	pub fn new(store: impl DataStore<Output = T> + Send + 'static) -> Self {
		Self {
			store: Mutex::new(Box::new(store)),
		}
	}

	/// Creates a new TransientDB instance with the provided data store implementation.
	#[cfg(target_arch = "wasm32")]
	pub fn new(store: impl DataStore<Output = T> + 'static) -> Self {
		Self {
			store: Mutex::new(Box::new(store)),
		}
	}

	/// Checks if the store contains any data that can be fetched.
	///
	/// # Examples
	/// ```
	/// use transientdb::{TransientDB, MemoryStore, MemoryConfig};
	/// use serde_json::json;
	///
	/// let db = TransientDB::new(MemoryStore::new(MemoryConfig {
	///     write_key: "test".into(),
	///     max_items: 100,
	///     max_fetch_size: 1024,
	/// }));
	///
	/// assert!(!db.has_data());
	/// db.append(json!({"test": "data"})).unwrap();
	/// assert!(db.has_data());
	/// ```
	pub fn has_data(&self) -> bool {
		self.store.lock().unwrap().has_data()
	}

	/// Removes all data from the store and resets it to initial state.
	///
	/// # Examples
	/// ```
	/// use transientdb::{TransientDB, MemoryStore, MemoryConfig};
	/// use serde_json::json;
	///
	/// let db = TransientDB::new(MemoryStore::new(MemoryConfig {
	///     write_key: "test".into(),
	///     max_items: 100,
	///     max_fetch_size: 1024,
	/// }));
	///
	/// db.append(json!({"test": "data"})).unwrap();
	/// assert!(db.has_data());
	///
	/// db.reset();
	/// assert!(!db.has_data());
	/// ```
	pub fn reset(&self) {
		self.store.lock().unwrap().reset();
	}

	/// Appends a new item to the store.
	///
	/// # Arguments
	/// * `data` - JSON value to store
	///
	/// # Examples
	/// ```
	/// use transientdb::{TransientDB, MemoryStore, MemoryConfig};
	/// use serde_json::json;
	///
	/// let db = TransientDB::new(MemoryStore::new(MemoryConfig {
	///     write_key: "test".into(),
	///     max_items: 100,
	///     max_fetch_size: 1024,
	/// }));
	///
	/// // Append a single value
	/// db.append(json!({"event": "user_login", "user_id": 123})).unwrap();
	///
	/// // Append structured data
	/// db.append(json!({
	///     "event": "purchase",
	///     "details": {
	///         "item_id": "ABC123",
	///         "amount": 99.99,
	///         "currency": "USD"
	///     }
	/// })).unwrap();
	/// ```
	pub fn append(&self, data: Value) -> Result<()> {
		self.store.lock().unwrap().append(data)
	}

	/// Fetches a batch of data from the store, respecting optional count and size limits.
	///
	/// # Arguments
	/// * `count` - Optional maximum number of items to fetch
	/// * `max_bytes` - Optional maximum total size in bytes to fetch
	///
	/// # Examples
	/// ```
	/// use transientdb::{TransientDB, MemoryStore, MemoryConfig};
	/// use serde_json::json;
	///
	/// let db = TransientDB::new(MemoryStore::new(MemoryConfig {
	///     write_key: "test".into(),
	///     max_items: 100,
	///     max_fetch_size: 1024,
	/// }));
	///
	/// // Add some data
	/// for i in 0..5 {
	///     db.append(json!({"index": i})).unwrap();
	/// }
	///
	/// // Fetch up to 3 items
	/// if let Ok(Some(result)) = db.fetch(Some(3), None) {
	///     // Process the data
	///     if let Some(data) = result.data {
	///         println!("Fetched data: {:?}", data);
	///     }
	///
	///     // Clean up the fetched items
	///     if let Some(removable) = result.removable {
	///         db.remove(&removable).unwrap();
	///     }
	/// }
	///
	/// // Fetch items with size limit (1KB)
	/// let result = db.fetch(None, Some(1024));
	/// ```
	pub fn fetch(
		&self,
		count: Option<usize>,
		max_bytes: Option<usize>,
	) -> Result<Option<DataResult<T>>> {
		self.store.lock().unwrap().fetch(count, max_bytes)
	}

	/// Removes previously fetched data from the store.
	///
	/// # Arguments
	/// * `data` - Slice of removable items from a previous fetch operation
	///
	/// # Examples
	/// ```
	/// use transientdb::{TransientDB, MemoryStore, MemoryConfig};
	/// use serde_json::json;
	///
	/// let db = TransientDB::new(MemoryStore::new(MemoryConfig {
	///     write_key: "test".into(),
	///     max_items: 100,
	///     max_fetch_size: 1024,
	/// }));
	///
	/// // Add and fetch data
	/// db.append(json!({"test": "data"})).unwrap();
	///
	/// if let Ok(Some(result)) = db.fetch(None, None) {
	///     // Process the data...
	///
	///     // Then remove the processed items
	///     if let Some(removable) = result.removable {
	///         db.remove(&removable).unwrap();
	///     }
	/// }
	/// ```
	pub fn remove(&self, data: &[Box<dyn Equivalent>]) -> Result<()> {
		self.store.lock().unwrap().remove(data)
	}
}
