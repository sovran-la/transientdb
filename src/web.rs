//! WebStore - Browser-based persistent storage using IndexedDB with in-memory caching
//!
//! This module provides a DataStore implementation for WASM targets that:
//! - Uses an in-memory queue as the source of truth for sync operations
//! - Persists to IndexedDB via fire-and-forget async writes
//! - Hydrates from IndexedDB on initialization
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │                    WebStore                          │
//! │                                                      │
//! │  append() ──► queue ──► spawn_local() ──► IndexedDB  │
//! │                 │            (fire & forget)         │
//! │  fetch()  ◄─────┘                                    │
//! │                                                      │
//! │  On init: IndexedDB ──► hydrate ──► queue            │
//! └─────────────────────────────────────────────────────┘
//! ```
//!
//! The store assigns every item's sequence number itself, so an item has the same key in
//! memory and in IndexedDB from the moment it is appended. Each write is issued to
//! IndexedDB during the call that causes it, so writes take effect in call order; see
//! [`idb`]. Which items a fetch has handed out is kept in memory only: fetching and
//! releasing write nothing.

mod connect;
mod hydrate;
mod idb;

use crate::queue::Queue;
use crate::{serialize, DataStore, Items, Result};
use serde_json::Value;
use std::cell::Cell;
use std::rc::Rc;
use web_sys::IdbDatabase;

/// Configuration for the web-based data store.
#[derive(Clone, Debug)]
pub struct WebConfig {
	/// Name of the IndexedDB database.
	/// Different stores should use different database names to avoid collisions.
	pub database_name: String,
	/// The largest item the store accepts: its compact `serde_json` text, in bytes.
	/// `append` rejects a larger item with [`Error::ItemTooLarge`](crate::Error). A stored
	/// item larger than this (persisted under a larger limit) is discarded, and logged,
	/// when the store opens. Must not be 0.
	pub max_item_size: usize,
	/// The most items the store holds at once. Appending past it drops the oldest item,
	/// fetched or not, in memory and in IndexedDB. A store opened over more stored items
	/// than this (persisted under a larger limit) drops the oldest until they fit, and
	/// logs it. Must not be 0.
	pub max_items: usize,
}

/// Indicates the persistence state of the WebStore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceState {
	/// IndexedDB is available and working. Events persist across page refreshes.
	Persisted,
	/// IndexedDB is unavailable (private browsing, blocked by browser policy, etc), or
	/// the database is held open at an older version by another page, or the store gave
	/// up its connection so that another page could upgrade the database. See
	/// [`WebStore`]'s "Database upgrades".
	/// Events are held in memory only and will be lost on page refresh.
	///
	/// When in this state, callers should consider:
	/// - Increasing flush frequency to minimize data loss window
	/// - Logging/alerting if this is unexpected
	MemoryOnly,
}

/// A browser-based data store using IndexedDB for persistence.
///
/// Items are stored in an in-memory queue for fast synchronous access, with asynchronous
/// fire-and-forget writes to IndexedDB for persistence across page refreshes. Each item
/// is persisted as its JSON text, so one restored after a reload equals the appended
/// value and serializes to the same bytes.
///
/// Items are stored until an [`Items`] holding them is passed to `remove`, they are
/// dropped to make room, or the store is reset. Which items a fetch has handed out is
/// kept in memory only, so after a reload every stored item can be fetched again.
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
/// A database written by an earlier version of this store, in an old record
/// format, is emptied when first opened: its records are discarded.
///
/// # Database upgrades
///
/// Upgrading a database waits for every other connection to it to close. A store never
/// waits for that, and never makes another page wait:
///
/// - If another page holds the database open at an older version and does not close it
///   (a page loaded before a deploy that brought a newer version of this store), opening
///   gives up at once instead of waiting for that page to go away. The store starts in
///   memory-only mode and logs a warning. The database is left as it was, to be upgraded
///   by the next store opened once that page is gone.
/// - If another page asks for a newer version of the database, or deletes it, this
///   store closes its connection at once and carries on in memory-only mode, with every
///   item it holds, and logs a warning. Its items stored in IndexedDB stay there for the
///   newer version to deal with; nothing it does from then on is written.
///
/// So [`persistence_state()`](Self::persistence_state) can change from `Persisted` to
/// `MemoryOnly` while the store is in use, but never back.
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
	queue: Queue,
	/// IndexedDB database handle (None if unavailable/blocked)
	db: Option<Rc<IdbDatabase>>,
	/// Set once `db` has closed itself for another page's upgrade; see [`connect`].
	closed: Rc<Cell<bool>>,
}

