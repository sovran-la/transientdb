//! Helpers shared by the byte-ceiling tests.

use super::*;
use crate::{batch, DataResult};

pub(super) const KEY: &str = "test-key";

pub(super) fn store_with(max_fetch_size: usize, write_key: &str) -> MemoryStore {
	MemoryStore::new(MemoryConfig {
		write_key: write_key.to_string(),
		max_items: 10_000,
		max_fetch_size,
		max_fetch_items: None,
	})
}

pub(super) fn store(max_fetch_size: usize) -> MemoryStore {
	store_with(max_fetch_size, KEY)
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

/// A fixed `sentAt` for deterministic envelope lengths.
pub(super) const SENT_AT: &str = "2026-11-09T12:34:56.123+00:00";

pub(super) fn envelope(write_key: &str) -> usize {
	batch::envelope_len(SENT_AT, write_key)
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

/// Item count of a fetch at the fixed `SENT_AT`, without removing anything.
pub(super) fn fetched_at_fixed_time(store: &mut MemoryStore, max_bytes: usize) -> usize {
	match store.fetch_with_sent_at(None, Some(max_bytes), SENT_AT) {
		Some(result) => checked_batch(&result, max_bytes).len(),
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
	let mut batches = Vec::new();
	while let Some(result) = store.fetch(count, max_bytes)? {
		let items = checked_batch(&result, limit);
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
