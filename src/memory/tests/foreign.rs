//! `remove` acts only on an `Items` this store instance issued. Another instance's, with
//! the same sequence numbers, is ignored, which is not an error, and goes back to the
//! store that issued it.

use super::*;
use contract::{append_all, available, tagged};

fn store_holding(n: usize) -> MemoryStore {
	let mut store = store(1024, 100);
	append_all(&mut store, &tagged(0..n));
	store
}

#[test]
fn test_another_stores_items_remove_nothing() {
	// Same config and same appends: the same sequence numbers in both.
	let (mut a, mut b) = (store_holding(5), store_holding(5));
	let from_a = a.fetch(3, 1024).unwrap().unwrap();
	let from_b = b.fetch(5, 1024).unwrap().unwrap();
	assert_eq!(from_a.seqs(), vec![1, 2, 3]);
	assert_eq!(from_b.seqs(), vec![1, 2, 3, 4, 5]);

	b.remove(from_a).unwrap();
	assert_eq!(queued(&b), tagged(0..5));
	// From b's point of view nothing changed: its own fetch is still pending.
	assert!(!b.has_data());
	// a got its items back, released.
	assert_eq!(queued(&a), tagged(0..5));
	assert_eq!(available(&mut a), tagged(0..5));

	a.remove(from_b).unwrap();
	assert_eq!(queued(&a), tagged(0..5));
	assert_eq!(available(&mut b), tagged(0..5));

	// Each store's own Items still removes exactly what it fetched.
	let from_a = a.fetch(3, 1024).unwrap().unwrap();
	a.remove(from_a).unwrap();
	assert_eq!(queued(&a), tagged(3..5));
	assert_eq!(queued(&b), tagged(0..5));
}

#[test]
fn test_every_instance_has_its_own_ledger() {
	let stores: Vec<MemoryStore> = (0..10).map(|_| store(64, 8)).collect();
	for (i, a) in stores.iter().enumerate() {
		for (j, b) in stores.iter().enumerate() {
			assert_eq!(a.queue.ledger.same(&b.queue.ledger), i == j);
		}
	}
}

#[test]
fn test_items_outliving_their_store_release_harmlessly() {
	let mut store = store_holding(3);
	let items = store.fetch(2, 1024).unwrap().unwrap();
	drop(store);
	assert_eq!(items.values(), tagged(0..2));
	items.release();

	let mut store = store_holding(3);
	let items = store.fetch(2, 1024).unwrap().unwrap();
	drop(store);
	drop(items);
}

#[test]
fn test_items_removed_from_a_store_made_after_the_first_is_dropped() {
	let mut first = store_holding(3);
	let items = first.fetch(3, 1024).unwrap().unwrap();
	drop(first);
	let mut second = store_holding(3);
	second.remove(items).unwrap();
	assert_eq!(available(&mut second), tagged(0..3));
}
