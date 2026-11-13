//! Helpers shared by the WebStore tests.

use super::*;
use std::cell::Ref;
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

	pub(super) fn tab(&self) -> &str {
		&self.shared.tab
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
