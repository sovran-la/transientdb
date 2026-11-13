//! Items persist under the store's tab and the sequence number the store gave them, so
//! what a later instance adopts once this one is gone is exactly what this one still
//! held: any JSON value, unmodified, in append order, and every item available, whatever
//! was pending.

use super::*;
use contract::{append_all, tagged};

/// Fetches every queued item and removes those at `positions`, releasing the rest.
fn remove_positions(store: &mut WebStore, positions: &[usize]) {
	let all = store.fetch(usize::MAX, usize::MAX).unwrap().unwrap();
	let values = all.values();
	drop(all);
	for (removed, &position) in positions.iter().enumerate() {
		// Skip what comes before it (held, then released), take it, remove it.
		let before = store.fetch(position - removed, usize::MAX).unwrap();
		let target = store.fetch(1, usize::MAX).unwrap().unwrap();
		assert_eq!(target.values(), vec![values[position].clone()]);
		store.remove(target).unwrap();
		drop(before);
	}
}

#[wasm_bindgen_test]
async fn test_every_json_type_round_trips_unchanged_in_order() {
	let db_name = "test-persist-json-types";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let items = vec![
		json!(null),
		json!(true),
		json!(false),
		json!(0),
		json!(42),
		json!(-7),
		json!(42.5),
		json!(-0.125),
		json!(""),
		json!("string"),
		json!([]),
		json!([1, "two", null, [3]]),
		json!({}),
		json!({"event": "e", "n": 1}),
		json!({"a": {"b": {"c": [1, {"d": null}]}}}),
		json!({"seq": "not the record key"}),
		json!({"seq": 999_999}),
		json!({"text": "not the record text"}),
		json!({"seq": 1, "text": 2}),
		json!({"_idb_key": 5, "other": "kept"}),
	];
	append_all(&mut store, &items);
	assert_eq!(persisted_values(&store).await, items);
	drop(store);

	assert_eq!(adopted(db_name).await, items);
	fresh_store(db_name, 4096, 1000).await;
	settle().await;
}

#[wasm_bindgen_test]
async fn test_memory_and_indexeddb_hold_the_same_records() {
	let db_name = "test-persist-keys-agree";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut store, &tagged(0..5));
	// The five items have the five sequence numbers before the next one.
	let first_seq = store.queue().next_seq - 5;
	remove_positions(&mut store, &[1]);
	let expected: Vec<(u64, String)> = [0, 2, 3, 4]
		.iter()
		.map(|&i| (first_seq + i, format!("{{\"i\":{i}}}")))
		.collect();
	assert_eq!(stored(&store), expected);
	assert_eq!(flushed(&store).await, expected);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_pending_items_are_available_after_a_reload() {
	let db_name = "test-persist-pending-reload";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut store, &tagged(0..5));
	let held = store.fetch(3, 4096).unwrap().unwrap();
	assert_eq!(held.values(), tagged(0..3));
	assert_eq!(available(&mut store), tagged(3..5));
	// Fetching wrote nothing: IndexedDB holds every item as appended.
	assert_eq!(flushed(&store).await, stored(&store));

	// The page reloads: the old instance goes away with three items still pending.
	drop(store);
	let mut reloaded = WebStore::new(config(db_name, 4096, 1000)).await;
	assert!(reloaded.has_data());
	let all = reloaded.fetch(10, 4096).unwrap().unwrap();
	assert_eq!(all.values(), tagged(0..5));
	// The old instance's fetch outlives it harmlessly, and matches nothing here.
	held.release();
	reloaded.remove(all).unwrap();
	assert!(flushed(&reloaded).await.is_empty());
	reloaded.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_removed_items_stay_removed_after_reopen() {
	let db_name = "test-persist-remove-some";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let items = tagged(0..6);
	append_all(&mut store, &items);
	remove_positions(&mut store, &[1, 3, 4]);
	let expected = vec![items[0].clone(), items[2].clone(), items[5].clone()];
	assert_eq!(queued(&store), expected);
	drop(store);

	assert_eq!(adopted(db_name).await, expected);
	fresh_store(db_name, 4096, 1000).await;
	settle().await;
}

#[wasm_bindgen_test]
async fn test_append_then_immediate_remove_is_gone() {
	let db_name = "test-persist-append-remove";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	// No await anywhere between these calls: the add must not land after the delete.
	store.append(json!("short-lived")).unwrap();
	let items = store.fetch(10, 4096).unwrap().unwrap();
	store.remove(items).unwrap();
	store.append(json!("kept")).unwrap();
	store.append(json!("also short-lived")).unwrap();
	remove_positions(&mut store, &[1]);
	drop(store);

	assert_eq!(adopted(db_name).await, vec![json!("kept")]);
	fresh_store(db_name, 4096, 1000).await;
	settle().await;
}

