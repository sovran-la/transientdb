//! Byte-ceiling arithmetic shared by the stores, and the nesting limit on items.
//!
//! A batch is serialized as the envelope's opening, the items separated by commas, and
//! the envelope's closing (see `envelope.rs`), so its compact length is the length of the
//! envelope with no items, plus every item, plus one comma between each pair of items.

use std::collections::VecDeque;
use std::io;

mod depth;
#[cfg(test)]
pub(crate) mod script;

pub(crate) use depth::within_depth;
#[cfg(test)]
pub(crate) use depth::{nested, Shape, MAX_ITEM_DEPTH, SHAPES};

/// Bytes between two items in a serialized items array.
pub(crate) const SEPARATOR_LEN: usize = 1;

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

/// The items at the front of `queue` appended under the head item's batch number: the
/// most one fetch may return, since a batch never crosses an `end_batch` boundary. Lazy,
/// so a fetch looks only at the items it may take.
pub(crate) fn head_batch<'a, T>(
	queue: &'a VecDeque<T>,
	batch_of: impl Fn(&T) -> u64 + 'a,
) -> impl Iterator<Item = &'a T> + 'a {
	let head = queue.front().map(&batch_of);
	queue
		.iter()
		.take_while(move |item| Some(batch_of(item)) == head)
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
	fn test_head_batch_is_the_run_sharing_the_heads_number() {
		let run = |queue: &[u64]| -> Vec<u64> {
			let queue: VecDeque<u64> = queue.iter().copied().collect();
			head_batch(&queue, |&b| b).copied().collect()
		};
		assert_eq!(run(&[3, 3, 4, 4, 3]), vec![3, 3]);
		assert_eq!(run(&[5]), vec![5]);
		assert_eq!(run(&[1, 2, 3]), vec![1]);
		assert_eq!(run(&[7, 7, 7]), vec![7, 7, 7]);
		assert!(run(&[]).is_empty());
	}

	#[test]
	fn test_head_batch_is_lazy() {
		let queue: VecDeque<u64> = vec![0; 1000].into();
		let looked = std::cell::Cell::new(0);
		let taken = head_batch(&queue, |&b| {
			looked.set(looked.get() + 1);
			b
		})
		.take(3)
		.count();
		// The head's own number, then one per item taken.
		assert_eq!((taken, looked.get()), (3, 4));
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
