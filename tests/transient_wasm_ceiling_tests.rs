//! Size limits through the TransientDB wrapper with the WebStore backend.

#![cfg(target_arch = "wasm32")]

use serde_json::{json, Value};
use transientdb::{Error, TransientDB, WebConfig, WebStore};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

async fn web_db(db_name: &str, max_item_size: usize) -> TransientDB {
	let db = TransientDB::new(
		WebStore::new(WebConfig {
			database_name: db_name.to_string(),
			max_item_size,
			max_items: 10_000,
		})
		.await,
	);
	// Discard anything hydrated from a previous run.
	db.reset();
	db
}

/// Fetches and removes until empty, checking each fetch is within `max_bytes`.
fn drain(db: &TransientDB, max_bytes: usize) -> Vec<Value> {
	let mut items = Vec::new();
	while let Some(fetched) = db.fetch(usize::MAX, max_bytes).unwrap() {
		assert!(
			fetched.byte_len() <= max_bytes,
			"{} bytes",
			fetched.byte_len()
		);
		assert_eq!(fetched.json().len(), fetched.byte_len() + fetched.len() + 1);
		items.extend(fetched.values());
		db.remove(fetched).unwrap();
	}
	assert!(!db.has_data());
	items
}

#[wasm_bindgen_test]
async fn test_web_oversized_append_through_transientdb() {
	let db = web_db("test-transient-ceiling-oversized", 200).await;
	db.append(json!({"i": 0})).unwrap();
	let err = db.append(json!({"big": "x".repeat(500)})).unwrap_err();
	assert!(
		matches!(
			err,
			Error::ItemTooLarge {
				size: 510,
				max: 200
			}
		),
		"{err:?}"
	);

	let mut expected = vec![json!({"i": 0})];
	for i in 1..20 {
		db.append(json!({"i": i, "pad": "p".repeat(i * 3)}))
			.unwrap();
		expected.push(json!({"i": i, "pad": "p".repeat(i * 3)}));
	}
	assert_eq!(drain(&db, 200), expected);
}

#[wasm_bindgen_test]
async fn test_web_mixed_sizes_respect_limits_through_transientdb() {
	let db = web_db("test-transient-ceiling-mixed", 230).await;
	let mut expected = Vec::new();
	for i in 0..100 {
		// Up to 79 crabs (316 bytes): those over 230 bytes are refused.
		let item = json!({"i": i, "s": "🦀".repeat(i % 80)});
		match db.append(item.clone()) {
			Ok(()) => expected.push(item),
			Err(e) => assert!(matches!(e, Error::ItemTooLarge { max: 230, .. })),
		}
	}
	assert!(expected.len() < 100, "some items should be too large");
	assert!(!expected.is_empty());
	assert_eq!(drain(&db, 300), expected);
}
