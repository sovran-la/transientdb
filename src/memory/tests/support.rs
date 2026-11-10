//! Helpers shared by the byte-ceiling tests.

use super::*;
use crate::envelope::samples::{expected_len, items_of};
use crate::DataResult;

/// The value of the test envelope's `source` field.
pub(super) const KEY: &str = "test-key";

/// A test envelope with a field and a timestamp:
/// `{"items":[...],"source":<source>,"sentAt":"..."}`.
pub(super) fn with_source(source: &str) -> Envelope {
	Envelope::new("items")
		.field("source", source)
		.timestamp("sentAt")
}

pub(super) fn store_with(max_fetch_size: usize, envelope: Envelope) -> MemoryStore {
	MemoryStore::new(MemoryConfig {
		envelope,
		max_items: Some(10_000),
		max_fetch_size,
		max_fetch_items: None,
	})
}

pub(super) fn store(max_fetch_size: usize) -> MemoryStore {
	store_with(max_fetch_size, with_source(KEY))
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

/// The length of [`with_source`]`(source)` with no items, built independently of the
/// store from the documented layout.
pub(super) fn envelope(source: &str) -> usize {
	expected_len(&with_source(source))
}

/// Items in one fetched batch, asserting its serialized length is within `limit` and
/// that it is exactly the shape of [`with_source`]`(KEY)`.
pub(super) fn checked_batch(result: &DataResult<Value>, limit: usize) -> Vec<Value> {
	checked_batch_with(result, limit, &with_source(KEY))
}

/// [`checked_batch`] for a batch under `envelope`.
pub(super) fn checked_batch_with(
	result: &DataResult<Value>,
	limit: usize,
	envelope: &Envelope,
) -> Vec<Value> {
	let batch = result.data.as_ref().unwrap();
	let len = serde_json::to_string(batch).unwrap().len();
	assert!(len <= limit, "batch is {len} bytes, over the {limit} limit");
	let items = items_of(batch, envelope);
	assert_eq!(result.removable.as_ref().unwrap().len(), items.len());
	items
}

/// Item count of one fetch, without removing anything.
pub(super) fn fetched(store: &mut MemoryStore, max_bytes: usize) -> usize {
	let envelope = store.config.envelope.clone();
	match store.fetch(None, Some(max_bytes)).unwrap() {
		Some(result) => checked_batch_with(&result, max_bytes, &envelope).len(),
		None => 0,
	}
}

/// Fetches and removes batches until the store is empty; returns each batch's items.
pub(super) fn drain(
	store: &mut MemoryStore,
	count: Option<usize>,
	max_bytes: Option<usize>,
) -> Result<Vec<Vec<Value>>> {
	let limit = max_bytes.unwrap_or(store.config.max_fetch_size);
	let envelope = store.config.envelope.clone();
	let mut batches = Vec::new();
	while let Some(result) = store.fetch(count, max_bytes)? {
		let items = checked_batch_with(&result, limit, &envelope);
		assert!(!items.is_empty());
		store.remove(&result.removable.unwrap())?;
		batches.push(items);
	}
	assert!(!store.has_data(), "drain stopped with data left");
	Ok(batches)
}

pub(super) fn assert_invalid_input(err: &std::io::Error) {
	assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput, "{err}");
}
