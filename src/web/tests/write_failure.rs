//! A failed IndexedDB write disturbs neither the in-memory queue nor any other record,
//! and later writes still persist. Each write is its own transaction, so a failure
//! never leaves a partial record.

use super::*;
use contract::{append_all, tagged};

#[wasm_bindgen_test]
async fn test_failed_write_leaves_memory_and_other_records_intact() {
	let db_name = "test-write-failure";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let before = tagged(0..3);
	append_all(&mut store, &before);
	settle().await;

	// Occupy the sequence number the next append will be stored under, so its add fails
	// with a constraint error and its transaction aborts.
	let blocker = json!("already stored under this key");
	let db = store.db();
	let next_seq = store.queue().next_seq;
	let occupied = idb::add(&db, store.tab(), next_seq, &blocker.to_string()).unwrap();
	idb::completion(&occupied).await.unwrap();

	let failed = json!({"i": "fails to persist"});
	store.append(failed.clone()).unwrap();
	let after = tagged(3..6);
	append_all(&mut store, &after);
	settle().await;

	// Memory is the source of truth and is unaffected: every append is queued in order.
	let all = [before.clone(), vec![failed], after.clone()].concat();
	assert_eq!(queued(&store), all);
	let fetched = store.fetch(100, 4096).unwrap().unwrap();
	assert_eq!(fetched.values(), all);

	// IndexedDB keeps every other record, and the writes after the failure persisted.
	assert_eq!(
		persisted_values(&store).await,
		[before.clone(), vec![blocker.clone()], after.clone()].concat()
	);

	// Removing items, the failed one included, still works in both places. Removing
	// the failed item deletes its key, the one the blocking record was put under.
	store.remove(fetched).unwrap();
	assert!(!store.has_data());
	assert!(records(&store).await.is_empty());
}
