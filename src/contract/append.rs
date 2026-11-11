//! `append`: the size and depth checks, and dropping the oldest item when full.

use super::*;
use crate::serialize::{nested, Shape, MAX_ITEM_DEPTH, SHAPES};
use crate::Error;
use serde_json::json;

pub(crate) fn item_of_exactly_max_item_size_is_accepted(store: &mut dyn DataStore) {
	let largest = item_of_len(32, 1);
	store.append(largest.clone()).unwrap();
	store.append(json!(7)).unwrap();
	let items = store.fetch(10, 1000).unwrap().unwrap();
	assert_eq!(items.values(), vec![largest, json!(7)]);
	assert_eq!(items.byte_len(), 33);
}

pub(crate) fn item_over_max_item_size_is_rejected(store: &mut dyn DataStore) {
	for len in [33, 34, 1000] {
		let err = store.append(item_of_len(len, 0)).unwrap_err();
		assert!(
			matches!(err, Error::ItemTooLarge { size, max: 32 } if size == len),
			"{err:?}"
		);
	}
	assert!(!store.has_data());
	assert!(store.fetch(10, 1000).unwrap().is_none());
	// Counted in bytes: eight 4-byte characters and two quotes are 34 bytes.
	assert!(matches!(
		store.append(json!("🦀".repeat(8))),
		Err(Error::ItemTooLarge { size: 34, max: 32 })
	));
}

pub(crate) fn rejected_item_changes_nothing(store: &mut dyn DataStore) {
	// Full: a rejected append must not drop the oldest item to make room.
	let kept = tagged(0..3);
	append_all(store, &kept);
	assert!(store.append(item_of_len(33, 9)).is_err());
	assert!(store
		.append(nested(MAX_ITEM_DEPTH + 1, Shape::Arrays, json!(0)))
		.is_err());
	assert!(store
		.append(nested(100_000, Shape::Mixed, json!(0)))
		.is_err());
	assert_eq!(available(store), kept);
	// Nor disturb what is pending.
	let pending = store.fetch(1, 1000).unwrap().unwrap();
	assert!(store.append(item_of_len(40, 9)).is_err());
	assert_eq!(available(store), kept[1..]);
	drop(pending);
	assert_eq!(available(store), kept);
	store.append(json!("next")).unwrap();
	assert_eq!(available(store), [&kept[1..], &[json!("next")]].concat());
}

/// Asserts that `store`, whose `max_item_size` is `max_item_size`, rejects an item larger
/// than that with [`Error::ItemTooLarge`], giving the item's size and the limit. Holds in
/// any state a store can be in, including one where it could store nothing.
pub(crate) fn assert_rejects_oversized_items(store: &mut dyn DataStore, max_item_size: usize) {
	for len in [
		max_item_size + 1,
		max_item_size + 2,
		max_item_size * 8 + 100,
	] {
		let err = store.append(item_of_len(len, 0)).unwrap_err();
		assert!(
			matches!(err, Error::ItemTooLarge { size, max } if size == len && max == max_item_size),
			"{len} bytes: {err:?}"
		);
	}
}

/// Asserts that `store` rejects an item nested past the limit with [`Error::ItemTooDeep`],
/// in every shape and up to `deepest` levels deep, without overflowing the stack, and
/// before its size is looked at. Holds in any state a store can be in, including one
/// where it could store nothing.
pub(crate) fn assert_rejects_too_deep_items(store: &mut dyn DataStore, deepest: usize) {
	for shape in SHAPES {
		for depth in [deepest, MAX_ITEM_DEPTH + 1, MAX_ITEM_DEPTH + 2] {
			let err = store.append(nested(depth, shape, json!(0))).unwrap_err();
			assert!(
				matches!(err, Error::ItemTooDeep),
				"{shape:?} {depth}: {err:?}"
			);
		}
	}
	let deep_and_large = nested(MAX_ITEM_DEPTH + 1, Shape::Objects, json!("x".repeat(4096)));
	assert!(matches!(
		store.append(deep_and_large),
		Err(Error::ItemTooDeep)
	));
}

pub(crate) fn rejected_item_is_rejected_the_same_way_in_every_state(store: &mut dyn DataStore) {
	let check = |store: &mut dyn DataStore, state: &str, expected: &[Value]| {
		assert_rejects_oversized_items(store, 32);
		assert_rejects_too_deep_items(store, 100_000);
		assert_eq!(available(store), expected, "{state}");
	};
	check(store, "empty", &[]);
	append_all(store, &tagged(0..2));
	check(store, "holding items", &tagged(0..2));
	store.append(json!({"i": 2})).unwrap();
	check(store, "full", &tagged(0..3));
	let oldest = store.fetch(1, 1000).unwrap().unwrap();
	check(store, "full, the oldest pending", &tagged(1..3));
	let rest = store.fetch(10, 1000).unwrap().unwrap();
	check(store, "everything pending", &[]);
	drop(rest);
	store.remove(oldest).unwrap();
	check(store, "after a remove", &tagged(1..3));
	store.reset();
	check(store, "after a reset", &[]);
	store.append(json!("next")).unwrap();
	assert_eq!(available(store), vec![json!("next")]);
}

