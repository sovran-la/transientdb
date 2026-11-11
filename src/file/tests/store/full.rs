//! A full store: the oldest item is dropped, fetched or not; a kill between the two
//! writes of a drop; `max_items` raised and lowered between runs.

use super::*;
use crate::contract::{append_all, available, tagged};
use crate::file::faults::Faults;
use crate::{DataStore, Items};
use serde_json::json;

#[test]
fn test_drops_go_oldest_first_into_the_oldest_slot() {
	let (_dir, path) = temp_path();
	let mut store = store(&path, 3);
	append_all(&mut store, &tagged(0..3));
	let len = file_len(&path);
	for i in 3..8 {
		store.append(json!({ "i": i })).unwrap();
		assert_eq!(stored(&mut store), tagged(i - 2..i + 1));
		// Each drop reuses the dropped item's slot: the file does not grow.
		assert_eq!(file_len(&path), len);
	}
	drop(store);
	assert_eq!(reopened(&path), tagged(5..8));
}

#[test]
fn test_a_dropped_pending_item_is_skipped_when_its_items_is_removed() {
	let (_dir, path) = temp_path();
	let mut store = store(&path, 3);
	append_all(&mut store, &tagged(0..3));
	let held = store.fetch(2, 100).unwrap().unwrap();
	// Drops item 0, which `held` holds.
	store.append(json!({"i": 3})).unwrap();
	assert_eq!(
		held.values(),
		tagged(0..2),
		"an Items keeps what it was handed"
	);
	// Item 0 is no longer held by anyone; item 1 still is.
	assert_eq!(store.ledger().lock().pending_count(), 1);
	assert_eq!(stored(&mut store), tagged(1..4));
	assert_eq!(available(&mut store), tagged(2..4));
	store.remove(held).unwrap();
	// Item 1 was removed; item 0 was already gone and matched nothing.
	assert_eq!(stored(&mut store), tagged(2..4));
	assert_eq!(available(&mut store), tagged(2..4));
	drop(store);
	assert_eq!(reopened(&path), tagged(2..4));
}

#[test]
fn test_a_dropped_pending_item_is_not_brought_back_by_releasing_its_items() {
	for give_back in [Items::release as fn(Items), drop] {
		let (_dir, path) = temp_path();
		let mut store = store(&path, 3);
		append_all(&mut store, &tagged(0..3));
		let held = store.fetch(3, 100).unwrap().unwrap();
		assert!(!store.has_data());
		// Drops items 0 and 1; the new ones are not held.
		append_all(&mut store, &tagged(3..5));
		assert_eq!(available(&mut store), tagged(3..5));
		give_back(held);
		assert_eq!(available(&mut store), tagged(2..5));
		assert_eq!(stored(&mut store), tagged(2..5));
	}
}

#[test]
fn test_every_held_item_dropped_then_its_items_removed() {
	let (_dir, path) = temp_path();
	let mut store = store(&path, 2);
	append_all(&mut store, &tagged(0..2));
	let held = store.fetch(2, 100).unwrap().unwrap();
	append_all(&mut store, &tagged(2..4));
	let bytes = bytes_of(&path);
	store.remove(held).unwrap();
	assert_eq!(bytes_of(&path), bytes, "nothing held was still stored");
	assert_eq!(available(&mut store), tagged(2..4));
}

#[test]
fn test_a_kill_between_the_two_writes_of_a_drop() {
	let (_dir, path) = temp_path();
	let mut first = store(&path, 2);
	append_all(&mut first, &tagged(0..2));
	drop(first);
	let hook = Faults::shared();
	let mut store = FileStore::with_hook(config(&path, SLOT, 2), hook.clone());
	let new = json!({"i": 22});
	// The payload reaches the oldest item's slot, then the process dies.
	hook.lock().unwrap().write_budget = Some(new.to_string().len());
	assert!(store.append(new).is_err());
	drop(store);
	// The next run: the oldest item's entry no longer matches its payload, so it is
	// freed at fetch; the new item was never acknowledged and is not there either.
	let mut next = super::store(&path, 2);
	assert!(next.has_data());
	assert_eq!(available(&mut next), tagged(1..2));
	assert_eq!(bytes_of(&path)[layout().entry_offset(0) as usize], 0);
	next.append(json!({"i": 3})).unwrap();
	assert_eq!(available(&mut next), vec![json!({"i": 1}), json!({"i": 3})]);
}

#[test]
fn test_a_kill_between_the_two_writes_of_a_drop_of_a_held_item() {
	let (_dir, path) = temp_path();
	let hook = Faults::shared();
	let mut store = FileStore::with_hook(config(&path, SLOT, 2), hook.clone());
	append_all(&mut store, &tagged(0..2));
	let held = store.fetch(2, 100).unwrap().unwrap();
	let new = json!({"i": 22});
	hook.lock().unwrap().write_budget = Some(new.to_string().len());
	assert!(store.append(new).is_err());
	// The dying store forgot the dropped item: its Items matches nothing for it.
	drop(held);
	drop(store);
	assert_eq!(available(&mut super::store(&path, 2)), tagged(1..2));
}

#[test]
fn test_max_items_lowered_and_raised_between_runs() {
	let (_dir, path) = temp_path();
	let mut store = super::store(&path, 6);
	append_all(&mut store, &tagged(0..6));
	drop(store);
	let six = file_len(&path);

	// Lowered: the oldest are dropped at open, and the file keeps its slots.
	let mut store = super::store(&path, 4);
	assert_eq!(available(&mut store), tagged(2..6));
	assert_eq!(file_len(&path), six);
	drop(store);
	assert_eq!(reopened(&path), tagged(2..6));

	// Raised: nothing is dropped, and the file may grow past where it was.
	let mut store = super::store(&path, 8);
	assert_eq!(stored(&mut store), tagged(2..6));
	append_all(&mut store, &tagged(6..10));
	assert_eq!(stored(&mut store), tagged(2..10));
	assert_eq!(file_len(&path), len_of(8));
	store.append(json!({"i": 10})).unwrap();
	assert_eq!(stored(&mut store), tagged(3..11));
	drop(store);

	// Lowered to 1: only the newest is left.
	let mut store = super::store(&path, 1);
	assert_eq!(available(&mut store), tagged(10..11));
	store.append(json!({"i": 11})).unwrap();
	assert_eq!(stored(&mut store), tagged(11..12));
}
