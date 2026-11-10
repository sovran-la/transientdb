//! `end_batch` sets a boundary no fetched batch crosses: a fetch returns only items
//! appended under the oldest item's batch number. Persistence across a reload is in
//! `end_batch_reload.rs`.

use super::support::*;
use super::*;

pub(super) fn tag(n: usize) -> Value {
	json!({"tag": n})
}

pub(super) fn tags(items: &[Value]) -> Vec<usize> {
	items
		.iter()
		.map(|v| v["tag"].as_u64().unwrap() as usize)
		.collect()
}

pub(super) fn append_tags(store: &mut WebStore, range: std::ops::Range<usize>) {
	for n in range {
		store.append(tag(n)).unwrap();
	}
}

pub(super) async fn store_with_limits(
	db_name: &str,
	max_items: Option<usize>,
	max_fetch_items: Option<usize>,
) -> WebStore {
	fresh_store_with(WebConfig {
		max_items,
		max_fetch_items,
		..config(db_name, 64 * 1024, with_source(KEY))
	})
	.await
}

pub(super) fn drained(
	store: &mut WebStore,
	count: Option<usize>,
	max_bytes: Option<usize>,
) -> Vec<Vec<usize>> {
	drain(store, count, max_bytes)
		.iter()
		.map(|b| tags(b))
		.collect()
}

/// The tags of one fetch, removing nothing.
fn peek(store: &mut WebStore, count: Option<usize>) -> Vec<usize> {
	let limit = store.config.max_fetch_size;
	match store.fetch(count, None).unwrap() {
		Some(result) => tags(&checked_batch(&result, limit)),
		None => Vec::new(),
	}
}

#[wasm_bindgen_test]
async fn test_items_before_and_after_never_share_a_batch() {
	let mut store = fresh_store("test-end-batch-split", 4096).await;
	append_tags(&mut store, 0..2);
	store.end_batch().unwrap();
	append_tags(&mut store, 2..4);
	assert_eq!(
		drained(&mut store, None, None),
		vec![vec![0, 1], vec![2, 3]]
	);
	settle().await;
}

