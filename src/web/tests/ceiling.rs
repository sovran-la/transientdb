//! A fetched batch, serialized compactly, never exceeds the byte limit in effect.

use super::support::*;
use super::*;

#[wasm_bindgen_test]
async fn test_ceiling_holds_for_mixed_sizes_and_drains_in_order() {
	for max in [100usize, 150, 256, 1000, 4096] {
		let mut store = fresh_store(&format!("test-ceiling-mixed-{max}"), max).await;
		let largest = store.max_item_size;
		let appended: Vec<Value> = (0..200)
			.map(|i| item_of_len(5 + (i * 37) % (largest - 4), i))
			.collect();
		for item in &appended {
			store.append(item.clone()).unwrap();
		}

		let batches = drain(&mut store, None, None);
		assert!(batches.len() > 1, "max={max} should take several batches");
		assert_eq!(batches.concat(), appended, "max={max}");
	}
}

async fn three_forty_byte_items(db_name: &str) -> WebStore {
	let mut store = fresh_store(db_name, 4096).await;
	for tag in 0..3 {
		store.append(item_of_len(40, tag)).unwrap();
	}
	store
}

#[wasm_bindgen_test]
async fn test_exact_fit_and_one_byte_over() {
	let mut store = three_forty_byte_items("test-ceiling-exact").await;
	let exact = envelope(KEY) + 3 * 40 + 2;
	let result = store.fetch(None, Some(exact)).unwrap().unwrap();
	let batch = serde_json::to_string(result.data.as_ref().unwrap()).unwrap();
	assert_eq!(batch.len(), exact);
	assert_eq!(fetched(&mut store, exact), 3);
	assert_eq!(fetched(&mut store, exact - 1), 2);
}

#[wasm_bindgen_test]
async fn test_separators_are_counted() {
	let mut store = three_forty_byte_items("test-ceiling-separators").await;
	// Room for the three items only if the two commas were free.
	assert_eq!(fetched(&mut store, envelope(KEY) + 3 * 40), 2);
	assert_eq!(fetched(&mut store, envelope(KEY) + 3 * 40 + 1), 2);
}

#[wasm_bindgen_test]
async fn test_envelope_is_counted() {
	let mut store = three_forty_byte_items("test-ceiling-envelope").await;
	// Room for the three items and commas, and all but one byte of the envelope.
	assert_eq!(fetched(&mut store, envelope(KEY) - 1 + 3 * 40 + 2), 2);
	// One item and its envelope, exactly; a byte less fetches nothing.
	assert_eq!(fetched(&mut store, envelope(KEY) + 40), 1);
	assert_eq!(fetched(&mut store, envelope(KEY) + 39), 0);
}

#[wasm_bindgen_test]
async fn test_envelope_length_matches_real_timestamps() {
	let store = fresh_store("test-ceiling-real-timestamps", 4096).await;
	for _ in 0..3 {
		let batch = store.config.envelope.batch(Vec::new(), chrono::Utc::now());
		let len = serde_json::to_string(&batch).unwrap().len();
		assert_eq!(len, envelope(KEY));
		assert_eq!(len, store.envelope_len);
	}
}

#[wasm_bindgen_test]
async fn test_longer_field_value_fits_fewer_items() {
	let long_source = "k".repeat(60);
	let mut sizes = Vec::new();
	for (i, source) in [KEY, long_source.as_str()].into_iter().enumerate() {
		let db_name = format!("test-ceiling-long-field-{i}");
		let mut store = fresh_store_with(config(&db_name, 400, with_source(source))).await;
		let appended: Vec<Value> = (0..30).map(|tag| item_of_len(40, tag)).collect();
		for item in &appended {
			store.append(item.clone()).unwrap();
		}
		let batches = drain(&mut store, None, None);
		assert_eq!(batches.concat(), appended);
		sizes.push(batches[0].len());
	}
	assert!(sizes[1] < sizes[0], "{sizes:?}");
}

#[wasm_bindgen_test]
async fn test_multibyte_utf8_is_measured_in_bytes() {
	let mut store = fresh_store("test-ceiling-utf8", 256).await;
	let words = ["🦀", "漢字", "é", "👾💥", "日本語テキスト", "ascii"];
	let appended: Vec<Value> = (0..120)
		.map(|i| json!({"i": i, "s": words[i % words.len()].repeat(1 + i % 7)}))
		.collect();
	for item in &appended {
		store.append(item.clone()).unwrap();
	}
	let chars: usize = appended.iter().map(|v| v.to_string().chars().count()).sum();
	let bytes: usize = appended.iter().map(item_len).sum();
	assert!(bytes > chars, "test data must be multi-byte");

	assert_eq!(drain(&mut store, None, None).concat(), appended);
}

#[wasm_bindgen_test]
async fn test_per_call_max_bytes_is_the_ceiling() {
	for max_bytes in [150usize, 300, 2000] {
		let db_name = format!("test-ceiling-per-call-{max_bytes}");
		let mut store = fresh_store(&db_name, 500).await;
		let appended: Vec<Value> = (0..60).map(|i| item_of_len(20 + i % 30, i)).collect();
		for item in &appended {
			store.append(item.clone()).unwrap();
		}
		// Smaller and larger than the configured 500: each fetch obeys max_bytes.
		let batches = drain(&mut store, None, Some(max_bytes));
		assert_eq!(batches.concat(), appended, "max_bytes={max_bytes}");
	}
}

#[wasm_bindgen_test]
async fn test_count_item_limit_and_bytes_each_win() {
	let mut config = config("test-ceiling-limits", 1000, with_source(KEY));
	config.max_fetch_items = Some(6);
	let mut store = fresh_store_with(config).await;
	for tag in 0..100 {
		store.append(item_of_len(40, tag)).unwrap();
	}
	let count = |store: &mut WebStore, count, max_bytes: Option<usize>| {
		let result = store.fetch(count, max_bytes).unwrap().unwrap();
		checked_batch(&result, max_bytes.unwrap_or(1000)).len()
	};
	// count wins
	assert_eq!(count(&mut store, Some(2), None), 2);
	// max_fetch_items wins
	assert_eq!(count(&mut store, Some(50), None), 6);
	assert_eq!(count(&mut store, None, None), 6);
	// bytes win
	let three = envelope(KEY) + 3 * 40 + 2;
	assert_eq!(count(&mut store, Some(50), Some(three)), 3);
	assert_eq!(count(&mut store, None, Some(three)), 3);

	let batches = drain(&mut store, Some(4), Some(three));
	assert!(batches.iter().all(|b| b.len() <= 3));
	assert_eq!(batches.concat().len(), 100);
}

#[wasm_bindgen_test]
async fn test_max_items_drop_oldest_with_ceiling() {
	let mut config = config("test-ceiling-max-items", 200, with_source(KEY));
	config.max_items = Some(10);
	let mut store = fresh_store_with(config).await;
	let appended: Vec<Value> = (0..15).map(|tag| item_of_len(30, tag)).collect();
	for item in &appended {
		store.append(item.clone()).unwrap();
	}
	assert_eq!(store.items.len(), 10);
	let batches = drain(&mut store, None, None);
	assert!(batches.len() > 1);
	assert_eq!(batches.concat(), appended[5..]);
}
