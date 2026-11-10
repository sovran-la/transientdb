//! Lightweight queues for transient data: append JSON items, fetch them, remove them once
//! they are delivered.
//!
//! Every store implements [`DataStore`]. [`MemoryStore`] keeps its items in memory;
//! `WebStore` (feature `web`, `wasm32` only) keeps them in IndexedDB as well, so they
//! survive a page reload. [`TransientDB`] wraps any store behind a mutex so threads can
//! share it.
//!
//! ```
//! use serde_json::json;
//! use transientdb::{MemoryConfig, MemoryStore, TransientDB};
//!
//! let db = TransientDB::new(MemoryStore::new(MemoryConfig {
//!     max_item_size: 8 * 1024,
//!     max_items: 1000,
//! }));
//! db.append(json!({"event": "login"})).unwrap();
//!
//! if let Some(items) = db.fetch(100, 64 * 1024).unwrap() {
//!     let body = format!("{{\"batch\":{}}}", items.json());
//!     // ... send `body`. On success:
//!     db.remove(items).unwrap();
//!     // On failure, drop `items` instead: a later fetch returns them again.
//! }
//! assert!(!db.has_data());
//! ```

mod claims;
#[cfg(test)]
mod contract;
mod error;
#[cfg(not(target_arch = "wasm32"))]
mod file;
#[cfg(test)]
mod item_samples;
mod items;
mod memory;
mod queue;
mod serialize;
mod transient;

#[cfg(all(feature = "web", target_arch = "wasm32"))]
mod web;

use serde_json::Value;

pub use error::{Error, Result};
pub use items::Items;
pub use memory::{MemoryConfig, MemoryStore};
pub use transient::TransientDB;

#[cfg(all(feature = "web", target_arch = "wasm32"))]
pub use web::{PersistenceState, WebConfig, WebStore};

/// A queue of JSON items: append them, fetch them, and remove them once delivered.
///
/// Every store takes the same two limits, `max_item_size` and `max_items`, so an item
/// one store accepts, every store accepts.
pub trait DataStore {
	/// Appends an item.
	///
	/// The item is stored as its compact `serde_json` text. A store already holding
	/// `max_items` items drops its oldest one to make room, whether or not that one has
	/// been fetched; appending never fails for being full. An `Items` holding a dropped
	/// item still removes or releases the rest normally.
	///
	/// # Errors
	/// [`Error::ItemTooDeep`], storing nothing, for an item nested more than 125 levels
	/// deep in arrays and objects (each `[]` or `{}` is a level): `serde_json` could not
	/// read it back once a caller puts the items in a request body. The check does not
	/// recurse, so even a pathologically deep item is rejected without overflowing the
	/// stack. Otherwise [`Error::ItemTooLarge`], storing nothing, for an item whose text
	/// is longer than `max_item_size` bytes. A store with storage of its own may also
	/// return the errors it documents.
	fn append(&mut self, data: Value) -> Result<()>;

	/// Hands out the oldest items, in the order they were appended: at most `count` of
	/// them, whose text totals at most `max_bytes` bytes.
	///
	/// `max_bytes` counts item bytes only: whatever the caller puts around or between the
	/// items is the caller's to account for. The items are taken in order, stopping at
	/// the first one that does not fit; a smaller item behind it does not jump the queue.
	///
	/// Returns `Ok(None)` when there is nothing to hand over: nothing is waiting, `count`
	/// is 0, or the oldest waiting item alone is larger than `max_bytes`. Otherwise the
	/// returned [`Items`] holds at least one item.
	///
	/// Fetching deletes nothing, but the items handed out are not returned by another
	/// fetch until their `Items` is released, so consecutive fetches never return the same
	/// item. Pass the `Items` to [`remove`](Self::remove) once its items are delivered, or
	/// drop it (or call [`Items::release`]) to have them returned by a later fetch.
	fn fetch(&mut self, count: usize, max_bytes: usize) -> Result<Option<Items>>;

	/// Deletes the items in an `Items` this store handed out.
	///
	/// Items no longer stored (dropped to make room since the fetch) are skipped. An
	/// `Items` from another store, or from before a [`reset`](Self::reset), matches
	/// nothing: it is ignored, which is not an error. Another store's `Items` is released
	/// back to that store.
	fn remove(&mut self, items: Items) -> Result<()>;

	/// Deletes every item, fetched or not. An `Items` fetched before the reset matches
	/// nothing afterwards: removing or releasing it does nothing.
	fn reset(&mut self);

	/// Whether a fetch could return something: some item is stored and not held by an
	/// unreleased `Items`.
	fn has_data(&self) -> bool;
}
