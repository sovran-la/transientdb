//! Opening a `WebStore`, and its `DataStore` operations.

use super::{
	adopt, connect, idb, interval, lock, pageshow, tab, warn, PersistenceState, Shared, WebConfig,
	WebStore,
};
use crate::queue::Queue;
use crate::{serialize, DataStore, Items, Result};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use web_sys::IdbDatabase;

impl Shared {
	fn new(config: WebConfig, db: Option<IdbDatabase>, closed: Rc<Cell<bool>>) -> Self {
		Self {
			queue: RefCell::new(Queue::new(config.max_items, idb::MAX_SEQ)),
			config,
			tab: RefCell::new(tab::new_id()),
			liveness: RefCell::new(None),
			db,
			closed,
			passing: Cell::new(false),
			dropped: Cell::new(false),
			generation: Cell::new(0),
			#[cfg(test)]
			passes: Cell::new(0),
		}
	}

	/// The connection to write to: `None` when there is none, or it has been closed.
	pub(super) fn live_db(&self) -> Option<&IdbDatabase> {
		self.db.as_ref().filter(|_| !self.closed.get())
	}

	/// The connection to write this tab's records with: [`live_db`](Self::live_db), while
	/// the tab's liveness lock is held.
	pub(super) fn tab_db(&self) -> Option<&IdbDatabase> {
		self.live_db().filter(|_| self.liveness.borrow().is_some())
	}

	/// Whether an adoption pass should go on: the store is still in use, persisted, and
	/// holds its tab's liveness lock.
	pub(super) fn active(&self) -> bool {
		!self.dropped.get() && self.tab_db().is_some()
	}

	/// Fire-and-forget write of the item `text` under this tab's `seq` to IndexedDB.
	pub(super) fn write_to_idb(&self, seq: u64, text: &str) {
		let Some(db) = self.tab_db() else { return };
		idb::fire_and_forget("write", idb::add(db, &self.tab.borrow(), seq, text));
	}

	/// Fire-and-forget delete of this tab's `seqs` from IndexedDB.
	pub(super) fn delete_from_idb(&self, seqs: &[u64]) {
		let Some(db) = self.tab_db() else { return };
		if seqs.is_empty() {
			return;
		}
		idb::fire_and_forget("delete", idb::delete(db, &self.tab.borrow(), seqs));
	}

	/// Waits for the liveness lock of the tab `tab`, and holds it: `true`. `false`, the lock
	/// released at once, if the store has been dropped or started over as another tab
	/// meanwhile. Returns why if the lock is not granted.
	pub(super) async fn hold_liveness(&self, tab: &str) -> std::result::Result<bool, String> {
		let name = lock::name(&self.config.database_name, tab);
		match lock::acquire(&name, false).await {
			Ok(Some(held)) => {
				let current = !self.dropped.get() && *self.tab.borrow() == tab;
				if current {
					*self.liveness.borrow_mut() = Some(held);
				}
				Ok(current)
			}
			Ok(None) => Err("Web Lock not granted".into()),
			Err(e) => Err(format!("Web Lock request failed ({e})")),
		}
	}
}

impl WebStore {
	/// Creates a new WebStore with IndexedDB persistence.
	///
	/// This opens (or creates) the IndexedDB database, takes this store's liveness lock,
	/// and adopts the items of every tab that no longer exists (see "Several tabs" on
	/// [`WebStore`]); it then adopts again every `orphan_interval` until it is dropped, and
	/// starts over as a new tab if the page is restored from the back/forward cache (see
	/// "Back/forward cache" on [`WebStore`]). A new store owns nothing of its own.
	///
	/// An adopted item's text is not parsed again: it was checked when it was appended. A
	/// stored record that is not an item (without a usable sequence number and a non-empty
	/// text string), or whose text is larger than `max_item_size`, is discarded, and if
	/// adopting takes the store past `max_items`, the oldest are dropped; both are logged
	/// to the console and deleted from IndexedDB.
	///
	/// If IndexedDB or Web Locks are unavailable (private browsing, third-party context,
	/// storage blocked, not a secure context, etc), the store falls back to memory-only
	/// mode and logs a warning to the console. Check
	/// [`persistence_state()`](Self::persistence_state) to detect this condition.
	///
	/// # Panics
	/// * If `max_item_size` is 0
	/// * If `max_items` is 0
	/// * If `orphan_interval` is zero
	pub async fn new(config: WebConfig) -> Self {
		if config.max_item_size == 0 {
			panic!("max_item_size = 0? Even `0` takes a byte. There is no item that small.");
		}
		if config.max_items == 0 {
			panic!("max_items = 0? So... you want a store that stores nothing? That's what /dev/null is for.");
		}
		if config.orphan_interval.is_zero() {
			panic!("orphan_interval = 0? Checking for orphans constantly won't make more of them.");
		}

		let memory_only = |config: WebConfig, why: String| {
			warn_memory_only(&why);
			Self::from_parts(Shared::new(config, None, Rc::new(Cell::new(false))))
		};
		if !lock::available() {
			// Without liveness locks, nothing written could ever be safely adopted.
			return memory_only(config, "Web Locks (navigator.locks) unavailable".into());
		}
		let db = match connect::open(&config.database_name).await {
			Ok(db) => db,
			Err(e) => return memory_only(config, format!("IndexedDB unavailable ({e})")),
		};
		// Before anything else, so that a newer version asked for meanwhile is not blocked.
		let closed = connect::close_on_versionchange(&db);
		let shared = Shared::new(config, Some(db), closed);
		// Held before anything is written under this tab, so its records never exist
		// without it.
		let tab = shared.tab.borrow().clone();
		if let Err(why) = shared.hold_liveness(&tab).await {
			if let Some(db) = &shared.db {
				db.close();
			}
			return memory_only(shared.config, why);
		}

		let mut store = Self::from_parts(shared);
		// Before the first pass, so that a restore from the back/forward cache during it is
		// not missed.
		store.pageshow = pageshow::Listener::start(Rc::downgrade(&store.shared));
		adopt::pass(&store.shared).await;
		let every = store.shared.config.orphan_interval;
		store.adoption = interval::Interval::start(Rc::downgrade(&store.shared), every);
		store
	}

