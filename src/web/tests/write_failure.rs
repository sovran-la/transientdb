//! A failed IndexedDB write disturbs neither the in-memory queue nor any other record,
//! and later writes still persist. Each write is its own transaction, so a failure
//! never leaves a partial record.

use super::support::*;
use super::*;

fn tagged(range: std::ops::Range<i64>) -> Vec<Value> {
	range.map(|i| json!({"i": i})).collect()
}

#[wasm_bindgen_test]
async fn test_failed_write_leaves_memory_and_other_records_intact() {
	let db_name = "test-write-failure";
	let mut store = fresh_store(db_name, 4096).await;
	if !store.is_persisted() {
		web_sys::console::log_1(&"Skipping write failure test - no persistence".into());
		return;
	}
	let before = tagged(0..3);
	for item in &before {
		store.append(item.clone()).unwrap();
	}
	settle().await;

	// Occupy the key the next append will be stored under, so its add fails with a
	// constraint error and its transaction aborts.
	let blocker = json!("already stored under this key");
	let db = store.db.clone().unwrap();
	let occupied = idb::add(&db, store.next_key, &blocker).unwrap();
	idb::completion(&occupied).await.unwrap();

	let failed = json!({"i": "fails to persist"});
	store.append(failed.clone()).unwrap();
	let after = tagged(3..6);
	for item in &after {
		store.append(item.clone()).unwrap();
	}
	settle().await;

	// Memory is the source of truth and is unaffected: every append is queued in order.
	let all = [before.clone(), vec![failed], after.clone()].concat();
	assert_eq!(queued(&store), all);
	let result = store.fetch(None, None).unwrap().unwrap();
	assert_eq!(checked_batch(&result, 4096), all);

	// IndexedDB keeps every other record, and the writes after the failure persisted.
	assert_eq!(
		hydrated(db_name, 4096).await,
		[before.clone(), vec![blocker.clone()], after.clone()].concat()
	);

	// Removing items, the failed one included, still works in both places. Removing
	// the failed item deletes its key, the one the blocking record was put under.
	store.remove(&result.removable.unwrap()).unwrap();
	settle().await;
	assert!(!store.has_data());
	assert!(hydrated(db_name, 4096).await.is_empty());
}
