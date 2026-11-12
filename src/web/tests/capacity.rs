//! `max_items` with IndexedDB: the newest `max_items` items are kept, and the rest are
//! deleted from IndexedDB too.

use super::*;
use contract::{append_all, drain, tagged};
use std::collections::VecDeque;

#[wasm_bindgen_test]
async fn test_cap_of_one_holds_only_the_newest_item() {
	let db_name = "test-capacity-one";
	let mut store = fresh_store(db_name, 1024, 1).await;
	for item in tagged(0..5) {
		store.append(item.clone()).unwrap();
		assert_eq!(queued(&store), vec![item]);
	}
	if store.is_persisted() {
		assert_eq!(flushed(&store).await, stored(&store));
	}
	assert_eq!(drain(&mut store, 10, 1024).concat(), tagged(4..5));
	settle().await;
}

#[wasm_bindgen_test]
async fn test_append_remove_cycles_keep_counters_and_contents() {
	let db_name = "test-capacity-cycles";
	let mut store = fresh_store(db_name, 1024, 100_000).await;
	let first_seq = store.queue.next_seq;
	let mut expected: VecDeque<Value> = VecDeque::new();
	let mut next = 0;
	for cycle in 0..30 {
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
		assert_eq!(store.queue.next_seq, first_seq + next as u64);
		let seqs: Vec<u64> = stored(&store).into_iter().map(|(seq, _)| seq).collect();
		assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
	}
	assert!(
		store.queue.items.len() > 1000,
		"the store should have grown"
	);
	if store.is_persisted() {
		assert_eq!(flushed(&store).await, stored(&store));
	}
	assert_eq!(
		drain(&mut store, 1000, 1 << 20).concat(),
		Vec::from(expected)
	);
	settle().await;
}

#[wasm_bindgen_test]
async fn test_a_large_store_persists_everything() {
	let db_name = "test-capacity-large";
	let Some(mut store) = persisted(db_name, 1024, 5000).await else {
		return;
	};
	let appended = tagged(0..2000);
	append_all(&mut store, &appended);
	assert_eq!(flushed(&store).await, stored(&store));

	let reopened = WebStore::new(config(db_name, 1024, 5000)).await;
	assert!(reopened.is_persisted());
	assert_eq!(queued(&reopened), appended);
	store.reset();
	settle().await;
}