#[wasm_bindgen_test]
async fn test_writes_are_ordered_before_the_next_instance_reads() {
	let db_name = "test-persist-no-settle";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	// No settle before the store goes and another adopts: the adopter's read is ordered
	// after every write this one issued, so the result cannot depend on timing.
	append_all(&mut store, &tagged(0..4));
	remove_positions(&mut store, &[0, 2]);
	store.append(json!({"i": 4})).unwrap();
	drop(store);

	assert_eq!(
		adopted(db_name).await,
		vec![json!({"i": 1}), json!({"i": 3}), json!({"i": 4})]
	);
	let mut store = WebStore::new(config(db_name, 4096, 1000)).await;
	store.reset();
	drop(store);
	assert!(adopted(db_name).await.is_empty());
}

#[wasm_bindgen_test]
async fn test_reset_of_this_sessions_items_persists() {
	let db_name = "test-persist-reset";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut store, &tagged(0..5));
	store.reset();
	assert!(records(&store).await.is_empty());

	// Appends right after a reset survive it.
	append_all(&mut store, &tagged(5..7));
	store.reset();
	append_all(&mut store, &tagged(7..9));
	assert_eq!(persisted_values(&store).await, tagged(7..9));
	assert_eq!(records(&store).await.len(), 2);
	drop(store);
	assert_eq!(adopted(db_name).await, tagged(7..9));
	fresh_store(db_name, 4096, 1000).await;
	settle().await;
}

#[wasm_bindgen_test]
async fn test_drop_oldest_persists_also_for_pending_items() {
	let db_name = "test-persist-drop-oldest";
	let Some(mut store) = persisted(db_name, 4096, 3).await else {
		return;
	};
	append_all(&mut store, &tagged(0..3));
	let held = store.fetch(2, 4096).unwrap().unwrap();
	append_all(&mut store, &tagged(3..8));
	assert_eq!(queued(&store), tagged(5..8));
	store.remove(held).unwrap();
	assert_eq!(flushed(&store).await, stored(&store));
	assert_eq!(persisted_values(&store).await, tagged(5..8));
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_identical_items_are_removed_one_at_a_time() {
	let db_name = "test-persist-identical";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let same = json!({"event": "click"});
	append_all(&mut store, &vec![same.clone(); 4]);

	let items = store.fetch(1, 4096).unwrap().unwrap();
	assert_eq!(items.values(), vec![same.clone()]);
	store.remove(items).unwrap();
	assert_eq!(queued(&store), vec![same.clone(); 3]);
	assert_eq!(persisted_values(&store).await, vec![same; 3]);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_items_carry_over_through_a_chain_of_reloads() {
	let db_name = "test-persist-reload-chain";
	let Some(mut first) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut first, &tagged(0..3));
	drop(first);

	let mut second = WebStore::new(config(db_name, 4096, 1000)).await;
	assert_eq!(queued(&second), tagged(0..3));
	append_all(&mut second, &tagged(3..6));
	let seqs: Vec<u64> = stored(&second).into_iter().map(|(seq, _)| seq).collect();
	assert_eq!(seqs, (1..=6).collect::<Vec<_>>());
	assert_eq!(flushed(&second).await, stored(&second));
	drop(second);

	let mut third = WebStore::new(config(db_name, 4096, 1000)).await;
	assert_eq!(queued(&third), tagged(0..6));
	append_all(&mut third, &tagged(6..8));
	drop(third);

	assert_eq!(adopted(db_name).await, tagged(0..8));
	fresh_store(db_name, 4096, 1000).await;
	settle().await;
}

#[wasm_bindgen_test]
async fn test_remove_works_on_adopted_and_new_items_together() {
	let db_name = "test-persist-mixed";
	let Some(mut first) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut first, &tagged(0..3));
	drop(first);

	let mut second = WebStore::new(config(db_name, 4096, 1000)).await;
	append_all(&mut second, &tagged(3..5));
	assert_eq!(queued(&second), tagged(0..5));
	// One adopted item, one appended this session.
	remove_positions(&mut second, &[1, 3]);
	let expected = vec![json!({"i": 0}), json!({"i": 2}), json!({"i": 4})];
	assert_eq!(queued(&second), expected);
	assert_eq!(persisted_values(&second).await, expected);
	drop(second);

	assert_eq!(adopted(db_name).await, expected);
	fresh_store(db_name, 4096, 1000).await;
	settle().await;
}

#[wasm_bindgen_test]
async fn test_memory_only_mode_keeps_working() {
	let mut store = WebStore::new(config("test-persist-memory-only", 4096, 1000)).await;
	// Simulate a closed connection: no writes are attempted.
	store.shared.closed.set(true);
	assert_eq!(store.persistence_state(), PersistenceState::MemoryOnly);
	store.reset();
	assert!(!store.is_persisted());

	let same = json!("dup");
	append_all(&mut store, &[same.clone(), same.clone(), json!("x")]);
	let items = store.fetch(1, 4096).unwrap().unwrap();
	store.remove(items).unwrap();
	assert_eq!(queued(&store), vec![same, json!("x")]);
	let held = store.fetch(10, 4096).unwrap().unwrap();
	assert!(!store.has_data());
	drop(held);
	assert!(store.has_data());
	store.reset();
	assert!(!store.has_data());
}
