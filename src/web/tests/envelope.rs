//! A fetched batch is exactly the configured envelope around the items, the byte ceiling
//! and the oversized-item rejection follow its size, and IndexedDB holds bare items, so
//! what was persisted is returned under whatever envelope the store has now.

use super::support::*;
use super::*;
use crate::envelope::samples::{expected_len, is_timestamp, sample_envelopes};

/// The one batch fetched after appending `items` under `envelope` to a fresh store.
async fn only_batch(db_name: &str, envelope: Envelope, items: &[Value]) -> Value {
	let mut store = fresh_store_with(config(db_name, 4096, envelope)).await;
	for item in items {
		store.append(item.clone()).unwrap();
	}
	let result = store.fetch(None, None).unwrap().unwrap();
	assert_eq!(result.removable.as_ref().unwrap().len(), items.len());
	store.reset();
	result.data.unwrap()
}

#[wasm_bindgen_test]
async fn test_batch_has_exactly_the_configured_keys() {
	let items = [json!({"a": 1}), json!("b"), json!([3])];
	for (i, envelope) in sample_envelopes().into_iter().enumerate() {
		let db_name = format!("test-envelope-keys-{i}");
		let batch = only_batch(&db_name, envelope.clone(), &items).await;
		let object = batch.as_object().unwrap();
		let mut keys: Vec<&String> = object.keys().collect();
		keys.sort();
		let mut expected: Vec<&String> = std::iter::once(&envelope.items_key)
			.chain(envelope.fields.iter().map(|(key, _)| key))
			.chain(envelope.timestamp_key.as_ref())
			.collect();
		expected.sort();
		assert_eq!(keys, expected, "{envelope:?}");
		assert_eq!(object[&envelope.items_key], json!(items), "{envelope:?}");
		for (key, value) in &envelope.fields {
			assert_eq!(&object[key], value, "{envelope:?}");
		}
		if let Some(key) = &envelope.timestamp_key {
			assert!(is_timestamp(&object[key]), "{envelope:?}: {batch}");
			assert_eq!(object[key].as_str().unwrap().len(), 24);
		}
	}
	settle().await;
}

#[wasm_bindgen_test]
async fn test_default_envelope_serializes_to_exactly_items() {
	let batch = only_batch(
		"test-envelope-default",
		Envelope::default(),
		&[json!({"event": "a"}), json!(2)],
	)
	.await;
	assert_eq!(
		serde_json::to_string(&batch).unwrap(),
		r#"{"items":[{"event":"a"},2]}"#
	);
	settle().await;
}

#[wasm_bindgen_test]
async fn test_no_timestamp_unless_configured() {
	let envelope = Envelope::new("events").field("source", "sensor-7");
	let batch = only_batch("test-envelope-no-timestamp", envelope, &[json!(1)]).await;
	assert_eq!(batch, json!({"events": [1], "source": "sensor-7"}));
	settle().await;
}

/// Envelopes of very different sizes for the ceiling tests.
fn ceiling_envelopes() -> Vec<Envelope> {
	vec![
		Envelope::default(),
		Envelope::new("events").timestamp("sentAt"),
		Envelope::new("events").field("note", "n".repeat(150)),
		Envelope::new("events")
			.field("note", "n".repeat(150))
			.timestamp("sentAt"),
		Envelope::new("項目🦀")
			.field("ключ-🦀", "значение-鍵")
			.timestamp("時刻"),
		Envelope::new("項目🦀").field("ключ-🦀", "значение-鍵"),
	]
}

#[wasm_bindgen_test]
async fn test_ceiling_holds_under_every_envelope() {
	for (i, envelope) in ceiling_envelopes().into_iter().enumerate() {
		for room in [40usize, 100, 333] {
			let db_name = format!("test-envelope-ceiling-{i}-{room}");
			let max = (expected_len(&envelope) + room).max(100);
			let mut store = fresh_store_with(config(&db_name, max, envelope.clone())).await;
			let largest = store.max_item_size;
			let appended: Vec<Value> = (0..120)
				.map(|i| item_of_len(5 + (i * 37) % (largest - 4), i))
				.collect();
			for item in &appended {
				store.append(item.clone()).unwrap();
			}
			// drain checks every batch's length and shape.
			let batches = drain(&mut store, None, None);
			assert!(batches.len() > 1, "{envelope:?} max={max}");
			assert_eq!(batches.concat(), appended, "{envelope:?} max={max}");
		}
	}
	settle().await;
}

