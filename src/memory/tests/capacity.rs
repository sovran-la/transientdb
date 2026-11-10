//! `max_items`: the store keeps the newest `max_items` items, and only removal, a full
//! store or a reset ever deletes one.

use super::*;
use contract::{append_all, drain, tagged};
use std::collections::VecDeque;

#[test]
fn test_cap_of_one_holds_only_the_newest_item() {
	let mut store = store(1024, 1);
	for item in tagged(0..5) {
		store.append(item.clone()).unwrap();
		assert_eq!(queued(&store), vec![item]);
	}
	assert_eq!(drain(&mut store, 10, 1024).concat(), tagged(4..5));
}

#[test]
fn test_below_the_cap_nothing_is_dropped() {
	let mut store = store(1024, 5000);
	let appended = tagged(0..5000);
	append_all(&mut store, &appended);
	assert_eq!(store.queue.items.len(), 5000);
	let batches = drain(&mut store, 100, 64 * 1024);
	assert_eq!(batches.len(), 50);
	assert_eq!(batches.concat(), appended);
}

#[test]
fn test_append_remove_cycles_keep_counters_and_contents() {
	let mut store = store(1024, 100_000);
	let mut expected: VecDeque<Value> = VecDeque::new();
	let mut next = 0;
	for cycle in 0..50 {
		let items = tagged(next..next + 100);
		next += 100;
		append_all(&mut store, &items);
		expected.extend(items);

		// Remove fewer than were appended, so the store keeps growing.
		let fetched = store.fetch(60 + cycle % 7, 64 * 1024).unwrap().unwrap();
		let oldest: Vec<Value> = expected.drain(..fetched.len()).collect();
		assert_eq!(fetched.values(), oldest);
		store.remove(fetched).unwrap();

		assert_eq!(queued(&store), Vec::from(expected.clone()));
		assert_eq!(store.queue.next_seq, next as u64 + 1);
		let seqs: Vec<u64> = store.queue.items.iter().map(|item| item.seq).collect();
		assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
	}
	assert!(
		store.queue.items.len() > 1000,
		"the store should have grown"
	);
	assert_eq!(
		drain(&mut store, 1000, 1 << 20).concat(),
		Vec::from(expected)
	);
}

#[test]
fn test_dropping_for_capacity_forgets_the_pending_mark() {
	let mut store = store(1024, 2);
	append_all(&mut store, &tagged(0..2));
	let held = store.fetch(2, 1024).unwrap().unwrap();
	assert_eq!(store.queue.ledger.lock().pending_count(), 2);
	append_all(&mut store, &tagged(2..4));
	// Both pending items were dropped; nothing is left marked for them.
	assert_eq!(store.queue.ledger.lock().pending_count(), 0);
	assert_eq!(queued(&store), tagged(2..4));
	assert!(store.has_data());
	drop(held);
	assert_eq!(drain(&mut store, 10, 1024).concat(), tagged(2..4));
}
