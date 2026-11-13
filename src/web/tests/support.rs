//! Helpers shared by the WebStore tests.

use super::*;
use std::cell::{Cell, Ref};
use std::rc::Rc;
use std::time::Duration;

/// Long enough that no interval pass runs during a test that does not ask for one.
pub(super) const NO_INTERVAL: Duration = Duration::from_secs(3600);

pub(super) fn config(db_name: &str, max_item_size: usize, max_items: usize) -> WebConfig {
	WebConfig {
		database_name: db_name.to_string(),
		max_item_size,
		max_items,
		orphan_interval: NO_INTERVAL,
	}
}

/// A store opened on `db_name` and emptied of anything left by a previous run: `reset`
/// clears every tab's records.
pub(super) async fn fresh_store(db_name: &str, max_item_size: usize, max_items: usize) -> WebStore {
	let mut store = WebStore::new(config(db_name, max_item_size, max_items)).await;
	store.reset();
	store
}

/// [`fresh_store`], or `None`, after logging, when IndexedDB is unavailable.
pub(super) async fn persisted(
	db_name: &str,
	max_item_size: usize,
	max_items: usize,
) -> Option<WebStore> {
	let store = fresh_store(db_name, max_item_size, max_items).await;
	if !store.is_persisted() {
		web_sys::console::log_1(&format!("Skipping {db_name} - no persistence").into());
		return None;
	}
	Some(store)
}

/// Lets fire-and-forget IndexedDB writes and deletes complete.
pub(super) async fn settle() {
	gloo_timers::future::TimeoutFuture::new(100).await;
}

impl WebStore {
	/// The store's queue, borrowed: drop it before calling the store again.
	pub(super) fn queue(&self) -> Ref<'_, Queue> {
		self.shared.queue.borrow()
	}

	pub(super) fn db(&self) -> IdbDatabase {
		self.shared.db.clone().unwrap()
	}

	pub(super) fn tab(&self) -> String {
		self.shared.tab.borrow().clone()
	}

	/// Runs an adoption pass now, as the interval would.
	pub(super) async fn adopt_now(&self) {
		adopt::pass(&self.shared).await;
	}
}

/// The values in `store`'s queue, fetched or not, in order.
pub(super) fn queued(store: &WebStore) -> Vec<Value> {
	let queue = store.queue();
	let texts = queue.items.iter().map(|item| item.text.as_str());
	texts
		.map(|text| serde_json::from_str(text).unwrap())
		.collect()
}

/// `(seq, text)` of every item in `store`'s queue, in order.
pub(super) fn stored(store: &WebStore) -> Vec<(u64, String)> {
	let queue = store.queue();
	let items = queue.items.iter();
	items.map(|item| (item.seq, item.text.clone())).collect()
}

/// Every record in `store`'s database, any tab's, in key order, once every write `store`
/// issued has landed: a read issued after them on the same connection runs after them.
pub(super) async fn records(store: &WebStore) -> Vec<Value> {
	idb::read_all(&store.db()).await.unwrap()
}

/// `(seq, text)` of every record `store`'s database holds under `store`'s tab, in key
/// order, once every write it issued has landed.
pub(super) async fn flushed(store: &WebStore) -> Vec<(u64, String)> {
	records(store)
		.await
		.iter()
		.filter(|r| r["tab"] == store.tab())
		.map(|r| idb::parse_record(r).unwrap())
		.collect()
}

/// The values of the records `store`'s database holds under `store`'s tab, in key order:
/// what a store would adopt from it once it is gone.
pub(super) async fn persisted_values(store: &WebStore) -> Vec<Value> {
	let flushed = flushed(store).await;
	flushed
		.iter()
		.map(|(_, text)| serde_json::from_str(text).unwrap())
		.collect()
}

/// The values a freshly opened store adopts from `db_name`, in queue order. Only orphans
/// are adopted, so every store that wrote them must be gone. The adopting store is dropped
/// again, so what it adopted is left for the next one.
pub(super) async fn adopted(db_name: &str) -> Vec<Value> {
	queued(&WebStore::new(config(db_name, 1 << 20, 1 << 20)).await)
}

/// The tabs with records in `store`'s database, in key order.
pub(super) async fn tabs(store: &WebStore) -> Vec<String> {
	let mut tabs: Vec<String> = records(store)
		.await
		.iter()
		.map(|r| r["tab"].as_str().unwrap().to_owned())
		.collect();
	tabs.dedup();
	tabs
}