#[wasm_bindgen_test]
async fn test_exact_fit_under_every_envelope() {
	for (i, envelope) in ceiling_envelopes().into_iter().enumerate() {
		let db_name = format!("test-envelope-exact-fit-{i}");
		let mut store = fresh_store_with(config(&db_name, 4096, envelope.clone())).await;
		for tag in 0..3 {
			store.append(item_of_len(40, tag)).unwrap();
		}
		let exact = expected_len(&envelope) + 3 * 40 + 2;
		let result = store.fetch(None, Some(exact)).unwrap().unwrap();
		let text = serde_json::to_string(result.data.as_ref().unwrap()).unwrap();
		assert_eq!(text.len(), exact, "{envelope:?}");
		assert_eq!(fetched(&mut store, exact - 1), 2, "{envelope:?}");
		store.reset();
	}
	settle().await;
}

#[wasm_bindgen_test]
async fn test_oversized_boundary_moves_with_the_envelope() {
	let max = 400;
	let mut largest_seen = Vec::new();
	for (i, envelope) in ceiling_envelopes().into_iter().enumerate() {
		let db_name = format!("test-envelope-boundary-{i}");
		let mut store = fresh_store_with(config(&db_name, max, envelope.clone())).await;
		let largest = max - expected_len(&envelope);
		assert_eq!(store.max_item_size, largest, "{envelope:?}");
		assert_invalid_input(&store.append(item_of_len(largest + 1, 0)).unwrap_err());
		assert!(!store.has_data());
		store.append(item_of_len(largest, 1)).unwrap();
		let result = store.fetch(None, None).unwrap().unwrap();
		let text = serde_json::to_string(result.data.as_ref().unwrap()).unwrap();
		assert_eq!(text.len(), max, "{envelope:?}");
		largest_seen.push(largest);
		store.reset();
	}
	largest_seen.sort();
	largest_seen.dedup();
	assert_eq!(largest_seen.len(), ceiling_envelopes().len());
	settle().await;
}

#[wasm_bindgen_test]
async fn test_reopened_store_with_the_same_envelope_returns_the_same_items() {
	let db_name = "test-envelope-reopen-same";
	let envelope = Envelope::new("events")
		.field("source", "sensor-7")
		.timestamp("sentAt");
	let mut store = fresh_store_with(config(db_name, 4096, envelope.clone())).await;
	if !store.is_persisted() {
		web_sys::console::log_1(&"Skipping envelope reopen test - no persistence".into());
		return;
	}
	let appended: Vec<Value> = (0..4).map(|i| json!({"i": i})).collect();
	for item in &appended {
		store.append(item.clone()).unwrap();
	}
	let before = store.fetch(None, None).unwrap().unwrap().data.unwrap();
	settle().await;

	let mut reopened = WebStore::new(config(db_name, 4096, envelope.clone())).await;
	let result = reopened.fetch(None, None).unwrap().unwrap();
	let after = result.data.as_ref().unwrap();
	assert_eq!(checked_batch_with(&result, 4096, &envelope), appended);
	assert_eq!(after["events"], before["events"]);
	assert_eq!(after["source"], before["source"]);
	reopened.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_persisted_items_come_back_under_the_current_envelope() {
	let db_name = "test-envelope-reopen-other";
	let first = Envelope::new("events")
		.field("source", "sensor-7")
		.timestamp("sentAt");
	let mut store = fresh_store_with(config(db_name, 4096, first)).await;
	if !store.is_persisted() {
		web_sys::console::log_1(&"Skipping envelope change test - no persistence".into());
		return;
	}
	let appended: Vec<Value> = (0..4).map(|i| json!({"i": i})).collect();
	for item in &appended {
		store.append(item.clone()).unwrap();
	}
	settle().await;

	for (i, now) in [
		Envelope::default(),
		Envelope::new("records").field("site", json!({"id": 3})),
		Envelope::new("項目🦀").timestamp("at"),
	]
	.into_iter()
	.enumerate()
	{
		let mut reopened = WebStore::new(config(db_name, 4096, now.clone())).await;
		let result = reopened.fetch(None, None).unwrap().unwrap();
		// checked_batch_with asserts the batch has exactly `now`'s keys and values.
		assert_eq!(
			checked_batch_with(&result, 4096, &now),
			appended,
			"reopen {i}"
		);
	}
	let mut last = WebStore::new(config(db_name, 4096, Envelope::default())).await;
	last.reset();
	settle().await;
}

#[wasm_bindgen_test]
#[should_panic(
	expected = "Envelope key \"items\" is used twice? A JSON object only keeps one of them."
)]
async fn test_new_rejects_an_invalid_envelope() {
	let envelope = Envelope::new("items").timestamp("items");
	let _store = WebStore::new(config("test-panic", 1024, envelope)).await;
}
