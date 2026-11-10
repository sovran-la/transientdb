use crate::{DataStore, Items, Result};
use serde_json::Value;
use std::sync::Mutex;

/// A thread-safe wrapper around a [`DataStore`], for queuing items such as events or
/// logs from several threads at once.
///
/// Every call locks the store for its duration. An [`Items`] never needs that lock: it
/// can be removed through this wrapper from any thread, and dropping or releasing it
/// works anywhere, even while another thread is inside a call.
pub struct TransientDB {
	#[cfg(not(target_arch = "wasm32"))]
	store: Mutex<Box<dyn DataStore + Send>>,

	#[cfg(target_arch = "wasm32")]
	store: Mutex<Box<dyn DataStore>>,
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
unsafe impl Send for TransientDB {}

#[cfg(target_arch = "wasm32")]
unsafe impl Sync for TransientDB {}

// Shared between threads on native targets.
#[cfg(not(target_arch = "wasm32"))]
const _: () = {
	const fn assert_send_sync<T: Send + Sync>() {}
	assert_send_sync::<TransientDB>();
};

impl TransientDB {
	/// Creates a new TransientDB around `store`.
	///
	/// # Examples
	/// ```
	/// use transientdb::{MemoryConfig, MemoryStore, TransientDB};
	///
	/// let store = MemoryStore::new(MemoryConfig {
	///     max_item_size: 8 * 1024,
	///     max_items: 1000,
	/// });
	/// let db = TransientDB::new(store);
	/// ```
	#[cfg(not(target_arch = "wasm32"))]
	pub fn new(store: impl DataStore + Send + 'static) -> Self {
		Self {
			store: Mutex::new(Box::new(store)),
		}
	}

	/// Creates a new TransientDB around `store`.
	#[cfg(target_arch = "wasm32")]
	pub fn new(store: impl DataStore + 'static) -> Self {
		Self {
			store: Mutex::new(Box::new(store)),
		}
	}

	/// Appends an item; see [`DataStore::append`].
	///
	/// # Examples
	/// ```
	/// use transientdb::{Error, MemoryConfig, MemoryStore, TransientDB};
	/// use serde_json::json;
	///
	/// let db = TransientDB::new(MemoryStore::new(MemoryConfig {
	///     max_item_size: 64,
	///     max_items: 100,
	/// }));
	///
	/// db.append(json!({"event": "user_login", "user_id": 123})).unwrap();
	///
	/// // Larger than max_item_size: never stored.
	/// let err = db.append(json!({"note": "x".repeat(100)})).unwrap_err();
	/// assert!(matches!(err, Error::ItemTooLarge { max: 64, .. }));
	/// ```
	pub fn append(&self, data: Value) -> Result<()> {
		self.store.lock().unwrap().append(data)
	}

	/// Hands out the oldest items, at most `count` of them totalling at most
	/// `max_bytes` bytes of item text; see [`DataStore::fetch`].
	///
	/// # Examples
	/// ```
	/// use transientdb::{MemoryConfig, MemoryStore, TransientDB};
	/// use serde_json::json;
	///
	/// let db = TransientDB::new(MemoryStore::new(MemoryConfig {
	///     max_item_size: 1024,
	///     max_items: 100,
	/// }));
	/// for i in 0..5 {
	///     db.append(json!({"index": i})).unwrap();
	/// }
	///
	/// let first = db.fetch(3, 64 * 1024).unwrap().unwrap();
	/// assert_eq!(first.json(), r#"[{"index":0},{"index":1},{"index":2}]"#);
	///
	/// // The first three are held by `first`, so the next fetch gets the rest.
	/// let second = db.fetch(3, 64 * 1024).unwrap().unwrap();
	/// assert_eq!(second.json(), r#"[{"index":3},{"index":4}]"#);
	/// assert!(db.fetch(3, 64 * 1024).unwrap().is_none());
	///
	/// db.remove(first).unwrap();
	/// // Dropping `second` releases it: its items are fetched again.
	/// drop(second);
	/// let again = db.fetch(10, 64 * 1024).unwrap().unwrap();
	/// assert_eq!(again.json(), r#"[{"index":3},{"index":4}]"#);
	/// ```
	pub fn fetch(&self, count: usize, max_bytes: usize) -> Result<Option<Items>> {
		self.store.lock().unwrap().fetch(count, max_bytes)
	}

	/// Deletes the items in an `Items` fetched from this TransientDB's store; see
	/// [`DataStore::remove`].
	///
	/// # Examples
	/// ```
	/// use transientdb::{MemoryConfig, MemoryStore, TransientDB};
	/// use serde_json::json;
	///
	/// let db = TransientDB::new(MemoryStore::new(MemoryConfig {
	///     max_item_size: 1024,
	///     max_items: 100,
	/// }));
	/// db.append(json!({"test": "data"})).unwrap();
	///
	/// let items = db.fetch(10, 4096).unwrap().unwrap();
	/// // ... deliver the items, then:
	/// db.remove(items).unwrap();
	/// assert!(!db.has_data());
	/// ```
	pub fn remove(&self, items: Items) -> Result<()> {
		self.store.lock().unwrap().remove(items)
	}

	/// Deletes every item, fetched or not; see [`DataStore::reset`].
	///
	/// # Examples
	/// ```
	/// use transientdb::{MemoryConfig, MemoryStore, TransientDB};
	/// use serde_json::json;
	///
	/// let db = TransientDB::new(MemoryStore::new(MemoryConfig {
	///     max_item_size: 1024,
	///     max_items: 100,
	/// }));
	/// db.append(json!({"test": "data"})).unwrap();
	/// db.reset();
	/// assert!(!db.has_data());
	/// ```
	pub fn reset(&self) {
		self.store.lock().unwrap().reset();
	}

	/// Whether a fetch could return something; see [`DataStore::has_data`].
	///
	/// # Examples
	/// ```
	/// use transientdb::{MemoryConfig, MemoryStore, TransientDB};
	/// use serde_json::json;
	///
	/// let db = TransientDB::new(MemoryStore::new(MemoryConfig {
	///     max_item_size: 1024,
	///     max_items: 100,
	/// }));
	/// assert!(!db.has_data());
	/// db.append(json!({"test": "data"})).unwrap();
	/// assert!(db.has_data());
	///
	/// // Everything stored is held by an unreleased fetch: nothing to fetch now.
	/// let items = db.fetch(10, 4096).unwrap().unwrap();
	/// assert!(!db.has_data());
	/// items.release();
	/// assert!(db.has_data());
	/// ```
	pub fn has_data(&self) -> bool {
		self.store.lock().unwrap().has_data()
	}
}
