mod directory;
mod memory;
mod transient;

#[cfg(feature = "web")]
mod web;

use serde_json::Value;
use std::any::Any;
use std::fmt::Debug;
use std::io::Result;

pub use directory::{DirectoryConfig, DirectoryStore};
pub use memory::{MemoryConfig, MemoryStore};
pub use transient::TransientDB;

// MaybeSend trait - allows Send bound on native, but is a no-op on WASM
// since WASM is single-threaded and doesn't need Send.
//
// This enables WebStore (which uses Rc<IdbDatabase>) to work with TransientDB
// on WASM targets while still requiring Send on native targets where
// multi-threaded access is possible.

/// A trait that requires `Send` on native targets but is automatically
/// implemented for all types on WASM targets.
///
/// This allows types like `WebStore` (which contain `Rc<IdbDatabase>`)
/// to be used with `TransientDB` on WASM, where the `Send` bound is
/// meaningless since there are no threads.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> MaybeSend for T {}

#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T> MaybeSend for T {}

#[cfg(feature = "web")]
pub use web::{PersistenceState, WebConfig, WebStore};

/// Represents the result of a data fetch operation.
/// Contains either raw data bytes or paths to data files, along with items that can be removed.
#[derive(Debug)]
pub struct DataResult<T> {
	pub data: Option<T>,
	pub removable: Option<Vec<Box<dyn Equivalent>>>,
}

/// Trait for types that can be compared for equality and downcasted.
/// Used primarily for tracking removable items in the data stores.
pub trait Equivalent: Any + Debug {
	/// Checks if this item equals another Equivalent item
	fn equals(&self, other: &dyn Equivalent) -> bool;

	/// Allows downcasting to concrete type
	fn as_any(&self) -> &dyn Any;
}

/// A trait for implementing persistent data stores that support batched operations.
/// Provides a common interface for storing, retrieving, and managing data with support
/// for size limits and batch processing.
///
/// This trait requires `MaybeSend`, which means:
/// - On native targets: implementations must be `Send` (thread-safe)
/// - On WASM targets: no restrictions (single-threaded environment)
pub trait DataStore: MaybeSend {
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
	fn append(&mut self, data: Value) -> Result<()>;

	/// Fetches a batch of data from the store, respecting optional count and size limits.
	///
	/// # Arguments
	/// * `count` - Optional maximum number of items to fetch
	/// * `max_bytes` - Optional maximum total size in bytes to fetch
	///
	/// Returns the fetched data along with items that can be passed to `remove()`.
	fn fetch(
		&mut self,
		count: Option<usize>,
		max_bytes: Option<usize>,
	) -> Result<Option<DataResult<Self::Output>>>;

	/// Removes previously fetched data from the store.
	///
	/// # Arguments
	/// * `data` - Slice of removable items from a previous fetch operation
	fn remove(&mut self, data: &[Box<dyn Equivalent>]) -> Result<()>;
}