/// Each shape at the deepest accepted depth, around a scalar, an empty array and an empty
/// object.
fn deepest_items() -> Vec<Value> {
	SHAPES
		.iter()
		.flat_map(|&shape| {
			[json!(0), json!([]), json!({})].map(|inner| nested(MAX_ITEM_DEPTH, shape, inner))
		})
		.collect()
}

pub(crate) fn depth_at_the_limit_is_accepted_and_reads_back(store: &mut dyn DataStore) {
	let items = deepest_items();
	append_all(store, &items);
	let fetched = store.fetch(100, usize::MAX).unwrap().unwrap();
	assert_eq!(fetched.values(), items);
	// What a caller builds from them reads back too.
	let body = format!("{{\"events\":{}}}", fetched.json());
	let body: Value = serde_json::from_str(&body).unwrap();
	assert_eq!(body["events"], Value::Array(items));
}

pub(crate) fn depth_over_the_limit_is_rejected_without_overflow(store: &mut dyn DataStore) {
	for shape in SHAPES {
		for depth in [MAX_ITEM_DEPTH + 1, MAX_ITEM_DEPTH + 2, 100_000] {
			let err = store.append(nested(depth, shape, json!(0))).unwrap_err();
			assert!(
				matches!(err, Error::ItemTooDeep),
				"{shape:?} {depth}: {err:?}"
			);
		}
	}
	assert!(!store.has_data());
	// Depth is checked before size: too deep and too large is too deep.
	let deep_and_large = nested(
		MAX_ITEM_DEPTH + 1,
		Shape::Objects,
		json!("x".repeat(2 << 20)),
	);
	assert!(matches!(
		store.append(deep_and_large),
		Err(Error::ItemTooDeep)
	));
	store.append(json!("after")).unwrap();
	assert_eq!(available(store), vec![json!("after")]);
}

pub(crate) fn full_store_drops_the_oldest(store: &mut dyn DataStore) {
	for (i, item) in tagged(0..5).into_iter().enumerate() {
		store.append(item).unwrap();
		let first = i.saturating_sub(2);
		assert_eq!(available(store), tagged(first..i + 1));
	}
	assert_eq!(drain(store, 10, 1000).concat(), tagged(2..5));
}

pub(crate) fn full_store_drops_a_pending_item(store: &mut dyn DataStore) {
	append_all(store, &tagged(0..3));
	let first = store.fetch(1, 1000).unwrap().unwrap();
	assert_eq!(first.values(), tagged(0..1));
	// Full: item 0 is dropped although it is pending.
	store.append(json!({"i": 3})).unwrap();
	assert_eq!(available(store), tagged(1..4));
	// Removing its Items now matches nothing and deletes nothing else.
	store.remove(first).unwrap();
	assert_eq!(available(store), tagged(1..4));

	// A pending item dropped and then released does not come back.
	let second = store.fetch(2, 1000).unwrap().unwrap();
	assert_eq!(second.values(), tagged(1..3));
	store.append(json!({"i": 4})).unwrap();
	assert_eq!(available(store), tagged(3..5));
	second.release();
	assert_eq!(available(store), tagged(2..5));

	// An Items whose items were partly dropped still removes the rest.
	let third = store.fetch(3, 1000).unwrap().unwrap();
	assert_eq!(third.values(), tagged(2..5));
	store.append(json!({"i": 5})).unwrap();
	store.remove(third).unwrap();
	assert_eq!(drain(store, 10, 1000).concat(), tagged(5..6));
}

pub(crate) fn identical_items_are_distinct(store: &mut dyn DataStore) {
	let same = json!({"event": "click"});
	append_all(store, &vec![same.clone(); 4]);
	let one = store.fetch(1, 1000).unwrap().unwrap();
	assert_eq!(one.values(), vec![same.clone()]);
	store.remove(one).unwrap();
	assert_eq!(available(store), vec![same.clone(); 3]);

	let (a, b) = (json!("a"), json!({"b": 1}));
	store.reset();
	let appended = vec![a.clone(), a.clone(), b.clone(), a.clone(), b.clone(), b, a];
	append_all(store, &appended);
	assert_eq!(drain(store, 2, 1000).concat(), appended);
}
