//! A custom envelope end to end through TransientDB with the WebStore backend, and the
//! batch it returns against the one documented for the same envelope and items.

#![cfg(target_arch = "wasm32")]

use serde_json::{json, Value};
use transientdb::{Envelope, TransientDB, WebConfig, WebStore};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

async fn web_db(db_name: &str, envelope: Envelope) -> TransientDB<Value> {
	let db = TransientDB::new(
		WebStore::new(WebConfig {
			envelope,
			database_name: db_name.to_string(),
			max_items: None,
			max_fetch_size: 4096,
			max_fetch_items: None,
		})
		.await,
	);
	// Discard anything hydrated from a previous run.
	db.reset();
	db
}

/// Whether `value` is a timestamp in the documented format.
fn is_timestamp(value: &Value) -> bool {
	value.as_str().is_some_and(|s| {
		s.len() == 24 && chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S.%3fZ").is_ok()
	})
}

fn items() -> Vec<Value> {
	vec![
		json!({"event": "a", "n": 1}),
		json!("two"),
		json!([3, {"x": "}"}]),
	]
}

#[wasm_bindgen_test]
async fn test_web_store_custom_envelope_end_to_end() {
	let envelope = Envelope::new("events")
		.field("source", "sensor-7")
		.field("schema", 3)
		.field("tags", json!(["a", {"b": null}]))
		.timestamp("sentAt");
	let db = web_db("test-transient-envelope", envelope).await;
	for item in items() {
		db.append(item).unwrap();
	}
	let result = db.fetch(None, None).unwrap().unwrap();
	let mut batch = result.data.unwrap();
	assert!(is_timestamp(&batch["sentAt"]), "{batch}");
	batch["sentAt"] = json!("<timestamp>");
	assert_eq!(
		batch,
		json!({
			"events": items(),
			"source": "sensor-7",
			"schema": 3,
			"tags": ["a", {"b": null}],
			"sentAt": "<timestamp>",
		})
	);
	db.remove(&result.removable.unwrap()).unwrap();
	assert!(!db.has_data());
}

#[wasm_bindgen_test]
async fn test_web_batch_equals_the_documented_bytes_parsed() {
	let envelopes = [
		Envelope::default(),
		Envelope::new("events"),
		Envelope::new("項目🦀")
			.field("quote\"key", "back\\slash\nvalue")
			.field("obj", json!({"z": 1, "a": [true, 1.5]}))
			.timestamp("sentAt"),
	];
	for (i, envelope) in envelopes.into_iter().enumerate() {
		let db = web_db(
			&format!("test-transient-envelope-bytes-{i}"),
			envelope.clone(),
		)
		.await;
		for item in items() {
			db.append(item).unwrap();
		}
		let result = db.fetch(None, None).unwrap().unwrap();
		let batch = result.data.unwrap();

		// The bytes DirectoryStore would write for this envelope, built from the
		// documented layout, with the batch's own timestamp.
		let mut text = format!(
			"{{{}:[",
			serde_json::to_string(&envelope.items_key).unwrap()
		);
		let items: Vec<String> = items().iter().map(Value::to_string).collect();
		text += &items.join(",");
		text += "]";
		for (key, value) in &envelope.fields {
			text += &format!(",{}:{}", serde_json::to_string(key).unwrap(), value);
		}
		if let Some(key) = &envelope.timestamp_key {
			assert!(is_timestamp(&batch[key]), "{batch}");
			text += &format!(",{}:{}", serde_json::to_string(key).unwrap(), batch[key]);
		}
		text += "}";
		let expected: Value = serde_json::from_str(&text).unwrap();
		assert_eq!(batch, expected, "{envelope:?}");
		assert_eq!(serde_json::to_string(&batch).unwrap().len(), text.len());
		db.remove(&result.removable.unwrap()).unwrap();
	}
}