	fn from_parts(shared: Shared) -> Self {
		Self {
			shared: Rc::new(shared),
			adoption: None,
			pageshow: None,
		}
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
		match self.shared.live_db() {
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
}

impl Drop for WebStore {
	/// Stops adopting and listening for `pageshow`, then releases the liveness lock: from
	/// then on this store's records are orphans, for another store to adopt. Writes
	/// already issued are ordered before any adopter's read of them.
	fn drop(&mut self) {
		self.shared.dropped.set(true);
		drop(self.adoption.take());
		drop(self.pageshow.take());
		drop(self.shared.liveness.take());
	}
}

impl DataStore for WebStore {
	/// Appends an item, dropping the oldest one if the store holds `max_items` already,
	/// in memory and in IndexedDB.
	///
	/// # Errors
	/// [`Error::ItemTooDeep`](crate::Error) or [`Error::ItemTooLarge`](crate::Error),
	/// changing nothing and writing nothing, as described on [`DataStore::append`]. An
	/// `Io` error, changing nothing, once the store has used up its sequence numbers: they
	/// never wrap, so no two items can share one. That takes 2^53 - 1 appends and
	/// adoptions (the most an IndexedDB number key holds exactly) by this instance. A
	/// failed IndexedDB write is not an error; it is logged, and the item is still queued
	/// in memory.
	fn append(&mut self, data: Value) -> Result<()> {
		let shared = &self.shared;
		let text = serialize::item_text(data, shared.config.max_item_size)?;
		let (seq, dropped) = shared.queue.borrow_mut().push(text.clone())?;
		shared.write_to_idb(seq, &text);
		shared.delete_from_idb(&dropped);
		Ok(())
	}

	/// Hands out the oldest items, as described on [`DataStore::fetch`]. Writes nothing
	/// to IndexedDB.
	fn fetch(&mut self, count: usize, max_bytes: usize) -> Result<Option<Items>> {
		Ok(self.shared.queue.borrow().fetch(count, max_bytes))
	}

	/// Deletes the items in `items`, in memory and in IndexedDB. The IndexedDB delete is
	/// issued during this call (see "Flushing when the page closes" on [`WebStore`]).
	///
	/// Only an `Items` this instance issued counts, so one from another `WebStore`, even
	/// one opened on the same `database_name`, is ignored.
	fn remove(&mut self, items: Items) -> Result<()> {
		let gone = self.shared.queue.borrow_mut().remove(items);
		self.shared.delete_from_idb(&gone);
		Ok(())
	}

	/// Removes every item this store holds, and every record in the database: those of
	/// every tab, live or not, as for a logout or an opt-out. Other live tabs keep the
	/// items they already hold in memory, but those are no longer stored, so they are
	/// lost if that tab goes away before sending them.
	fn reset(&mut self) {
		self.shared.generation.set(self.shared.generation.get() + 1);
		self.shared.queue.borrow_mut().reset();
		if let Some(db) = self.shared.live_db() {
			idb::fire_and_forget("clear", idb::clear(db));
		}
	}

	fn has_data(&self) -> bool {
		self.shared.queue.borrow().has_data()
	}
}

/// Logs that the store is in memory-only mode from now on, and why.
pub(super) fn warn_memory_only(why: &str) {
	warn(&format!(
		"{why}, falling back to memory-only storage. Events will not persist across page \
		 refreshes. Consider increasing flush frequency."
	));
}
