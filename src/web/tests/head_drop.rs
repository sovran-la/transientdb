//! Items persisted before the ceiling applied, too large for any batch, are dropped by
//! `fetch` only when they reach the head of the queue.

use super::support::*;
use super::*;

const SMALL: usize = 200;
const LARGE: usize = 4096;

fn oversized(tag: usize) -> Value {
	item_of_len(500, tag)
}

fn values(store: &WebStore) -> Vec<Value> {
	store.items.iter().map(|e| e.value.clone()).collect()
}

/// Puts items in the queue the way hydration does, bypassing `append`'s check.
/// Keys are far above anything a test database holds, so their deletes are no-ops.
fn inject(store: &mut WebStore, items: &[Value]) {
	for (i, value) in items.iter().enumerate() {
		store.items.push_back(StoredEvent {
			key: 1_000_000 + i as u32,
			value: value.clone(),
		});
	}
}

#[wasm_bindgen_test]
async fn test_fetch_drops_oversized_head_and_returns_items_behind() {
	let mut store = fresh_store("test-head-drop-seam-head", SMALL).await;
	let (a, b) = (json!("a"), json!("b"));
	inject(
		&mut store,
		&[
			oversized(0),
			oversized(1),
			oversized(2),
			a.clone(),
			b.clone(),
		],
	);

	let result = store.fetch(None, None).unwrap().unwrap();
	assert_eq!(checked_batch(&result, SMALL), vec![a.clone(), b.clone()]);
	// All three oversized items went in one fetch; the fetched ones are still queued.
	assert_eq!(values(&store), vec![a, b]);
}

#[wasm_bindgen_test]
async fn test_oversized_item_behind_normal_ones_waits_for_the_head() {
	let mut store = fresh_store("test-head-drop-seam-deep", SMALL).await;
	let (a, b, c) = (json!("a"), json!("b"), json!("c"));
	inject(&mut store, &[a.clone(), b.clone(), oversized(0), c.clone()]);

	let result = store.fetch(None, None).unwrap().unwrap();
	assert_eq!(checked_batch(&result, SMALL), vec![a.clone(), b.clone()]);
	assert_eq!(values(&store), vec![a, b, oversized(0), c.clone()]);
	store.remove(&result.removable.unwrap()).unwrap();
	assert_eq!(values(&store), vec![oversized(0), c.clone()]);

	let result = store.fetch(None, None).unwrap().unwrap();
	assert_eq!(checked_batch(&result, SMALL), vec![c.clone()]);
	assert_eq!(values(&store), vec![c]);
}

#[wasm_bindgen_test]
async fn test_small_per_call_max_bytes_drops_nothing() {
	let mut store = fresh_store("test-head-drop-seam-budget", SMALL).await;
	// Fits the configured limit, not the caller's budget.
	let head = item_of_len(100, 0);
	inject(&mut store, &[head.clone(), json!("a")]);

	assert!(store
		.fetch(None, Some(envelope(KEY) + 50))
		.unwrap()
		.is_none());
	assert_eq!(values(&store), vec![head.clone(), json!("a")]);
	let result = store.fetch(None, None).unwrap().unwrap();
	assert_eq!(checked_batch(&result, SMALL)[0], head);
}

#[wasm_bindgen_test]
async fn test_queue_of_only_oversized_items_ends_empty() {
	let mut store = fresh_store("test-head-drop-seam-empty", SMALL).await;
	inject(&mut store, &[oversized(0), oversized(1)]);
	assert!(store.has_data());

	assert!(store.fetch(None, None).unwrap().is_none());
	assert!(!store.has_data());
	assert!(store.items.is_empty());
}

#[wasm_bindgen_test]
async fn test_item_at_the_limit_is_not_dropped() {
	let mut store = fresh_store("test-head-drop-seam-limit", SMALL).await;
	let largest = item_of_len(store.max_item_size, 0);
	inject(&mut store, &[largest.clone(), oversized(1)]);

	let result = store.fetch(None, None).unwrap().unwrap();
	assert_eq!(checked_batch(&result, SMALL), vec![largest.clone()]);
	assert_eq!(values(&store), vec![largest, oversized(1)]);
}

/// Writes `items` through a store with a large limit, then waits for IndexedDB.
/// Returns false, after logging, when IndexedDB is unavailable.
async fn persist_with_large_limit(db_name: &str, items: &[Value]) -> bool {
	let mut store = fresh_store(db_name, LARGE).await;
	if !store.is_persisted() {
		web_sys::console::log_1(&"Skipping head-drop persistence test - no persistence".into());
		return false;
	}
	for item in items {
		store.append(item.clone()).unwrap();
	}
	settle().await;
	true
}

#[wasm_bindgen_test]
async fn test_persisted_oversized_items_dropped_at_head_only() {
	let db_name = "test-head-drop-persisted";
	let (a, b, c) = (json!("a"), json!("b"), json!("c"));
	let items = [
		oversized(0),
		oversized(1),
		a.clone(),
		b.clone(),
		oversized(2),
		c.clone(),
	];
	if !persist_with_large_limit(db_name, &items).await {
		return;
	}

	let mut store = WebStore::new(config(db_name, SMALL, with_source(KEY))).await;
	// Hydration keeps everything; nothing is dropped before a fetch.
	assert!(store.has_data());
	assert_eq!(values(&store), items.to_vec());

	// One fetch drops both leading oversized items and returns those behind them.
	let result = store.fetch(None, None).unwrap().unwrap();
	assert_eq!(checked_batch(&result, SMALL), vec![a.clone(), b.clone()]);
	store.remove(&result.removable.unwrap()).unwrap();
	// The deeper oversized item is untouched until it reaches the head.
	assert_eq!(values(&store), vec![oversized(2), c.clone()]);
	settle().await;

	// The drops reached IndexedDB: a new instance no longer sees them.
	assert_eq!(
		hydrated(db_name, SMALL).await,
		vec![oversized(2), c.clone()]
	);

	let result = store.fetch(None, None).unwrap().unwrap();
	assert_eq!(checked_batch(&result, SMALL), vec![c]);
	store.remove(&result.removable.unwrap()).unwrap();
	assert!(!store.has_data());
	settle().await;
	assert!(hydrated(db_name, SMALL).await.is_empty());
}

#[wasm_bindgen_test]
async fn test_persisted_small_budget_deletes_nothing() {
	let db_name = "test-head-drop-persisted-budget";
	let head = item_of_len(100, 0);
	if !persist_with_large_limit(db_name, &[head.clone(), json!("a")]).await {
		return;
	}

	let mut store = WebStore::new(config(db_name, SMALL, with_source(KEY))).await;
	assert!(store
		.fetch(None, Some(envelope(KEY) + 50))
		.unwrap()
		.is_none());
	settle().await;
	assert_eq!(hydrated(db_name, SMALL).await, vec![head, json!("a")]);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_persisted_queue_of_only_oversized_items_ends_empty() {
	let db_name = "test-head-drop-persisted-empty";
	if !persist_with_large_limit(db_name, &[oversized(0), oversized(1)]).await {
		return;
	}

	let mut store = WebStore::new(config(db_name, SMALL, with_source(KEY))).await;
	assert!(store.has_data());
	assert!(store.fetch(None, None).unwrap().is_none());
	assert!(!store.has_data());
	settle().await;
	assert!(hydrated(db_name, SMALL).await.is_empty());
}
