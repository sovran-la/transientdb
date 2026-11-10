//! Hydration is WebStore's recovery step: reopening any number of times, and repeating a
//! fetch, never changes or duplicates what is stored.

use super::support::*;
use super::*;

#[wasm_bindgen_test]
async fn test_repeated_reopening_changes_nothing() {
	let db_name = "test-reopen-idempotent";
	let mut store = fresh_store(db_name, 4096).await;
	if !store.is_persisted() {
		web_sys::console::log_1(&"Skipping reopen test - no persistence".into());
		return;
	}
	let appended: Vec<Value> = (0..4).map(|i| json!({"i": i})).collect();
	for item in &appended {
		store.append(item.clone()).unwrap();
	}
	let keys: Vec<u32> = store.items.iter().map(|e| e.key).collect();
	settle().await;

	for _ in 0..4 {
		let reopened = WebStore::new(config(db_name, 4096, with_source(KEY))).await;
		assert_eq!(queued(&reopened), appended);
		assert_eq!(
			reopened.items.iter().map(|e| e.key).collect::<Vec<_>>(),
			keys
		);
		let db = reopened.db.clone().unwrap();
		assert_eq!(idb::read_all(&db).await.unwrap().len(), appended.len());
	}
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_repeated_fetches_change_nothing() {
	let db_name = "test-reopen-repeat-fetch";
	let mut store = fresh_store(db_name, 4096).await;
	let appended: Vec<Value> = (0..5).map(|tag| item_of_len(20, tag)).collect();
	for item in &appended {
		store.append(item.clone()).unwrap();
	}

	for _ in 0..4 {
		let result = store.fetch(None, None).unwrap().unwrap();
		assert_eq!(checked_batch(&result, 4096), appended);
		assert_eq!(queued(&store), appended);
	}
	if store.is_persisted() {
		settle().await;
		assert_eq!(hydrated(db_name, 4096).await, appended);
	}
	assert_eq!(drain(&mut store, None, None).concat(), appended);
	settle().await;
}
