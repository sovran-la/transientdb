//! Shrinking through the store: the file is exactly its header once nothing is stored,
//! and only then.

use super::*;
use crate::contract::{append_all, available, tagged};
use crate::file::faults::Op;
use crate::file::header::encode;
use crate::DataStore;
use serde_json::json;

/// Whether the file at `path` is exactly the header a store writes.
fn is_header(path: &Path) -> bool {
	bytes_of(path) == encode(SLOT as u32, ID)
}

#[test]
fn test_cut_to_its_header_when_the_last_item_is_removed_and_not_before() {
	let (_dir, path) = temp_path();
	let mut store = store(&path, 100);
	append_all(&mut store, &tagged(0..3));
	let first = store.fetch(1, 100).unwrap().unwrap();
	let rest = store.fetch(2, 100).unwrap().unwrap();
	// Everything is held: still stored, not cut.
	assert_eq!(file_len(&path), len_of(3));
	store.remove(first).unwrap();
	// One removed, two held.
	assert_eq!(file_len(&path), len_of(3));
	store.remove(rest).unwrap();
	assert!(is_header(&path));
}

#[test]
fn test_not_cut_while_an_item_is_held_and_the_others_are_removed() {
	let (_dir, path) = temp_path();
	let mut store = store(&path, 100);
	append_all(&mut store, &tagged(0..3));
	let held = store.fetch(1, 100).unwrap().unwrap();
	let rest = store.fetch(2, 100).unwrap().unwrap();
	store.remove(rest).unwrap();
	assert_eq!(file_len(&path), len_of(3));
	// Released, not removed: still stored.
	drop(held);
	assert_eq!(file_len(&path), len_of(3));
	let held = store.fetch(1, 100).unwrap().unwrap();
	store.remove(held).unwrap();
	assert!(is_header(&path));
}

#[test]
fn test_cut_when_a_drop_takes_the_last_item() {
	let (_dir, path) = temp_path();
	let (mut store, hook) = store_with(&path, 1);
	store.append(json!({"i": 0})).unwrap();
	// The drop's commit fails: the oldest is gone and the new one never stored.
	hook.lock().unwrap().fail(Op::Entry);
	assert!(store.append(json!({"i": 1})).is_err());
	hook.lock().unwrap().fail.clear();
	assert!(is_header(&path));
	assert!(!store.has_data());
}

#[test]
fn test_cut_when_the_last_item_is_freed_as_damaged() {
	let (_dir, path) = temp_path();
	let mut store = store(&path, 100);
	store.append(json!({"i": 0})).unwrap();
	poke(&path, layout().payload_offset(0) + 2, b"x");
	assert!(store.fetch(10, 100).unwrap().is_none());
	assert!(is_header(&path));
	assert!(!store.has_data());
}

#[test]
fn test_reset_cuts_to_the_header_with_items_held_or_not() {
	let (_dir, path) = temp_path();
	let mut store = store(&path, 100);
	store.reset();
	assert!(is_header(&path));
	append_all(&mut store, &tagged(0..4));
	let held = store.fetch(2, 100).unwrap().unwrap();
	store.reset();
	assert!(is_header(&path));
	assert!(!store.has_data());
	// The old Items matches nothing.
	store.remove(held).unwrap();
	assert!(is_header(&path));
}

#[test]
fn test_the_next_append_grows_the_file_again_from_slot_0() {
	for empty in ["remove", "reset"] {
		let (_dir, path) = temp_path();
		let mut store = store(&path, 100);
		append_all(&mut store, &tagged(0..3));
		if empty == "remove" {
			let all = store.fetch(3, 100).unwrap().unwrap();
			store.remove(all).unwrap();
		} else {
			store.reset();
		}
		assert!(is_header(&path), "{empty}");
		store.append(json!({"i": 9})).unwrap();
		assert_eq!(slots_of(store.core().unwrap()), vec![0], "{empty}");
		assert_eq!(file_len(&path), len_of(1), "{empty}");
		drop(store);
		assert_eq!(reopened(&path), vec![json!({"i": 9})], "{empty}");
	}
}

#[test]
fn test_a_failed_cut_after_the_last_removal_changes_nothing_else() {
	let (_dir, path) = temp_path();
	let (mut store, hook) = store_with(&path, 100);
	append_all(&mut store, &tagged(0..2));
	let all = store.fetch(2, 100).unwrap().unwrap();
	hook.lock().unwrap().fail(Op::Cut);
	// The items are removed; only the cut failed, and that is logged, not returned.
	store.remove(all).unwrap();
	hook.lock().unwrap().fail.clear();
	assert_eq!(file_len(&path), len_of(2));
	assert!(stored(&mut store).is_empty());
	assert!(!store.has_data());
	// The free slots are reused, lowest first, and the file is still valid.
	store.append(json!({"i": 5})).unwrap();
	assert_eq!(slots_of(store.core().unwrap()), vec![0]);
	assert_eq!(file_len(&path), len_of(2));
	drop(store);
	assert_eq!(reopened(&path), vec![json!({"i": 5})]);
}

#[test]
fn test_a_failed_reset_changes_nothing() {
	let (_dir, path) = temp_path();
	let (mut store, hook) = store_with(&path, 100);
	append_all(&mut store, &tagged(0..4));
	let held = store.fetch(2, 100).unwrap().unwrap();
	let bytes = bytes_of(&path);
	hook.lock().unwrap().fail(Op::Cut);
	store.reset();
	hook.lock().unwrap().fail.clear();
	assert_eq!(bytes_of(&path), bytes);
	assert_eq!(stored(&mut store), tagged(0..4));
	// What was held is still held, and its Items still removes it.
	assert_eq!(available(&mut store), tagged(2..4));
	store.remove(held).unwrap();
	assert_eq!(stored(&mut store), tagged(2..4));
	// The next reset works.
	store.reset();
	assert!(is_header(&path));
}
