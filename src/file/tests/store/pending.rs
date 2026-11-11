//! Fetched items are held back from other fetches until released, in memory only: nothing
//! is written for it, and it does not survive reopening.

use super::*;
use crate::contract::{append_all, available, tagged};
use crate::{DataStore, Items};
use std::panic::{catch_unwind, AssertUnwindSafe};

/// A store holding `tagged(0..n)`, its faults with nothing recorded yet, and its path's
/// directory.
fn holding(n: usize) -> (tempfile::TempDir, std::path::PathBuf, FileStore, Hook) {
	let (dir, path) = temp_path();
	let (mut store, hook) = store_with(&path, 100);
	append_all(&mut store, &tagged(0..n));
	drain(&hook);
	(dir, path, store, hook)
}

#[test]
fn test_consecutive_fetches_are_disjoint_and_write_nothing() {
	let (_dir, path, mut store, hook) = holding(7);
	let bytes = bytes_of(&path);
	let fetches: Vec<Items> = (0..3)
		.map(|_| store.fetch(2, 100).unwrap().unwrap())
		.collect();
	let values: Vec<Vec<Value>> = fetches.iter().map(Items::values).collect();
	assert_eq!(values, vec![tagged(0..2), tagged(2..4), tagged(4..6)]);
	let last = store.fetch(10, 100).unwrap().unwrap();
	assert_eq!(last.values(), tagged(6..7));
	assert!(store.fetch(10, 100).unwrap().is_none());
	assert!(drain(&hook).is_empty());
	assert_eq!(bytes_of(&path), bytes);
	// Every item is still stored.
	assert_eq!(stored(&mut store), tagged(0..7));
}

/// Fetches two groups, gives the first back with `give_back`, and checks it is fetched
/// again ahead of newer items, in sequence order, with nothing written for any of it.
fn given_back_in_order(give_back: impl Fn(&mut FileStore) -> Items) {
	let (_dir, path, mut store, hook) = holding(6);
	let bytes = bytes_of(&path);
	let second = give_back(&mut store);
	assert_eq!(second.values(), tagged(2..4));
	assert!(drain(&hook).is_empty());
	assert_eq!(bytes_of(&path), bytes);
	let mut expected = tagged(0..2);
	expected.extend(tagged(4..6));
	assert_eq!(available(&mut store), expected);
	drop(second);
	assert_eq!(available(&mut store), tagged(0..6));
	assert!(drain(&hook).is_empty());
	assert_eq!(bytes_of(&path), bytes);
}

#[test]
fn test_release_makes_items_available_again_in_order() {
	given_back_in_order(|store| {
		let first = store.fetch(2, 100).unwrap().unwrap();
		let second = store.fetch(2, 100).unwrap().unwrap();
		first.release();
		second
	});
}

#[test]
fn test_drop_makes_items_available_again_in_order() {
	given_back_in_order(|store| {
		let first = store.fetch(2, 100).unwrap().unwrap();
		let second = store.fetch(2, 100).unwrap().unwrap();
		drop(first);
		second
	});
}

#[test]
fn test_a_panic_while_holding_items_makes_them_available_again_in_order() {
	given_back_in_order(|store| {
		let mut second = None;
		let outcome = catch_unwind(AssertUnwindSafe(|| {
			let first = store.fetch(2, 100).unwrap().unwrap();
			second = store.fetch(2, 100).unwrap();
			assert_eq!(first.values(), tagged(0..2));
			panic!("the send failed");
		}));
		assert!(outcome.is_err());
		second.unwrap()
	});
}

#[test]
fn test_a_panic_with_several_items_held_releases_them_all() {
	let (_dir, path, mut store, hook) = holding(6);
	let bytes = bytes_of(&path);
	let kept = store.fetch(2, 100).unwrap().unwrap();
	let outcome = catch_unwind(AssertUnwindSafe(|| {
		let _a = store.fetch(2, 100).unwrap().unwrap();
		let _b = store.fetch(2, 100).unwrap().unwrap();
		panic!("boom");
	}));
	assert!(outcome.is_err());
	assert_eq!(available(&mut store), tagged(2..6));
	assert!(drain(&hook).is_empty());
	assert_eq!(bytes_of(&path), bytes);
	store.remove(kept).unwrap();
	assert_eq!(stored(&mut store), tagged(2..6));
}

#[test]
fn test_pending_does_not_survive_reopening() {
	let (_dir, path, mut store, _) = holding(5);
	let held = store.fetch(3, 100).unwrap().unwrap();
	assert_eq!(available(&mut store), tagged(3..5));
	drop(store);
	// The next run sees every item, the fetched ones included.
	let mut next = super::store(&path, 100);
	assert!(next.has_data());
	assert_eq!(available(&mut next), tagged(0..5));
	// The old run's Items is another store's: removing it deletes nothing.
	next.remove(held).unwrap();
	assert_eq!(stored(&mut next), tagged(0..5));
	assert_eq!(available(&mut next), tagged(0..5));
}

#[test]
fn test_items_outliving_their_store_release_harmlessly() {
	let (_dir, path, mut store, _) = holding(3);
	let released = store.fetch(1, 100).unwrap().unwrap();
	let dropped = store.fetch(1, 100).unwrap().unwrap();
	drop(store);
	released.release();
	drop(dropped);
	assert_eq!(reopened(&path), tagged(0..3));
}
