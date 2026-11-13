//! WebStore - Browser-based persistent storage using IndexedDB with in-memory caching
//!
//! This module provides a DataStore implementation for WASM targets that:
//! - Uses an in-memory queue as the source of truth for sync operations
//! - Persists to IndexedDB via fire-and-forget async writes, under the instance's tab id
//! - Adopts the records of instances that no longer exist (orphans), when it opens and
//!   every `orphan_interval`
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────┐
//! │                        WebStore (tab T)                   │
//! │                                                           │
//! │  append() ──► queue ──► IndexedDB [T, seq]                │
//! │                 │       (issued now, fire & forget)       │
//! │  fetch()  ◄─────┘                                         │
//! │                                                           │
//! │  holds Web Lock "transientdb:{db}:tab:T" while it lives   │
//! │                                                           │
//! │  On open, every orphan_interval:                          │
//! │    tab U's lock free? ──► move U's records to [T, new seq]│
//! │                           and into the queue              │
//! └──────────────────────────────────────────────────────────┘
//! ```
//!
//! The store assigns every item's sequence number itself, so an item has the same key in
//! memory and in IndexedDB from the moment it is appended. Each write is issued to
//! IndexedDB during the call that causes it, so writes take effect in call order; see
//! [`idb`]. Which items a fetch has handed out is kept in memory only: fetching and
//! releasing write nothing.
//!
//! The queue is shared with the adoption pass ([`adopt`]), which runs in the background
//! between the store's synchronous calls. Neither holds a borrow of it across an `await`.

mod adopt;
mod connect;
mod idb;
mod interval;
mod lock;
mod pageshow;
mod store;
mod tab;
mod takeover;

use crate::queue::Queue;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;
use web_sys::IdbDatabase;

/// Configuration for the web-based data store.
#[derive(Clone, Debug)]
pub struct WebConfig {
	/// Name of the IndexedDB database.
	/// Different stores should use different database names to avoid collisions. Every
	/// tab of a page opens the same database; see [`WebStore`]'s "Several tabs".
	pub database_name: String,
	/// The largest item the store accepts: its compact `serde_json` text, in bytes.
	/// `append` rejects a larger item with [`Error::ItemTooLarge`](crate::Error). A stored
	/// item larger than this (persisted under a larger limit) is discarded, and logged,
	/// when it is adopted. Must not be 0.
	pub max_item_size: usize,
	/// The most items the store holds at once, its own and those it adopted: a soft limit.
	/// Appending past it drops the oldest items, fetched or not, in memory and in
	/// IndexedDB. Adopting past it drops the oldest items until they fit, and logs it,
	/// once the adoption pass is over, so the store can hold more until then. A pass cut
	/// short by another page's database upgrade (see [`WebStore`]'s "Database upgrades")
	/// skips that, and the excess stays until the next append drops it. Must not be 0.
	pub max_items: usize,
	/// How often the store looks for orphans to adopt: items stored by a `WebStore` that
	/// no longer exists, such as one in a tab that was closed, crashed or reloaded. It
	/// also looks once when it opens, so a reloaded page picks up what it left at once.
	///
	/// `Duration::from_secs(30)` is a good default: an orphan waits at most that long for
	/// a live tab to send it. Browsers may run timers in a hidden tab less often. Must not
	/// be zero.
	pub orphan_interval: Duration,
}

