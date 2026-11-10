//! `max_items`: `None` never drops an item for capacity; `Some(n)` keeps the newest `n`,
//! deleting the rest from IndexedDB. Hydration loads every persisted item whatever the
//! cap; a store opened over more than `n` items trims them on its next append.

use super::support::*;
use super::*;
use std::collections::VecDeque;

fn capped(db_name: &str, max_items: Option<usize>) -> WebConfig {
	let mut config = config(db_name, 8 * 1024, with_source(KEY));
	config.max_items = max_items;
	config
}

fn tagged(range: std::ops::Range<usize>) -> Vec<Value> {
	range.map(|i| json!({"i": i})).collect()
}

fn append_all(store: &mut WebStore, items: &[Value]) {
	for item in items {
		store.append(item.clone()).unwrap();
	}
}

/// The values `store`'s IndexedDB holds, in key order, once every write it issued has
/// landed: a read issued after them on the same connection runs after them.
async fn flushed(store: &WebStore) -> Vec<Value> {
	let records = idb::read_all(store.db.as_ref().unwrap()).await.unwrap();
	records
		.iter()
		.map(|record| idb::record_value(record).unwrap())
		.collect()
}

/// A store on `db_name` emptied of anything a previous run left, or `None`, after
/// logging, when IndexedDB is unavailable.
async fn persisted(config: WebConfig) -> Option<WebStore> {
	let store = fresh_store_with(config).await;
	if !store.is_persisted() {
		web_sys::console::log_1(&"Skipping capacity persistence test - no persistence".into());
		return None;
	}
	Some(store)
}

#[wasm_bindgen_test]
async fn test_unbounded_never_drops() {
	let db_name = "test-capacity-unbounded";
	let mut store = fresh_store_with(capped(db_name, None)).await;
	let appended = tagged(0..5000);
	append_all(&mut store, &appended);
	assert_eq!(store.items.len(), 5000);

	let batches = drain(&mut store, None, None);
	assert!(batches.len() > 1, "expected several batches");
	assert_eq!(batches.concat(), appended);
	if store.is_persisted() {
		assert!(flushed(&store).await.is_empty());
	}
}

#[wasm_bindgen_test]
async fn test_unbounded_batches_still_limited() {
	let db_name = "test-capacity-unbounded-limits";
	let mut config = capped(db_name, None);
	// Room for 5 items of 20 bytes and their separators, not 6.
	config.max_fetch_size = envelope(KEY) + 5 * 20 + 4;
	config.max_fetch_items = Some(8);
	let mut store = fresh_store_with(config).await;
	let appended: Vec<Value> = (0..2000).map(|tag| item_of_len(20, tag)).collect();

	// The byte ceiling wins over the item limit; `drain` checks every batch against it.
	append_all(&mut store, &appended);
	let batches = drain(&mut store, None, None);
	assert!(batches.iter().all(|batch| batch.len() == 5));
	assert_eq!(batches.concat(), appended);

	// The item limit wins when the per-call ceiling is roomier.
	append_all(&mut store, &appended);
	let batches = drain(&mut store, None, Some(64 * 1024));
	assert_eq!(batches.len(), 250);
	assert!(batches.iter().all(|batch| batch.len() == 8));
	assert_eq!(batches.concat(), appended);
	settle().await;
}

#[wasm_bindgen_test]
async fn test_cap_of_one_holds_only_the_newest_item() {
	let db_name = "test-capacity-one";
	let mut store = fresh_store_with(capped(db_name, Some(1))).await;
	for item in tagged(0..5) {
		store.append(item.clone()).unwrap();
		assert_eq!(queued(&store), vec![item]);
	}
	if store.is_persisted() {
		assert_eq!(flushed(&store).await, tagged(4..5));
	}
	assert_eq!(drain(&mut store, None, None).concat(), tagged(4..5));
	settle().await;
}

#[wasm_bindgen_test]
async fn test_unbounded_append_remove_cycles_keep_counters_and_contents() {
	let db_name = "test-capacity-cycles";
	let mut store = fresh_store_with(capped(db_name, None)).await;
	let first_key = store.next_key;
	let mut expected: VecDeque<Value> = VecDeque::new();
	let mut next = 0;
	for cycle in 0..30 {
		let items = tagged(next..next + 100);
		next += 100;
		append_all(&mut store, &items);
		expected.extend(items);

		// Remove fewer than were appended, so the store keeps growing.
		let result = store.fetch(Some(60 + cycle % 7), None).unwrap().unwrap();
		let fetched = checked_batch(&result, 8 * 1024);
		let oldest: Vec<Value> = expected.drain(..fetched.len()).collect();
		assert_eq!(fetched, oldest);
		store.remove(&result.removable.unwrap()).unwrap();

		assert_eq!(queued(&store), Vec::from(expected.clone()));
		assert_eq!(store.next_key, first_key + next as u32);
		let keys: Vec<u32> = store.items.iter().map(|e| e.key).collect();
		assert!(keys.windows(2).all(|w| w[0] < w[1]), "{keys:?}");
	}
	assert!(store.items.len() > 1000, "the store should have grown");
	if store.is_persisted() {
		assert_eq!(flushed(&store).await, Vec::from(expected.clone()));
	}
	assert_eq!(drain(&mut store, None, None).concat(), Vec::from(expected));
	settle().await;
}

#[wasm_bindgen_test]
async fn test_unbounded_store_persists_everything() {
	let db_name = "test-capacity-unbounded-persist";
	let Some(mut store) = persisted(capped(db_name, None)).await else {
		return;
	};
	let appended = tagged(0..2000);
	append_all(&mut store, &appended);
	assert_eq!(flushed(&store).await, appended);

	let reopened = WebStore::new(capped(db_name, None)).await;
	assert!(reopened.is_persisted());
	assert_eq!(queued(&reopened), appended);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_hydration_with_a_cap_keeps_everything_until_the_next_append() {
	let db_name = "test-capacity-hydrate-capped";
	let Some(mut writer) = persisted(capped(db_name, None)).await else {
		return;
	};
	let appended = tagged(0..10);
	append_all(&mut writer, &appended);
	assert_eq!(flushed(&writer).await, appended);

	// Hydration does not apply the cap: all 10 are loaded and none is deleted.
	let mut capped_store = WebStore::new(capped(db_name, Some(3))).await;
	assert!(capped_store.is_persisted());
	assert_eq!(queued(&capped_store), appended);
	assert_eq!(flushed(&capped_store).await, appended);

	// The next append trims to the newest 3, in memory and in IndexedDB.
	capped_store.append(json!({"i": 10})).unwrap();
	let kept = tagged(8..11);
	assert_eq!(queued(&capped_store), kept);
	assert_eq!(flushed(&capped_store).await, kept);
	assert_eq!(queued(&WebStore::new(capped(db_name, None)).await), kept);
	capped_store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_hydration_without_a_cap_loads_everything() {
	let db_name = "test-capacity-hydrate-unbounded";
	let Some(mut writer) = persisted(capped(db_name, Some(1000))).await else {
		return;
	};
	let appended = tagged(0..1000);
	append_all(&mut writer, &appended);
	assert_eq!(flushed(&writer).await, appended);

	let mut unbounded = WebStore::new(capped(db_name, None)).await;
	assert!(unbounded.is_persisted());
	assert_eq!(queued(&unbounded), appended);

	// Past the old cap nothing is dropped.
	append_all(&mut unbounded, &tagged(1000..1500));
	assert_eq!(queued(&unbounded), tagged(0..1500));
	assert_eq!(flushed(&unbounded).await, tagged(0..1500));
	unbounded.reset();
	settle().await;
}