/// Everything a fetch could hand over now, released at once.
pub(super) fn available(store: &mut WebStore) -> Vec<Value> {
	contract::available(store)
}

/// Takes every warning logged so far.
pub(super) fn warnings() -> Vec<String> {
	WARNINGS.take()
}

/// `count` dead tabs on `db_name`, holding `per_tab` items each, all tagged by tab.
pub(super) async fn orphans(db_name: &str, count: usize, per_tab: usize) -> Vec<Value> {
	let mut all = Vec::new();
	let mut stores = Vec::new();
	for t in 0..count {
		let mut store = WebStore::new(config(db_name, 4096, 1 << 20)).await;
		let items: Vec<Value> = (0..per_tab).map(|i| json!({"t": t, "i": i})).collect();
		contract::append_all(&mut store, &items);
		all.extend(items);
		stores.push(store);
	}
	all
}

/// Starts a pass of `store`'s in the background; the flag is set once it has finished.
pub(super) fn spawn_pass(store: &WebStore) -> Rc<Cell<bool>> {
	let done = Rc::new(Cell::new(false));
	let (shared, finished) = (store.shared.clone(), done.clone());
	wasm_bindgen_futures::spawn_local(async move {
		adopt::pass(&shared).await;
		finished.set(true);
	});
	done
}

/// Waits, a few milliseconds at a time, until `done` is true, failing with `what` if it
/// still is not after about five seconds.
pub(super) async fn until(what: &str, done: impl Fn() -> bool) {
	for _ in 0..1000 {
		if done() {
			return;
		}
		gloo_timers::future::TimeoutFuture::new(5).await;
	}
	panic!("{what}: still not done");
}

/// Waits, a few milliseconds at a time, until every flag is set.
pub(super) async fn until_all(flags: &[Rc<Cell<bool>>]) {
	for _ in 0..1000 {
		if flags.iter().all(|f| f.get()) {
			return;
		}
		gloo_timers::future::TimeoutFuture::new(5).await;
	}
	panic!("a pass did not finish");
}

pub(super) fn sorted(mut values: Vec<Value>) -> Vec<String> {
	let mut texts: Vec<String> = values.drain(..).map(|v| v.to_string()).collect();
	texts.sort();
	texts
}

/// Dispatches `pageshow` on the window: `persisted`, as when the page is restored from the
/// back/forward cache.
pub(super) fn pageshow(persisted: bool) {
	let init = web_sys::PageTransitionEventInit::new();
	init.set_persisted(persisted);
	let event = web_sys::PageTransitionEvent::new_with_event_init_dict("pageshow", &init).unwrap();
	web_sys::window().unwrap().dispatch_event(&event).unwrap();
}

/// How many `pageshow` events stores' listeners have seen so far.
pub(super) fn pageshows() -> usize {
	pageshow::PAGESHOWS.get()
}

/// Waits, a few milliseconds at a time, until `store` holds its tab's lock and runs no
/// adoption pass: it has finished starting over.
pub(super) async fn started_over(store: &WebStore) {
	for _ in 0..1000 {
		if store.shared.liveness.borrow().is_some() && !store.shared.passing.get() {
			return;
		}
		gloo_timers::future::TimeoutFuture::new(5).await;
	}
	panic!("the store did not finish starting over");
}

/// Whether the liveness lock of tab `tab` on `db_name` is held, by anyone. Asking takes
/// it for a moment if it is free.
pub(super) async fn lock_held(db_name: &str, tab: &str) -> bool {
	let name = lock::name(db_name, tab);
	lock::acquire(&name, true).await.unwrap().is_none()
}

/// Waits until the liveness lock of tab `tab` on `db_name` is free, failing if it never is.
pub(super) async fn until_free(db_name: &str, tab: &str) {
	for _ in 0..200 {
		if !lock_held(db_name, tab).await {
			return;
		}
		gloo_timers::future::TimeoutFuture::new(5).await;
	}
	panic!("the lock of tab {tab} is still held");
}

/// The values of the records `store`'s database holds under tab `tab`, in key order.
pub(super) async fn values_under(store: &WebStore, tab: &str) -> Vec<Value> {
	let records = records(store).await;
	let mine = records.iter().filter(|r| r["tab"] == tab);
	mine.map(|r| serde_json::from_str(r["text"].as_str().unwrap()).unwrap())
		.collect()
}
