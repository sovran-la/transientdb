//! Items too large for any batch are rejected at append, before any IndexedDB write.

use super::support::*;
use super::*;

#[wasm_bindgen_test]
async fn test_oversized_append_leaves_store_unchanged() {
	let mut store = fresh_store("test-oversized-unchanged", 100).await;
	assert_invalid_input(&store.append(item_of_len(500, 0)).unwrap_err());
	assert!(!store.has_data());
	assert!(store.fetch(None, None).unwrap().is_none());

	store.append(json!("a")).unwrap();
	store.append(json!("b")).unwrap();
	let before: Vec<(u32, Value)> = store
		.items
		.iter()
		.map(|e| (e.key, e.value.clone()))
		.collect();
	let next_key = store.next_key;
	assert_invalid_input(&store.append(item_of_len(500, 1)).unwrap_err());
	let after: Vec<(u32, Value)> = store
		.items
		.iter()
		.map(|e| (e.key, e.value.clone()))
		.collect();
	assert_eq!(after, before);
	assert_eq!(store.next_key, next_key);
	assert!(store.has_data());
}

#[wasm_bindgen_test]
async fn test_oversized_error_message_states_sizes() {
	let mut store = fresh_store("test-oversized-message", 100).await;
	let largest = store.max_item_size;
	let message = store.append(item_of_len(1234, 0)).unwrap_err().to_string();
	assert!(message.contains("1234 bytes"), "{message}");
	assert!(message.contains(&format!("{largest} bytes")), "{message}");
	assert!(message.contains("max_fetch_size = 100 bytes"), "{message}");
}

#[wasm_bindgen_test]
async fn test_largest_item_accepted_and_fetchable_alone() {
	for max in [100usize, 101, 512, 4096] {
		let mut store = fresh_store(&format!("test-oversized-largest-{max}"), max).await;
		let largest = store.max_item_size;
		assert_eq!(largest, max - envelope(KEY));

		assert_invalid_input(&store.append(item_of_len(largest + 1, 0)).unwrap_err());
		store.append(item_of_len(largest, 1)).unwrap();
		store.append(item_of_len(10, 2)).unwrap();
		assert_invalid_input(&store.append(item_of_len(largest + 1, 3)).unwrap_err());

		let batches = drain(&mut store, None, None);
		assert_eq!(batches[0], vec![item_of_len(largest, 1)], "max={max}");
		assert_eq!(batches.concat().len(), 2);
	}
}

#[wasm_bindgen_test]
async fn test_rejections_mixed_with_accepted_items_never_wedge() {
	let mut store = fresh_store("test-oversized-mixed", 150).await;
	let mut accepted = Vec::new();
	for i in 0..60 {
		if i % 3 == 1 {
			let item = json!({"rejected": "r".repeat(70 + i)});
			assert_invalid_input(&store.append(item).unwrap_err());
		} else {
			let item = json!({"i": i, "pad": "p".repeat(i % 30)});
			store.append(item.clone()).unwrap();
			accepted.push(item);
		}
	}

	let items = drain(&mut store, None, None).concat();
	assert_eq!(items, accepted);
	assert!(items.iter().all(|item| item.get("rejected").is_none()));
}

#[wasm_bindgen_test]
async fn test_small_per_call_max_bytes_returns_none_without_wedging() {
	let mut store = fresh_store("test-oversized-small-budget", 300).await;
	let head = item_of_len(150, 0);
	store.append(head.clone()).unwrap();
	store.append(item_of_len(10, 1)).unwrap();

	// The caller's budget is too small for the head item: nothing, and nothing lost.
	assert!(store.fetch(None, Some(150)).unwrap().is_none());
	assert!(store.has_data());
	assert_eq!(store.items.len(), 2);

	// The configured limit always fits it.
	let result = store.fetch(None, None).unwrap().unwrap();
	assert_eq!(checked_batch(&result, 300)[0], head);
}

#[wasm_bindgen_test]
async fn test_rejected_item_does_not_trigger_drop_oldest() {
	let mut config = config("test-oversized-max-items", 200, with_source(KEY));
	config.max_items = Some(3);
	let mut store = fresh_store_with(config).await;
	for tag in 0..3 {
		store.append(item_of_len(20, tag)).unwrap();
	}
	assert_invalid_input(&store.append(item_of_len(500, 9)).unwrap_err());
	let items = drain(&mut store, None, None).concat();
	assert_eq!(
		items,
		(0..3).map(|tag| item_of_len(20, tag)).collect::<Vec<_>>()
	);
}

#[wasm_bindgen_test]
async fn test_rejected_item_is_never_persisted() {
	let db_name = "test-oversized-not-persisted";
	let mut store = fresh_store(db_name, 200).await;
	if !store.is_persisted() {
		web_sys::console::log_1(&"Skipping persistence check - no persistence".into());
		return;
	}
	store.append(json!("a")).unwrap();
	assert_invalid_input(&store.append(item_of_len(500, 0)).unwrap_err());
	store.append(json!("b")).unwrap();
	settle().await;

	assert_eq!(hydrated(db_name, 200).await, vec![json!("a"), json!("b")]);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
#[should_panic(
	expected = "max_fetch_size is all envelope and no room for an item? That's packaging, not a batch."
)]
async fn test_rejects_envelope_that_leaves_no_room() {
	// The envelope with this source is exactly 100 bytes.
	let source = "k".repeat(100 - envelope(""));
	let _store = WebStore::new(config("test-panic", 100, with_source(&source))).await;
}

#[wasm_bindgen_test]
async fn test_envelope_leaving_one_byte_is_accepted() {
	let source = "k".repeat(99 - envelope(""));
	let mut store =
		fresh_store_with(config("test-oversized-one-byte", 100, with_source(&source))).await;
	assert_eq!(store.max_item_size, 1);
	assert_invalid_input(&store.append(json!(10)).unwrap_err());
	store.append(json!(7)).unwrap();
	assert_eq!(drain(&mut store, None, None).concat(), vec![json!(7)]);
}
