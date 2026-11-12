//! `remove`: several fetches removed in every order, another store's `Items`, and a write
//! that fails part way through.

use super::*;
use crate::contract::{append_all, available, tagged};
use crate::file::faults::Op;
use crate::memory::{MemoryConfig, MemoryStore};
use crate::{DataStore, Error, Items};

#[test]
fn test_several_fetches_removed_in_every_order() {
	for order in permutations(3) {
		let (_dir, path) = temp_path();
		let mut store = store(&path, 100);
		append_all(&mut store, &tagged(0..7));
		let mut fetches: Vec<Option<Items>> =
			(0..3).map(|_| store.fetch(2, 100).unwrap()).collect();
		let mut removed = [false; 3];
		for &which in &order {
			store.remove(fetches[which].take().unwrap()).unwrap();
			removed[which] = true;
			// Stored: every group not yet removed, and the item never fetched.
			let mut left: Vec<Value> = (0..3)
				.filter(|&f| !removed[f])
				.flat_map(|f| tagged(2 * f..2 * f + 2))
				.collect();
			left.extend(tagged(6..7));
			assert_eq!(stored(&mut store), left, "{order:?}");
			// The other groups are still held back.
			assert_eq!(available(&mut store), tagged(6..7), "{order:?}");
		}
		assert_eq!(stored(&mut store), tagged(6..7), "{order:?}");
		drop(store);
		assert_eq!(reopened(&path), tagged(6..7), "{order:?}");
	}
}

#[test]
fn test_another_filestores_items_remove_nothing_and_go_back_to_it() {
	let (_dir_a, path_a) = temp_path();
	let (_dir_b, path_b) = temp_path();
	// Same config and appends: the same sequence numbers and the same slots in both.
	let (mut a, mut b) = (store(&path_a, 100), store(&path_b, 100));
	append_all(&mut a, &tagged(0..5));
	append_all(&mut b, &tagged(0..5));
	let from_a = a.fetch(3, 100).unwrap().unwrap();
	let from_b = b.fetch(5, 100).unwrap().unwrap();
	let bytes_b = bytes_of(&path_b);

	b.remove(from_a).unwrap();
	assert_eq!(bytes_of(&path_b), bytes_b);
	assert!(!b.has_data(), "b's own fetch is still held");
	// a got its items back.
	assert_eq!(available(&mut a), tagged(0..5));

	a.remove(from_b).unwrap();
	assert_eq!(stored(&mut a), tagged(0..5));
	assert_eq!(available(&mut b), tagged(0..5));
}

#[test]
fn test_items_from_and_to_a_memory_store_remove_nothing() {
	let (_dir, path) = temp_path();
	let mut file = store(&path, 100);
	let mut memory = MemoryStore::new(MemoryConfig {
		max_item_size: 64,
		max_items: 100,
	});
	append_all(&mut file, &tagged(0..3));
	append_all(&mut memory, &tagged(0..3));
	let from_file = file.fetch(3, 100).unwrap().unwrap();
	let from_memory = memory.fetch(3, 100).unwrap().unwrap();
	let bytes = bytes_of(&path);

	file.remove(from_memory).unwrap();
	assert_eq!(bytes_of(&path), bytes);
	assert_eq!(available(&mut memory), tagged(0..3));
	memory.remove(from_file).unwrap();
	assert_eq!(available(&mut file), tagged(0..3));
	assert_eq!(available(&mut memory), tagged(0..3));
}

#[test]
fn test_a_failed_write_tries_every_item_and_returns_the_first_error() {
	let (_dir, path) = temp_path();
	let (mut store, hook) = store_with(&path, 100);
	append_all(&mut store, &tagged(0..5));
	let items = store.fetch(4, 100).unwrap().unwrap();
	// The first removal fails, the rest go through.
	hook.lock().unwrap().fail_next(Op::State, 1);
	let outcome = store.remove(items);
	let Err(Error::Io(e)) = &outcome else {
		panic!("expected an I/O error, got {outcome:?}");
	};
	assert!(e.to_string().contains("injected State failure"), "{e}");
	// The one that failed is stored and fetched again, ahead of the rest.
	let mut left = tagged(0..1);
	left.extend(tagged(4..5));
	assert_eq!(stored(&mut store), left);
	assert_eq!(available(&mut store), left);
	drop(store);
	assert_eq!(reopened(&path), left);
}

#[test]
fn test_a_write_that_fails_after_the_first_item_leaves_the_rest_for_the_next_call() {
	let (_dir, path) = temp_path();
	let (mut store, hook) = store_with(&path, 100);
	append_all(&mut store, &tagged(0..5));
	let items = store.fetch(4, 100).unwrap().unwrap();
	hook.lock().unwrap().fail_after(Op::State, 1);
	assert!(matches!(store.remove(items), Err(Error::Io(_))));
	hook.lock().unwrap().fail.clear();
	assert_eq!(stored(&mut store), tagged(1..5));
	assert_eq!(available(&mut store), tagged(1..5));
	let again = store.fetch(4, 100).unwrap().unwrap();
	assert_eq!(again.values(), tagged(1..5));
	store.remove(again).unwrap();
	assert!(!store.has_data());
	assert_eq!(file_len(&path), crate::file::layout::HEADER_LEN);
}

#[test]
fn test_a_failed_write_on_every_item_changes_nothing() {
	let (_dir, path) = temp_path();
	let (mut store, hook) = store_with(&path, 100);
	append_all(&mut store, &tagged(0..3));
	let bytes = bytes_of(&path);
	let items = store.fetch(3, 100).unwrap().unwrap();
	hook.lock().unwrap().fail(Op::State);
	assert!(matches!(store.remove(items), Err(Error::Io(_))));
	hook.lock().unwrap().fail.clear();
	assert_eq!(bytes_of(&path), bytes);
	assert_eq!(available(&mut store), tagged(0..3));
}
