//! Byte-ceiling behavior through the TransientDB wrapper with the WebStore backend.

#![cfg(target_arch = "wasm32")]

use serde_json::{json, Value};
use std::io::ErrorKind;
use transientdb::{Envelope, TransientDB, WebConfig, WebStore};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

async fn web_db(db_name: &str, max_fetch_size: usize) -> TransientDB<Value> {
	let db = TransientDB::new(
		WebStore::new(WebConfig {
			envelope: Envelope::default(),
			database_name: db_name.to_string(),
			max_items: Some(10_000),
			max_fetch_size,
			max_fetch_items: None,
		})
		.await,
	);
	// Discard anything hydrated from a previous run.
	db.reset();
	db
}

/// Fetches and removes batches until empty, checking each is within `max` bytes.
fn drain(db: &TransientDB<Value>, max: usize) -> Vec<Value> {
	let mut items = Vec::new();
	while let Some(result) = db.fetch(None, None).unwrap() {
		let batch = result.data.unwrap();
		let len = serde_json::to_string(&batch).unwrap().len();
		assert!(len <= max, "batch is {len} bytes, over {max}");
		items.extend(batch["items"].as_array().unwrap().iter().cloned());
		db.remove(&result.removable.unwrap()).unwrap();
	}
	assert!(!db.has_data());
	items
}

#[wasm_bindgen_test]
async fn test_web_oversized_append_through_transientdb() {
	let db = web_db("test-transient-ceiling-oversized", 200).await;
	db.append(json!({"i": 0})).unwrap();
	let err = db.append(json!({"big": "x".repeat(500)})).unwrap_err();
	assert_eq!(err.kind(), ErrorKind::InvalidInput);
	assert!(err.to_string().contains("max_fetch_size"), "{err}");

	let mut expected = vec![json!({"i": 0})];
	for i in 1..20 {
		db.append(json!({"i": i, "pad": "p".repeat(i * 3)}))
			.unwrap();
		expected.push(json!({"i": i, "pad": "p".repeat(i * 3)}));
	}
	assert_eq!(drain(&db, 200), expected);
}

#[wasm_bindgen_test]
async fn test_web_mixed_sizes_respect_ceiling_through_transientdb() {
	let db = web_db("test-transient-ceiling-mixed", 300).await;
	let mut expected = Vec::new();
	for i in 0..100 {
		// Up to 79 crabs (316 bytes): those over 230 bytes cannot fit under 300.
		let item = json!({"i": i, "s": "🦀".repeat(i % 80)});
		match db.append(item.clone()) {
			Ok(()) => expected.push(item),
			Err(e) => assert_eq!(e.kind(), ErrorKind::InvalidInput),
		}
	}
	assert!(expected.len() < 100, "some items should be too large");
	assert!(!expected.is_empty());
	assert_eq!(drain(&db, 300), expected);
}
