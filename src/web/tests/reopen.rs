//! Adoption is WebStore's recovery step: reopening any number of times, and repeating a
//! fetch, never changes or duplicates what is stored.

use super::*;
use contract::{append_all, drain, item_of_len, tagged};

#[wasm_bindgen_test]
async fn test_repeated_reopening_changes_nothing() {
	let db_name = "test-reopen-idempotent";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut store, &tagged(0..4));
	drop(store);

	// Each reopened store adopts the last one's items, and leaves them when it goes:
	// never more, never fewer, never another order.
	for _ in 0..4 {
		let reopened = WebStore::new(config(db_name, 4096, 1000)).await;
		assert_eq!(queued(&reopened), tagged(0..4));
		assert_eq!(flushed(&reopened).await, stored(&reopened));
		assert_eq!(records(&reopened).await.len(), 4);
	}
	fresh_store(db_name, 4096, 1000).await;
	settle().await;
}

#[wasm_bindgen_test]
async fn test_fetching_and_releasing_writes_nothing() {
	let db_name = "test-reopen-repeat-fetch";
	let mut store = fresh_store(db_name, 4096, 1000).await;
	let appended: Vec<Value> = (0..5).map(|tag| item_of_len(20, tag)).collect();
	append_all(&mut store, &appended);

	for _ in 0..4 {
		let fetched = store.fetch(100, 4096).unwrap().unwrap();
		assert_eq!(fetched.values(), appended);
		assert_eq!(queued(&store), appended);
		fetched.release();
	}
	if store.is_persisted() {
		assert_eq!(flushed(&store).await, stored(&store));
		assert_eq!(persisted_values(&store).await, appended);
	}
	assert_eq!(drain(&mut store, 100, 4096).concat(), appended);
	settle().await;
}