/// Indicates the persistence state of the WebStore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceState {
	/// IndexedDB is available and working. Events persist across page refreshes.
	Persisted,
	/// IndexedDB or Web Locks are unavailable (private browsing, blocked by browser
	/// policy, not a secure context, an old browser, etc), or the database is held open at
	/// an older version by another page, or the store gave up its connection so that
	/// another page could upgrade the database. See [`WebStore`]'s "Database upgrades".
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
/// is persisted as its JSON text, so one adopted after a reload equals the appended
/// value and serializes to the same bytes.
///
/// Items are stored until an [`Items`](crate::Items) holding them is passed to `remove`,
/// they are dropped to make room, or the store is reset. Which items a fetch has handed
/// out is kept in memory only, so items adopted after a reload can all be fetched again.
///
/// # Several tabs
///
/// Every tab of a page opens the same database. Each `WebStore` instance is a *tab*: it
/// gets a random tab id when it is created (so a reloaded page is a new tab), keeps its
/// records under that id, and only ever fetches, sends and removes its own items. Live
/// tabs do not coordinate and never see each other's items, and `max_items` is per tab.
///
/// Items left behind by a tab that no longer exists (closed, crashed, discarded by the
/// browser, reloaded) are *orphans*. A live tab adopts them: it moves them under its own
/// id, behind the items it already holds, and from then on they are its own, counting
/// toward its `max_items`. A store looks for orphans when it opens, before `new` returns,
/// and then every [`orphan_interval`](WebConfig::orphan_interval). Nothing is shared, so
/// items from several tabs have no order relative to each other.
///
/// Whether a tab still exists is told by a [Web Lock]: each store holds the lock
/// `transientdb:{database_name}:tab:{tab id}` from before it writes anything until it is
/// dropped, and the browser releases it when the page goes away. Adopting a tab's items
/// takes that tab's lock first, so two tabs never adopt the same items, and a tab's items
/// are never adopted while it lives. Dropping a store releases its lock: its items are
/// orphans from then on, and any store on the same database, even in the same page, may
/// adopt them.
///
/// Web Locks need a secure context (https, or `localhost`). Without them the store works
/// in memory only (see below): items it wrote could never be safely adopted.
///
/// [Web Lock]: https://developer.mozilla.org/docs/Web/API/Web_Locks_API
///
/// # Back/forward cache
///
/// A page restored from the back/forward cache starts over as a new tab: the store
/// releases its old lock, takes a new tab id and lock, and resets its queue in memory, so
/// an `Items` fetched before matches nothing. Until the new lock is granted, appended
/// items are kept in memory, and written once it is. It then adopts its own earlier items
/// like any orphan's, unless another tab adopted them while the page was cached, in which
/// case that tab sends them. A send in flight when the page was cached can therefore be
/// sent again: delivery is at least once.
///
/// How browsers treat the liveness lock, as measured:
///
/// - Firefox does not cache a page holding a Web Lock, so a page with a persisted
///   `WebStore` misses Firefox's back/forward cache.
/// - Chrome caches it with its lock held, and may evict it when another tab requests the
///   lock to adopt its items.
/// - Safari caches it but releases its locks, so another tab may adopt its items while it
///   is cached. Starting over is what keeps the restored page from sending them too.
///
/// # Flushing when the page closes
///
/// A send started while the page is closing cannot report back, so the store cannot learn
/// that it succeeded. For the last batch, remove the items *before* sending them:
///
/// ```ignore
/// if let Some(items) = store.fetch(100, 64 * 1024)? {
///     let body = items.json();
///     store.remove(items)?; // the IndexedDB delete is issued during this call
///     navigator.send_beacon_with_opt_str(url, Some(&body));
/// }
/// ```
///
/// If the beacon fails, that batch is lost: at most once, and the last batch only. If the
/// delete does not commit before the page is gone, the batch is adopted by another tab,
/// or the reloaded page, and sent again. Do the main flush on `visibilitychange` to
/// `hidden`, while the page is still alive and sends can report back, and keep this for
/// what is left.
///
/// # Persistence Behavior
///
/// The store attempts to use IndexedDB for durable storage. If IndexedDB or Web Locks
/// are unavailable (private browsing, third-party context, storage blocked, an insecure
/// context, etc), the store falls back to memory-only mode and logs a warning to the
/// console. In memory-only mode nothing is written, and nothing is adopted.
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
/// Upgrading a database waits for every other connection to it to close. A store waits
/// at most about a second for that, and never makes another page wait:
///
/// - If another page holds the database open at an older version and does not close it
///   (a page loaded before a deploy that brought a newer version of this store), opening
///   gives up after about a second instead of waiting for that page to go away. The
///   store starts in memory-only mode and logs a warning. The database is left as it
///   was, to be upgraded by the next store opened once that page is gone. A connection
///   that is closing, finishing its last writes, is waited for if it closes within that
///   second, and the store opens normally.
/// - If another page asks for a newer version of the database, or deletes it, this
///   store closes its connection at once and carries on in memory-only mode, with every
///   item it holds, and logs a warning. Its items stored in IndexedDB stay there for the
///   newer version to deal with; nothing it does from then on is written, so the items it
///   removes afterwards stay there too. It adopts nothing more.
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
	shared: Rc<Shared>,
	/// The timer that runs an adoption pass every `orphan_interval`. `None` in memory-only
	/// mode.
	adoption: Option<interval::Interval>,
	/// The `pageshow` listener that starts the store over as a new tab when the page is
	/// restored from the back/forward cache. `None` in memory-only mode.
	pageshow: Option<pageshow::Listener>,
}

/// What the store, its adoption passes and its `pageshow` listener share.
struct Shared {
	config: WebConfig,
	/// This store's tab id, under which its records are kept. Changed only when the page
	/// is restored from the back/forward cache; see [`pageshow`].
	tab: RefCell<String>,
	/// The liveness lock of `tab`, while it is held. Nothing is written under `tab`
	/// without it: `None` in memory-only mode, and from a restore from the back/forward
	/// cache until the new tab's lock is granted.
	liveness: RefCell<Option<lock::Held>>,
	/// The source of truth for sync operations. Never borrowed across an `await`.
	queue: RefCell<Queue>,
	/// IndexedDB database handle (None if unavailable/blocked)
	db: Option<IdbDatabase>,
	/// Set once `db` has closed itself for another page's upgrade; see [`connect`].
	closed: Rc<Cell<bool>>,
	/// Set while an adoption pass runs, so that no other starts.
	passing: Cell<bool>,
	/// Set once the store is dropped, so that a pass still running stops.
	dropped: Cell<bool>,
	/// How many times the store has been reset or started over as a new tab: an adoption
	/// that began before either adopts nothing, as the queue it would adopt into is gone.
	generation: Cell<u64>,
	/// How many adoption passes have started, for tests.
	#[cfg(test)]
	passes: Cell<usize>,
}

/// Logs a warning to the console, and, in tests, keeps it in `WARNINGS`.
fn warn(message: &str) {
	#[cfg(test)]
	WARNINGS.with_borrow_mut(|warnings| warnings.push(message.to_owned()));
	web_sys::console::warn_1(&message.into());
}

#[cfg(test)]
thread_local! {
	/// Every warning the store has logged through [`warn`], for tests to check and clear.
	static WARNINGS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests;
