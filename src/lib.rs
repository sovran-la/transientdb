mod batch;
mod directory;
mod envelope;
mod memory;
mod transient;

#[cfg(all(feature = "web", target_arch = "wasm32"))]
mod web;

use serde_json::Value;
use std::any::Any;
use std::fmt::Debug;
use std::io::Result;

pub use directory::{DirectoryConfig, DirectoryStore};
pub use envelope::Envelope;
pub use memory::{MemoryConfig, MemoryStore};
pub use transient::TransientDB;

#[cfg(all(feature = "web", target_arch = "wasm32"))]
pub use web::{PersistenceState, WebConfig, WebStore};

/// A new identity for a store instance, unique within the process. A store carries it
/// in every removable it issues, so its `remove` can ignore another instance's.
pub(crate) fn next_store_id() -> u64 {
	static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
	NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Represents the result of a data fetch operation.
/// Contains either raw data bytes or paths to data files, along with items that can be removed.
#[derive(Debug)]
pub struct DataResult<T> {
	pub data: Option<T>,
	pub removable: Option<Vec<Box<dyn Equivalent>>>,
}

/// Trait for types that can be compared for equality and downcasted.
/// Used primarily for tracking removable items in the data stores.
pub trait Equivalent: Any + Debug + Send {
	/// Checks if this item equals another Equivalent item
	fn equals(&self, other: &dyn Equivalent) -> bool;

	/// Allows downcasting to concrete type
	fn as_any(&self) -> &dyn Any;
}

/// A trait for implementing persistent data stores that support batched operations.
/// Provides a common interface for storing, retrieving, and managing data with support
/// for size limits and batch processing.
pub trait DataStore {
	/// The type of data returned by fetch operations.
	type Output;

	/// Checks if the store contains any data that can be fetched.
	fn has_data(&self) -> bool;

	/// Removes all data from the store and resets it to initial state.
	fn reset(&mut self);

	/// Appends a new item to the store.
	///
	/// # Arguments
	/// * `data` - JSON value to store
	///
	/// # Errors
	/// The built-in stores return `ErrorKind::InvalidInput`, and store nothing, for an
	/// item too large to fit by itself under the store's configured byte limit, envelope
	/// included. Such an item could never be delivered.
	///
	/// They also return `ErrorKind::InvalidInput`, and store nothing, for an item nested
	/// more than 125 levels deep in arrays and objects (each `[]` or `{}` is a level).
	/// `serde_json` cannot read a deeper item back from a batch, so neither could the
	/// store's own recovery or the consumer. The check does not recurse, so even a
	/// pathologically deep item is rejected without overflowing the stack.
	fn append(&mut self, data: Value) -> Result<()>;

	/// Fetches a batch of data from the store, respecting optional count and size limits.
	///
	/// # Arguments
	/// * `count` - Optional maximum number of items to fetch
	/// * `max_bytes` - Optional maximum total size in bytes to fetch
	///
	/// In the built-in stores byte limits are hard ceilings on the whole batch, envelope
	/// and separators included. `MemoryStore` and `WebStore` apply `max_bytes` (or their
	/// configured `max_fetch_size`) to the returned batch; `DirectoryStore` applies it to
	/// the total size of the returned files, each of which is at most `max_file_size`.
	///
	/// Returns the fetched data along with items that can be passed to `remove()`.
	/// Fetching removes nothing: the data stays in the store, and a later fetch returns it
	/// again, until those items are passed to `remove()`.
	fn fetch(
		&mut self,
		count: Option<usize>,
		max_bytes: Option<usize>,
	) -> Result<Option<DataResult<Self::Output>>>;

	/// Removes previously fetched data from the store.
	///
	/// # Arguments
	/// * `data` - Slice of removable items from a previous fetch operation
	///
	/// The built-in stores act only on removables issued by the same store instance
	/// (for `DirectoryStore`, on paths to its own finished files) and silently ignore
	/// anything else, returning `Ok`, as they do for items already removed.
	fn remove(&mut self, data: &[Box<dyn Equivalent>]) -> Result<()>;
}