impl WebStore {
	/// Creates a new WebStore with IndexedDB persistence.
	///
	/// This opens (or creates) the IndexedDB database and hydrates in-memory state from
	/// any previously persisted items. Stored records that are not items this store would
	/// accept (malformed, or larger than `max_item_size`) are discarded, and if more items
	/// are stored than `max_items` allows, the oldest are dropped; both are logged to the
	/// console and deleted from IndexedDB.
	///
	/// If IndexedDB is unavailable (private browsing, third-party context,
	/// storage blocked, etc), or its contents cannot be read, the store falls
	/// back to memory-only mode and logs a warning to the console. Check
	/// [`persistence_state()`](Self::persistence_state) to detect this condition.
	///
	/// # Panics
	/// * If `max_item_size` is 0
	/// * If `max_items` is 0
	pub async fn new(config: WebConfig) -> Self {
		if config.max_item_size == 0 {
			panic!("max_item_size = 0? Even `0` takes a byte. There is no item that small.");
		}
		if config.max_items == 0 {
			panic!("max_items = 0? So... you want a store that stores nothing? That's what /dev/null is for.");
		}

		let mut store = Self {
			queue: Queue::new(config.max_items, idb::MAX_SEQ),
			config,
			db: None,
			closed: Rc::new(Cell::new(false)),
		};

		// Attempt to open IndexedDB - fall back to memory-only if it fails
		let warn = |message: String| web_sys::console::warn_1(&message.into());
		let db = match connect::open(&store.config.database_name).await {
			Ok(db) => db,
			Err(e) => {
				warn(format!(
					"IndexedDB unavailable ({e}), falling back to memory-only storage. Events \
					 will not persist across page refreshes. Consider increasing flush frequency."
				));
				return store;
			}
		};
		// Before hydrating, so that a newer version asked for meanwhile is not blocked.
		store.closed = connect::close_on_versionchange(&db);
		match store.hydrate(&db).await {
			Ok(()) => store.db = Some(Rc::new(db)),
			Err(e) => {
				// Without the stored sequence numbers, new ones could collide with them, so
				// writing to this database is not safe. Leave it untouched.
				db.close();
				store.queue = Queue::new(store.config.max_items, idb::MAX_SEQ);
				warn(format!(
					"Failed to hydrate from IndexedDB ({e:?}), falling back to memory-only storage."
				));
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
	/// let store = WebStore::new(config).await;
	///
	/// let flush_interval = match store.persistence_state() {
	///     PersistenceState::Persisted => Duration::from_secs(30),
	///     PersistenceState::MemoryOnly => Duration::from_secs(5), // Flush aggressively
	/// };
	/// ```
	pub fn persistence_state(&self) -> PersistenceState {
		match self.live_db() {
			Some(_) => PersistenceState::Persisted,
			None => PersistenceState::MemoryOnly,
		}
	}

	/// Returns `true` if IndexedDB persistence is available.
	///
	/// Convenience method equivalent to checking if
	/// `persistence_state() == PersistenceState::Persisted`.
	pub fn is_persisted(&self) -> bool {
		self.persistence_state() == PersistenceState::Persisted
	}

	/// The connection to write to: `None` when there is none, or it has been closed.
	fn live_db(&self) -> Option<&IdbDatabase> {
		self.db.as_deref().filter(|_| !self.closed.get())
	}

	/// Fire-and-forget delete of `seqs` from IndexedDB
	fn delete_from_idb(&self, seqs: &[u64]) {
		let Some(db) = self.live_db() else { return };
		if seqs.is_empty() {
			return;
		}
		idb::fire_and_forget("delete", idb::delete(db, seqs));
	}
}

impl DataStore for WebStore {
	/// Appends an item, dropping the oldest one if the store holds `max_items` already,
	/// in memory and in IndexedDB.
	///
	/// # Errors
	/// [`Error::ItemTooDeep`](crate::Error) or [`Error::ItemTooLarge`](crate::Error),
	/// changing nothing and writing nothing, as described on [`DataStore::append`]. An
	/// `Io` error, changing nothing, if the store has used up its sequence numbers (after
	/// 2^53 - 1 appends, the most an IndexedDB number key holds exactly): they never wrap,
	/// so no two items can share one. A failed IndexedDB write is not an error; it is
	/// logged, and the item is still queued in memory.
	fn append(&mut self, data: Value) -> Result<()> {
		let text = serialize::item_text(data, self.config.max_item_size)?;
		let (seq, dropped) = self.queue.push(text.clone())?;
		if let Some(db) = self.live_db() {
			idb::fire_and_forget("write", idb::add(db, seq, &text));
		}
		if let Some(dropped) = dropped {
			self.delete_from_idb(&[dropped]);
		}
		Ok(())
	}

	/// Hands out the oldest items, as described on [`DataStore::fetch`]. Writes nothing
	/// to IndexedDB.
	fn fetch(&mut self, count: usize, max_bytes: usize) -> Result<Option<Items>> {
		Ok(self.queue.fetch(count, max_bytes))
	}

	/// Deletes the items in `items`, in memory and in IndexedDB.
	///
	/// Only an `Items` this instance issued counts, so one from another `WebStore` opened
	/// on the same `database_name`, holding the same items under the same sequence
	/// numbers, is ignored.
	fn remove(&mut self, items: Items) -> Result<()> {
		let gone = self.queue.remove(items);
		self.delete_from_idb(&gone);
		Ok(())
	}

	/// Removes every item, in memory and in IndexedDB.
	fn reset(&mut self) {
		self.queue.reset();
		if let Some(db) = self.live_db() {
			idb::fire_and_forget("clear", idb::clear(db));
		}
	}

	fn has_data(&self) -> bool {
		self.queue.has_data()
	}
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests;
