//! Helpers shared by the WebStore tests.

use super::*;

pub(super) fn config(db_name: &str, max_item_size: usize, max_items: usize) -> WebConfig {
	WebConfig {
		database_name: db_name.to_string(),
		max_item_size,
		max_items,
	}
}

/// A store opened on `db_name` and emptied of anything left by a previous run.
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

/// The values in `store`'s queue, fetched or not, in order.
pub(super) fn queued(store: &WebStore) -> Vec<Value> {
	let texts = store.queue.items.iter().map(|item| item.text.as_str());
	texts
		.map(|text| serde_json::from_str(text).unwrap())
		.collect()
}

/// `(seq, text)` of every item in `store`'s queue, in order.
pub(super) fn stored(store: &WebStore) -> Vec<(u64, String)> {
	let items = store.queue.items.iter();
	items.map(|item| (item.seq, item.text.clone())).collect()
}

/// `(seq, text)` of every record `store`'s IndexedDB holds, in key order, once every
/// write it issued has landed: a read issued after them on the same connection runs
/// after them.
pub(super) async fn flushed(store: &WebStore) -> Vec<(u64, String)> {
	let records = idb::read_all(store.db.as_ref().unwrap()).await.unwrap();
	records
		.iter()
		.map(|r| idb::parse_record(r).unwrap())
		.collect()
}

/// The values a freshly opened store hydrates from `db_name`, in queue order.
pub(super) async fn hydrated(db_name: &str) -> Vec<Value> {
	queued(&WebStore::new(config(db_name, 1 << 20, 1 << 20)).await)
}

/// Everything a fetch could hand over now, released at once.
pub(super) fn available(store: &mut WebStore) -> Vec<Value> {
	contract::available(store)
}
