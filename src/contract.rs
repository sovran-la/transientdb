//! For tests: the behavior every store shows through [`DataStore`], written once and run
//! against each store.
//!
//! Each check takes a store that is empty and configured with the `max_item_size` and
//! `max_items` listed for it in [`for_each_check`]. A store's test module turns the list
//! into one test per check:
//!
//! ```ignore
//! macro_rules! memory_check {
//!     ($name:ident, $max_item_size:expr, $max_items:expr) => {
//!         #[test]
//!         fn $name() {
//!             contract::$name(&mut store($max_item_size, $max_items));
//!         }
//!     };
//! }
//! contract::for_each_check!(memory_check);
//! ```

use crate::{DataStore, Items};
use serde_json::Value;

mod append;
mod fetch;
mod pending;

pub(crate) use append::*;
pub(crate) use fetch::*;
pub(crate) use pending::*;

/// Calls `$m!(check, max_item_size, max_items)` for every check.
macro_rules! for_each_check {
	($m:ident) => {
		// append
		$m!(item_of_exactly_max_item_size_is_accepted, 32, 8);
		$m!(item_over_max_item_size_is_rejected, 32, 8);
		$m!(rejected_item_changes_nothing, 32, 3);
		$m!(rejected_item_is_rejected_the_same_way_in_every_state, 32, 3);
		$m!(depth_at_the_limit_is_accepted_and_reads_back, 1 << 20, 64);
		$m!(
			depth_over_the_limit_is_rejected_without_overflow,
			1 << 20,
			64
		);
		$m!(full_store_drops_the_oldest, 64, 3);
		$m!(full_store_drops_a_pending_item, 64, 3);
		$m!(identical_items_are_distinct, 64, 16);
		// fetch
		$m!(empty_store_hands_over_nothing, 64, 8);
		$m!(count_zero_hands_over_nothing, 64, 8);
		$m!(count_limits_the_items, 64, 16);
		$m!(max_bytes_at_and_below_the_boundary, 64, 16);
		$m!(oldest_that_does_not_fit_hands_over_nothing, 64, 16);
		$m!(no_item_jumps_the_queue, 64, 16);
		$m!(bytes_are_counted_not_characters, 64, 64);
		$m!(json_values_and_iter_are_exact, 4096, 64);
		$m!(json_of_one_item, 64, 8);
		$m!(draining_in_batches_returns_everything_in_order, 64, 32);
		$m!(every_fetch_is_as_large_as_the_limits_allow, 64, 256);
		// pending
		$m!(consecutive_fetches_are_disjoint, 64, 16);
		$m!(release_makes_items_available_in_order, 64, 16);
		$m!(drop_makes_items_available_in_order, 64, 16);
		$m!(has_data_is_false_with_everything_pending, 64, 16);
		$m!(remove_deletes_exactly_what_was_fetched, 64, 16);
		$m!(remove_in_every_order, 64, 16);
		$m!(reset_clears_pending_and_old_items_match_nothing, 64, 16);
		$m!(append_after_reset_continues_in_order, 64, 16);
	};
}
pub(crate) use for_each_check;

/// A unique JSON string item whose compact text is exactly `len` bytes.
pub(crate) fn item_of_len(len: usize, tag: usize) -> Value {
	let width = len - 2;
	let item = Value::String(format!("{tag:0>width$}"));
	assert_eq!(
		item.to_string().len(),
		len,
		"tag {tag} too wide for {len} bytes"
	);
	item
}

/// `{"i": i}` for each `i`.
pub(crate) fn tagged(range: std::ops::Range<usize>) -> Vec<Value> {
	range.map(|i| serde_json::json!({ "i": i })).collect()
}

pub(crate) fn append_all(store: &mut dyn DataStore, items: &[Value]) {
	for item in items {
		store.append(item.clone()).unwrap();
	}
}

/// Fetches with roomy limits; `None` as an empty list.
pub(crate) fn fetch_values(store: &mut dyn DataStore, count: usize) -> (Vec<Value>, Option<Items>) {
	match store.fetch(count, usize::MAX).unwrap() {
		Some(items) => (items.values(), Some(items)),
		None => (Vec::new(), None),
	}
}

/// Everything a fetch could hand over now, without keeping any of it pending.
pub(crate) fn available(store: &mut dyn DataStore) -> Vec<Value> {
	fetch_values(store, usize::MAX).0
}

/// Fetches and removes until nothing is handed over; returns each fetch's items.
pub(crate) fn drain(store: &mut dyn DataStore, count: usize, max_bytes: usize) -> Vec<Vec<Value>> {
	let mut batches = Vec::new();
	while let Some(items) = store.fetch(count, max_bytes).unwrap() {
		assert!(items.len() <= count && items.byte_len() <= max_bytes);
		batches.push(items.values());
		store.remove(items).unwrap();
	}
	assert!(!store.has_data(), "drain stopped with data left");
	batches
}
