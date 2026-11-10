//! Helpers shared by the byte-ceiling tests.

use super::*;

pub(super) const KEY: &str = "test-key";

pub(super) fn config(db_name: &str, max_fetch_size: usize, write_key: &str) -> WebConfig {
	WebConfig {
		write_key: write_key.to_string(),
		database_name: db_name.to_string(),
		max_items: Some(10_000),
		max_fetch_size,
		max_fetch_items: None,
	}
}

/// A store opened on `db_name` and emptied of anything left by a previous run.
pub(super) async fn fresh_store(db_name: &str, max_fetch_size: usize) -> WebStore {
	fresh_store_with(config(db_name, max_fetch_size, KEY)).await
}

pub(super) async fn fresh_store_with(config: WebConfig) -> WebStore {
	let mut store = WebStore::new(config).await;
	store.reset();
	store
}

/// A unique JSON string item whose compact serialization is exactly `len` bytes.
pub(super) fn item_of_len(len: usize, tag: usize) -> Value {
	let width = len - 2;
	let item = Value::String(format!("{tag:0>width$}"));
	assert_eq!(item_len(&item), len, "tag {tag} too wide for {len} bytes");
	item
}

pub(super) fn item_len(item: &Value) -> usize {
	serde_json::to_string(item).unwrap().len()
}

/// The envelope length; WebStore's timestamp is fixed-width, so this is exact.
pub(super) fn envelope(write_key: &str) -> usize {
	WebStore::widest_envelope_len(write_key)
}

/// Items in one fetched batch, asserting its serialized length is within `limit`.
pub(super) fn checked_batch(result: &DataResult<Value>, limit: usize) -> Vec<Value> {
	let batch = result.data.as_ref().unwrap();
	let len = serde_json::to_string(batch).unwrap().len();
	assert!(len <= limit, "batch is {len} bytes, over the {limit} limit");
	assert!(batch["sentAt"].is_string());
	let items = batch["batch"].as_array().unwrap().clone();
	assert_eq!(result.removable.as_ref().unwrap().len(), items.len());
	items
}

/// Item count of one fetch, without removing anything.
pub(super) fn fetched(store: &mut WebStore, max_bytes: usize) -> usize {
	match store.fetch(None, Some(max_bytes)).unwrap() {
		Some(result) => checked_batch(&result, max_bytes).len(),
		None => 0,
	}
}

/// Fetches and removes batches until the store is empty; returns each batch's items.
pub(super) fn drain(
	store: &mut WebStore,
	count: Option<usize>,
	max_bytes: Option<usize>,
) -> Vec<Vec<Value>> {
	let limit = max_bytes.unwrap_or(store.config.max_fetch_size);
	let mut batches = Vec::new();
	while let Some(result) = store.fetch(count, max_bytes).unwrap() {
		let items = checked_batch(&result, limit);
		assert!(!items.is_empty());
		store.remove(&result.removable.unwrap()).unwrap();
		batches.push(items);
	}
	assert!(!store.has_data(), "drain stopped with data left");
	batches
}

pub(super) fn assert_invalid_input(err: &std::io::Error) {
	assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput, "{err}");
}

/// Lets fire-and-forget IndexedDB writes and deletes complete.
pub(super) async fn settle() {
	gloo_timers::future::TimeoutFuture::new(100).await;
}

/// The values in `store`'s queue, in order.
pub(super) fn queued(store: &WebStore) -> Vec<Value> {
	store.items.iter().map(|e| e.value.clone()).collect()
}

/// The values a freshly opened store hydrates from `db_name`, in queue order.
pub(super) async fn hydrated(db_name: &str, max_fetch_size: usize) -> Vec<Value> {
	let store = WebStore::new(config(db_name, max_fetch_size, KEY)).await;
	store.items.iter().map(|e| e.value.clone()).collect()
}