#[wasm_bindgen_test]
async fn test_a_fetch_stops_at_the_boundary_until_removed() {
	let mut store = fresh_store("test-end-batch-stops", 4096).await;
	append_tags(&mut store, 0..3);
	store.end_batch().unwrap();
	append_tags(&mut store, 3..5);
	for _ in 0..3 {
		assert_eq!(peek(&mut store, Some(1000)), vec![0, 1, 2]);
	}
	let result = store.fetch(Some(1000), None).unwrap().unwrap();
	assert_eq!(result.removable.as_ref().unwrap().len(), 3);
	store.remove(&result.removable.unwrap()).unwrap();
	assert_eq!(peek(&mut store, Some(1000)), vec![3, 4]);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_several_boundaries_make_one_batch_per_span() {
	let mut store = fresh_store("test-end-batch-several", 4096).await;
	append_tags(&mut store, 0..1);
	store.end_batch().unwrap();
	append_tags(&mut store, 1..3);
	store.end_batch().unwrap();
	append_tags(&mut store, 3..6);
	store.end_batch().unwrap();
	append_tags(&mut store, 6..7);
	assert_eq!(
		drained(&mut store, None, None),
		vec![vec![0], vec![1, 2], vec![3, 4, 5], vec![6]]
	);
	settle().await;
}

#[wasm_bindgen_test]
async fn test_nothing_to_end_changes_nothing() {
	let mut store = fresh_store("test-end-batch-nothing", 4096).await;
	let start = store.current_batch;
	store.end_batch().unwrap();
	store.end_batch().unwrap();
	assert_eq!(store.current_batch, start);
	assert!(!store.has_data());
	assert!(store.fetch(None, None).unwrap().is_none());

	append_tags(&mut store, 0..2);
	drain(&mut store, None, None);
	let current = store.current_batch;
	store.end_batch().unwrap();
	store.end_batch().unwrap();
	assert_eq!(store.current_batch, current);
	assert!(!store.has_data());

	append_tags(&mut store, 2..4);
	store.reset();
	let current = store.current_batch;
	store.end_batch().unwrap();
	store.end_batch().unwrap();
	assert_eq!(store.current_batch, current);
	assert!(!store.has_data());

	append_tags(&mut store, 4..7);
	assert_eq!(drained(&mut store, None, None), vec![vec![4, 5, 6]]);
	settle().await;
}

#[wasm_bindgen_test]
async fn test_repeated_end_batch_is_one_boundary() {
	let mut store = fresh_store("test-end-batch-repeated", 4096).await;
	append_tags(&mut store, 0..2);
	store.end_batch().unwrap();
	let after_first = store.current_batch;
	for _ in 0..3 {
		store.end_batch().unwrap();
	}
	assert_eq!(store.current_batch, after_first);
	append_tags(&mut store, 2..3);
	store.end_batch().unwrap();
	store.end_batch().unwrap();
	assert_eq!(store.current_batch, after_first + 1);
	assert_eq!(drained(&mut store, None, None), vec![vec![0, 1], vec![2]]);
	settle().await;
}

#[wasm_bindgen_test]
async fn test_drop_oldest_keeps_batch_numbers() {
	let mut store = store_with_limits("test-end-batch-drop-oldest", Some(4), None).await;
	append_tags(&mut store, 0..3);
	store.end_batch().unwrap();
	append_tags(&mut store, 3..6);
	assert_eq!(tags(&queued(&store)), vec![2, 3, 4, 5]);
	assert_eq!(
		drained(&mut store, None, None),
		vec![vec![2], vec![3, 4, 5]]
	);

	append_tags(&mut store, 6..7);
	store.end_batch().unwrap();
	append_tags(&mut store, 7..11);
	assert_eq!(drained(&mut store, None, None), vec![vec![7, 8, 9, 10]]);
	settle().await;
}

#[wasm_bindgen_test]
async fn test_boundary_does_not_leak_across_reset() {
	let mut store = fresh_store("test-end-batch-reset", 4096).await;
	append_tags(&mut store, 0..1);
	store.end_batch().unwrap();
	append_tags(&mut store, 1..2);
	store.reset();
	append_tags(&mut store, 2..4);
	store.end_batch().unwrap();
	append_tags(&mut store, 4..5);
	assert_eq!(drained(&mut store, None, None), vec![vec![2, 3], vec![4]]);

	append_tags(&mut store, 5..6);
	store.end_batch().unwrap();
	store.reset();
	append_tags(&mut store, 6..8);
	assert_eq!(drained(&mut store, None, None), vec![vec![6, 7]]);
	settle().await;
}

#[wasm_bindgen_test]
async fn test_batch_numbers_never_wrap() {
	let mut store = fresh_store("test-end-batch-exhausted", 4096).await;
	store.current_batch = u32::MAX - 1;
	append_tags(&mut store, 0..1);
	store.end_batch().unwrap();
	assert_eq!(store.current_batch, u32::MAX);
	append_tags(&mut store, 1..2);
	for _ in 0..3 {
		let err = store.end_batch().unwrap_err();
		assert!(err.to_string().contains("batch numbers exhausted"), "{err}");
		assert_eq!(store.current_batch, u32::MAX);
	}
	append_tags(&mut store, 2..3);
	assert_eq!(
		store.items.iter().map(|e| e.batch).collect::<Vec<_>>(),
		vec![u32::MAX - 1, u32::MAX, u32::MAX]
	);
	assert_eq!(drained(&mut store, None, None), vec![vec![0], vec![1, 2]]);
	settle().await;
}

/// Puts an item in the queue under `batch`, bypassing `append`'s size check, the way
/// hydration does. Keys are far above anything the test database holds.
fn inject(store: &mut WebStore, key: u32, batch: u32, value: Value) {
	store.items.push_back(StoredEvent {
		key: 1_000_000 + key,
		batch,
		value,
	});
}

#[wasm_bindgen_test]
async fn test_head_drop_next_to_a_boundary() {
	let small = 200;
	let mut store = fresh_store("test-end-batch-head-drop", small).await;
	let oversized = |n| item_of_len(500, n);
	let (a, b, c) = (json!("a"), json!("b"), json!("c"));

	// Oversized head inside the first span: the rest of that span comes back.
	inject(&mut store, 0, 0, oversized(0));
	inject(&mut store, 1, 0, a.clone());
	inject(&mut store, 2, 1, b.clone());
	assert_eq!(
		drain(&mut store, None, None),
		vec![vec![a.clone()], vec![b.clone()]]
	);

	// Oversized item alone before a boundary: the new head's batch governs.
	inject(&mut store, 3, 0, oversized(1));
	inject(&mut store, 4, 1, a.clone());
	inject(&mut store, 5, 1, b.clone());
	inject(&mut store, 6, 2, c.clone());
	let result = store.fetch(None, None).unwrap().unwrap();
	assert_eq!(checked_batch(&result, small), vec![a.clone(), b.clone()]);
	store.remove(&result.removable.unwrap()).unwrap();
	assert_eq!(drain(&mut store, None, None), vec![vec![c.clone()]]);

	// Oversized item just after a boundary: dropped once it reaches the head.
	inject(&mut store, 7, 0, a.clone());
	inject(&mut store, 8, 1, oversized(2));
	inject(&mut store, 9, 1, b.clone());
	inject(&mut store, 10, 2, c.clone());
	assert_eq!(
		drain(&mut store, None, None),
		vec![vec![a], vec![b], vec![c]]
	);
	settle().await;
}
