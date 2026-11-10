//! Every envelope, awkward keys and values included, round-trips through a fetched batch,
//! and the byte ceiling counts its escaped bytes.

use super::support::*;
use super::*;
use crate::envelope::samples::{expected_len, items_of, sample_envelopes};

/// The fetched batch, serialized and parsed back.
fn reparsed(result: &DataResult<Value>) -> Value {
	let text = serde_json::to_string(result.data.as_ref().unwrap()).unwrap();
	serde_json::from_str(&text).unwrap()
}

#[wasm_bindgen_test]
async fn test_awkward_envelopes_round_trip_under_the_ceiling() {
	for (i, envelope) in sample_envelopes().into_iter().enumerate() {
		let db_name = format!("test-envelope-round-trip-{i}");
		let max = (expected_len(&envelope) + 120).max(100);
		let mut store = fresh_store_with(config(&db_name, max, envelope.clone())).await;
		let appended: Vec<Value> = (0..40).map(|i| json!({"i": i})).collect();
		for item in &appended {
			store.append(item.clone()).unwrap();
		}

		let mut seen = Vec::new();
		let mut batches = 0;
		while let Some(result) = store.fetch(None, None).unwrap() {
			let items = checked_batch_with(&result, max, &envelope);
			// What a consumer parses back is still exactly the configured shape.
			assert_eq!(items_of(&reparsed(&result), &envelope), items);
			seen.extend(items);
			store.remove(&result.removable.unwrap()).unwrap();
			batches += 1;
		}
		assert!(batches > 1, "{envelope:?} should take several batches");
		assert_eq!(seen, appended, "{envelope:?}");
	}
	settle().await;
}

#[wasm_bindgen_test]
async fn test_awkward_envelopes_exact_fit_lands_on_the_limit() {
	for (i, envelope) in sample_envelopes().into_iter().enumerate() {
		let db_name = format!("test-envelope-exact-{i}");
		let mut store = fresh_store_with(config(&db_name, 4096, envelope.clone())).await;
		assert_eq!(store.envelope_len, expected_len(&envelope), "{envelope:?}");
		for tag in 0..3 {
			store.append(item_of_len(40, tag)).unwrap();
		}
		let exact = expected_len(&envelope) + 3 * 40 + 2;
		let result = store.fetch(None, Some(exact)).unwrap().unwrap();
		let text = serde_json::to_string(result.data.as_ref().unwrap()).unwrap();
		assert_eq!(text.len(), exact, "{envelope:?}");
		assert_eq!(items_of(&reparsed(&result), &envelope).len(), 3);
		assert_eq!(fetched(&mut store, exact), 3, "{envelope:?}");
		assert_eq!(fetched(&mut store, exact - 1), 2, "{envelope:?}");
		store.reset();
	}
	settle().await;
}

#[wasm_bindgen_test]
async fn test_awkward_envelopes_largest_item_fits_alone() {
	for (i, envelope) in sample_envelopes().into_iter().enumerate() {
		let db_name = format!("test-envelope-largest-{i}");
		let max = expected_len(&envelope) + 150;
		let mut store = fresh_store_with(config(&db_name, max, envelope.clone())).await;
		assert_eq!(store.max_item_size, 150, "{envelope:?}");
		assert_invalid_input(&store.append(item_of_len(151, 0)).unwrap_err());
		store.append(item_of_len(150, 1)).unwrap();
		assert_eq!(drain(&mut store, None, None).concat().len(), 1);
	}
	settle().await;
}
