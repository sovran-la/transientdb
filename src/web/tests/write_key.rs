//! `write_key` round-trips through the batch envelope whatever it contains, and the
//! byte ceiling counts its escaped bytes.

use super::support::*;
use super::*;
use crate::batch::AWKWARD_WRITE_KEYS;

/// The envelope length for `write_key`, built independently of the store. WebStore's
/// `sentAt` is a fixed-width ISO string.
fn expected_envelope(write_key: &str) -> usize {
	let key = serde_json::to_string(write_key).unwrap();
	let sent_at = "2026-11-09T12:34:56.789Z";
	format!("{{\"batch\":[],\"sentAt\":\"{sent_at}\",\"writeKey\":{key}}}").len()
}

/// The fetched batch, serialized and parsed back.
fn reparsed(result: &DataResult<Value>) -> Value {
	let text = serde_json::to_string(result.data.as_ref().unwrap()).unwrap();
	serde_json::from_str(&text).unwrap()
}

#[wasm_bindgen_test]
async fn test_awkward_keys_round_trip_under_the_ceiling() {
	for (i, key) in AWKWARD_WRITE_KEYS.into_iter().enumerate() {
		let db_name = format!("test-write-key-round-trip-{i}");
		let mut store = fresh_store_with(config(&db_name, 300, key)).await;
		let appended: Vec<Value> = (0..40).map(|i| json!({"i": i})).collect();
		for item in &appended {
			store.append(item.clone()).unwrap();
		}

		let mut seen = Vec::new();
		let mut batches = 0;
		while let Some(result) = store.fetch(None, None).unwrap() {
			seen.extend(checked_batch(&result, 300));
			assert_eq!(reparsed(&result)["writeKey"], key);
			store.remove(&result.removable.unwrap()).unwrap();
			batches += 1;
		}
		assert!(batches > 1, "key {key:?} should take several batches");
		assert_eq!(seen, appended, "key {key:?}");
	}
	settle().await;
}

#[wasm_bindgen_test]
async fn test_awkward_keys_exact_fit_lands_on_the_limit() {
	for (i, key) in AWKWARD_WRITE_KEYS.into_iter().enumerate() {
		assert_eq!(envelope(key), expected_envelope(key), "key {key:?}");
		let db_name = format!("test-write-key-exact-{i}");
		let mut store = fresh_store_with(config(&db_name, 4096, key)).await;
		for tag in 0..3 {
			store.append(item_of_len(40, tag)).unwrap();
		}
		let exact = expected_envelope(key) + 3 * 40 + 2;
		let result = store.fetch(None, Some(exact)).unwrap().unwrap();
		let text = serde_json::to_string(result.data.as_ref().unwrap()).unwrap();
		assert_eq!(text.len(), exact, "key {key:?}");
		assert_eq!(reparsed(&result)["writeKey"], key);
		assert_eq!(fetched(&mut store, exact - 1), 2, "key {key:?}");
		store.reset();
	}
	settle().await;
}
