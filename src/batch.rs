//! Byte-ceiling arithmetic shared by the stores.
//!
//! A batch is serialized as `{"batch":[item,item,...],"sentAt":"...","writeKey":"..."}`,
//! so its compact length is the length of that envelope with an empty `batch`, plus
//! every item, plus one comma between each pair of items.

use serde_json::json;
use std::io;

/// Bytes between two items in a serialized `batch` array.
pub(crate) const SEPARATOR_LEN: usize = 1;

/// Length of the compact batch envelope with an empty `batch` array.
pub(crate) fn envelope_len(sent_at: &str, write_key: &str) -> usize {
	json!({"batch": [], "sentAt": sent_at, "writeKey": write_key})
		.to_string()
		.len()
}

/// The configured item limit caps the caller's count; a larger count is clamped.
pub(crate) fn effective_count(
	count: Option<usize>,
	max_fetch_items: Option<usize>,
) -> Option<usize> {
	match (count, max_fetch_items) {
		(Some(count), Some(limit)) => Some(count.min(limit)),
		(count, limit) => count.or(limit),
	}
}

/// How many items from the front of the queue fit in one batch whose serialized length,
/// envelope included, is at most `max_bytes`, taking at most `count` items.
pub(crate) fn leading_items_that_fit(
	item_sizes: impl IntoIterator<Item = usize>,
	envelope_len: usize,
	max_bytes: usize,
	count: Option<usize>,
) -> usize {
	let mut size = envelope_len;
	let mut fitting = 0;
	for item_size in item_sizes.into_iter().take(count.unwrap_or(usize::MAX)) {
		let separator = if fitting > 0 { SEPARATOR_LEN } else { 0 };
		if size + separator + item_size > max_bytes {
			break;
		}
		size += separator + item_size;
		fitting += 1;
	}
	fitting
}

/// How many items at the front of the queue are each larger than `max_item_len`.
/// Only WebStore can hold such items; it drops them when they reach the head.
#[cfg_attr(not(all(feature = "web", target_arch = "wasm32")), allow(dead_code))]
pub(crate) fn leading_oversized(
	item_sizes: impl IntoIterator<Item = usize>,
	max_item_len: usize,
) -> usize {
	item_sizes
		.into_iter()
		.take_while(|&size| size > max_item_len)
		.count()
}

/// The error `append` returns for an item that can never fit under the store's byte limit.
pub(crate) fn item_too_large(
	item_len: usize,
	max_item_len: usize,
	limit_name: &str,
	limit: usize,
) -> io::Error {
	io::Error::new(
		io::ErrorKind::InvalidInput,
		format!(
			"item is {item_len} bytes; the largest item that fits under {limit_name} = {limit} bytes is {max_item_len} bytes"
		),
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::Value;

	#[test]
	fn test_envelope_len_matches_serialized_batch() {
		for (sent_at, key) in [("", ""), ("2026-01-01T00:00:00Z", "key"), ("t", "🦀\"q")] {
			let len = envelope_len(sent_at, key);
			let empty = json!({"batch": [], "sentAt": sent_at, "writeKey": key});
			assert_eq!(len, serde_json::to_string(&empty).unwrap().len());

			let items: Vec<Value> = vec![json!(1), json!("two"), json!({"three": 3})];
			let full = json!({"batch": items, "sentAt": sent_at, "writeKey": key});
			let items_len: usize = items.iter().map(|i| i.to_string().len()).sum();
			assert_eq!(
				serde_json::to_string(&full).unwrap().len(),
				len + items_len + 2 * SEPARATOR_LEN
			);
		}
	}

	#[test]
	fn test_leading_items_that_fit_counts_envelope_and_separators() {
		// 10 + 5 + 1 + 5 = 21
		assert_eq!(leading_items_that_fit([5, 5, 5], 10, 21, None), 2);
		assert_eq!(leading_items_that_fit([5, 5, 5], 10, 20, None), 1);
		assert_eq!(leading_items_that_fit([5, 5, 5], 10, 27, None), 3);
		assert_eq!(leading_items_that_fit([5, 5, 5], 10, 26, None), 2);
		assert_eq!(leading_items_that_fit([5], 10, 14, None), 0);
		assert_eq!(leading_items_that_fit([5], 10, 15, None), 1);
		assert_eq!(leading_items_that_fit([], 10, 100, None), 0);
	}

	#[test]
	fn test_leading_items_that_fit_stops_at_count_and_first_misfit() {
		assert_eq!(leading_items_that_fit([1; 10], 0, 1000, Some(4)), 4);
		assert_eq!(leading_items_that_fit([1; 10], 0, 1000, Some(0)), 0);
		// The third item does not fit; the small fourth must not jump the queue.
		assert_eq!(leading_items_that_fit([5, 5, 50, 1], 10, 30, None), 2);
	}

	#[test]
	fn test_leading_items_that_fit_is_lazy() {
		let mut measured = 0;
		let sizes = (0..1000).map(|_| {
			measured += 1;
			10
		});
		assert_eq!(leading_items_that_fit(sizes, 0, 1000, Some(3)), 3);
		assert_eq!(measured, 3);
	}

	#[test]
	fn test_effective_count() {
		assert_eq!(effective_count(None, None), None);
		assert_eq!(effective_count(Some(3), None), Some(3));
		assert_eq!(effective_count(None, Some(8)), Some(8));
		assert_eq!(effective_count(Some(3), Some(8)), Some(3));
		assert_eq!(effective_count(Some(30), Some(8)), Some(8));
	}

	#[test]
	fn test_leading_oversized() {
		assert_eq!(leading_oversized([11, 12, 10, 50], 10), 2);
		assert_eq!(leading_oversized([10, 50], 10), 0);
		assert_eq!(leading_oversized([50, 50], 10), 2);
		assert_eq!(leading_oversized([], 10), 0);
	}

	#[test]
	fn test_item_too_large_message() {
		let err = item_too_large(500, 28, "max_fetch_size", 100);
		assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
		assert_eq!(
			err.to_string(),
			"item is 500 bytes; the largest item that fits under max_fetch_size = 100 bytes is 28 bytes"
		);
	}
}
