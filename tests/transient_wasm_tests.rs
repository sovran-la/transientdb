//! WASM tests for TransientDB with WebStore backend
//!
//! These tests validate that TransientDB works correctly with WebStore
//! in a browser environment. This is a regression test for the MaybeSend
//! fix that allows Rc<IdbDatabase> to be used with TransientDB on WASM.

#![cfg(target_arch = "wasm32")]

use serde_json::{json, Value};
use transientdb::{TransientDB, WebConfig, WebStore};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

fn test_config(db_name: &str) -> WebConfig {
	WebConfig {
		write_key: "test-key".to_string(),
		database_name: db_name.to_string(),
		max_items: 1000,
		max_fetch_size: 1024 * 1024,
	}
}

// =============================================================================
// Basic Operations
// =============================================================================

#[wasm_bindgen_test]
async fn test_transientdb_creation() {
	// This test validates the MaybeSend fix - previously this would fail with:
	// "Rc<IdbDatabase> cannot be sent between threads safely"
	let store = WebStore::new(test_config("test-creation")).await;
	let _db = TransientDB::new(store);
	// If we got here without a compile error, the fix works!
}

#[wasm_bindgen_test]
async fn test_transientdb_empty_state() {
	let store = WebStore::new(test_config("test-empty")).await;
	let db = TransientDB::new(store);

	assert!(!db.has_data(), "New TransientDB should be empty");
}

#[wasm_bindgen_test]
async fn test_transientdb_append_single() {
	let store = WebStore::new(test_config("test-append-single")).await;
	let db = TransientDB::new(store);

	db.append(json!({"event": "test", "value": 42})).unwrap();
	assert!(db.has_data(), "Should have data after append");
}

#[wasm_bindgen_test]
async fn test_transientdb_append_multiple() {
	let store = WebStore::new(test_config("test-append-multi")).await;
	let db = TransientDB::new(store);

	for i in 0..10 {
		db.append(json!({"index": i})).unwrap();
	}
	assert!(db.has_data());

	// Fetch and verify count
	if let Some(result) = db.fetch(None, None).unwrap() {
		let batch: Value = result.data.unwrap();
		let items = batch["batch"].as_array().unwrap();
		assert_eq!(items.len(), 10, "Should have 10 items");
	} else {
		panic!("Expected data but got none");
	}
}

// =============================================================================
// Fetch Operations
// =============================================================================

#[wasm_bindgen_test]
async fn test_transientdb_fetch_empty() {
	let store = WebStore::new(test_config("test-fetch-empty")).await;
	let db = TransientDB::new(store);

	let result = db.fetch(None, None).unwrap();
	assert!(result.is_none(), "Fetch on empty store should return None");
}

#[wasm_bindgen_test]
async fn test_transientdb_fetch_with_count_limit() {
	let store = WebStore::new(test_config("test-fetch-count")).await;
	let db = TransientDB::new(store);

	for i in 0..20 {
		db.append(json!({"index": i})).unwrap();
	}

	// Fetch only 5 items
	if let Some(result) = db.fetch(Some(5), None).unwrap() {
		let batch: Value = result.data.unwrap();
		let items = batch["batch"].as_array().unwrap();
		assert_eq!(items.len(), 5, "Should respect count limit");

		// Verify we got the first 5 (FIFO order)
		for (i, item) in items.iter().enumerate() {
			assert_eq!(item["index"], i as i64);
		}
	} else {
		panic!("Expected data");
	}
}

#[wasm_bindgen_test]
async fn test_transientdb_fetch_with_byte_limit() {
	let store = WebStore::new(test_config("test-fetch-bytes")).await;
	let db = TransientDB::new(store);

	// Add items with predictable sizes
	for i in 0..20 {
		let padding = "x".repeat(100);
		db.append(json!({
			"index": i,
			"padding": padding
		}))
		.unwrap();
	}

	// Fetch with small byte limit - should get fewer items
	if let Some(result) = db.fetch(None, Some(500)).unwrap() {
		let batch: Value = result.data.unwrap();
		let items = batch["batch"].as_array().unwrap();
		assert!(items.len() < 20, "Byte limit should restrict item count");
		assert!(items.len() > 0, "Should get at least one item");
	} else {
		panic!("Expected data");
	}
}

#[wasm_bindgen_test]
async fn test_transientdb_fetch_preserves_data() {
	let store = WebStore::new(test_config("test-fetch-preserves")).await;
	let db = TransientDB::new(store);

	db.append(json!({"key": "value"})).unwrap();

	// Fetch without removing
	let _ = db.fetch(None, None).unwrap();

	// Data should still be there
	assert!(db.has_data(), "Fetch should not remove data");
}

// =============================================================================
// Remove Operations
// =============================================================================

#[wasm_bindgen_test]
async fn test_transientdb_remove() {
	let store = WebStore::new(test_config("test-remove")).await;
	let db = TransientDB::new(store);

	db.append(json!({"test": "data"})).unwrap();
	assert!(db.has_data());

	// Fetch and remove
	if let Some(result) = db.fetch(None, None).unwrap() {
		if let Some(removable) = result.removable {
			db.remove(&removable).unwrap();
		}
	}

	assert!(!db.has_data(), "Should be empty after remove");
}

#[wasm_bindgen_test]
async fn test_transientdb_partial_remove() {
	let store = WebStore::new(test_config("test-partial-remove")).await;
	let db = TransientDB::new(store);

	for i in 0..10 {
		db.append(json!({"index": i})).unwrap();
	}

	// Fetch and remove only 3 items
	if let Some(result) = db.fetch(Some(3), None).unwrap() {
		if let Some(removable) = result.removable {
			db.remove(&removable).unwrap();
		}
	}

	// Should have 7 items left
	if let Some(result) = db.fetch(None, None).unwrap() {
		let batch: Value = result.data.unwrap();
		let items = batch["batch"].as_array().unwrap();
		assert_eq!(items.len(), 7, "Should have 7 items remaining");

		// First item should now be index 3
		assert_eq!(items[0]["index"], 3);
	} else {
		panic!("Expected remaining data");
	}
}

// =============================================================================
// Reset Operations
// =============================================================================

#[wasm_bindgen_test]
async fn test_transientdb_reset() {
	let store = WebStore::new(test_config("test-reset")).await;
	let db = TransientDB::new(store);

	for i in 0..10 {
		db.append(json!({"index": i})).unwrap();
	}
	assert!(db.has_data());

	db.reset();
	assert!(!db.has_data(), "Should be empty after reset");
}

#[wasm_bindgen_test]
async fn test_transientdb_reset_then_append() {
	let store = WebStore::new(test_config("test-reset-append")).await;
	let db = TransientDB::new(store);

	db.append(json!({"before": "reset"})).unwrap();
	db.reset();
	db.append(json!({"after": "reset"})).unwrap();

	if let Some(result) = db.fetch(None, None).unwrap() {
		let batch: Value = result.data.unwrap();
		let items = batch["batch"].as_array().unwrap();
		assert_eq!(items.len(), 1);
		assert_eq!(items[0]["after"], "reset");
	} else {
		panic!("Expected data");
	}
}

// =============================================================================
// JSON Type Handling
// =============================================================================

#[wasm_bindgen_test]
async fn test_transientdb_json_types() {
	let store = WebStore::new(test_config("test-json-types")).await;
	let db = TransientDB::new(store);

	// Test all JSON types
	db.append(json!(null)).unwrap();
	db.append(json!(true)).unwrap();
	db.append(json!(false)).unwrap();
	db.append(json!(42)).unwrap();
	db.append(json!(3.14159)).unwrap();
	db.append(json!("string")).unwrap();
	db.append(json!(["array", "of", "values"])).unwrap();
	db.append(json!({"object": "value"})).unwrap();

	if let Some(result) = db.fetch(None, None).unwrap() {
		let batch: Value = result.data.unwrap();
		let items = batch["batch"].as_array().unwrap();
		assert_eq!(items.len(), 8, "All JSON types should be stored");
	}
}

#[wasm_bindgen_test]
async fn test_transientdb_nested_json() {
	let store = WebStore::new(test_config("test-nested-json")).await;
	let db = TransientDB::new(store);

	let nested = json!({
		"level1": {
			"level2": {
				"level3": {
					"value": "deep",
					"array": [1, 2, {"nested": true}]
				}
			}
		}
	});

	db.append(nested.clone()).unwrap();

	if let Some(result) = db.fetch(None, None).unwrap() {
		let batch: Value = result.data.unwrap();
		let items = batch["batch"].as_array().unwrap();
		assert_eq!(items[0], nested);
	}
}

#[wasm_bindgen_test]
async fn test_transientdb_unicode() {
	let store = WebStore::new(test_config("test-unicode")).await;
	let db = TransientDB::new(store);

	db.append(json!({
		"emoji": "🦀💥👾",
		"chinese": "你好世界",
		"arabic": "مرحبا بالعالم",
		"mixed": "Hello 世界 🌍"
	}))
	.unwrap();

	if let Some(result) = db.fetch(None, None).unwrap() {
		let batch: Value = result.data.unwrap();
		let items = batch["batch"].as_array().unwrap();
		assert_eq!(items[0]["emoji"], "🦀💥👾");
		assert_eq!(items[0]["chinese"], "你好世界");
	}
}

// =============================================================================
// Batch Metadata
// =============================================================================

#[wasm_bindgen_test]
async fn test_transientdb_batch_structure() {
	let store = WebStore::new(test_config("test-batch-structure")).await;
	let db = TransientDB::new(store);

	db.append(json!({"test": "data"})).unwrap();

	if let Some(result) = db.fetch(None, None).unwrap() {
		let batch: Value = result.data.unwrap();

		// Verify batch structure
		assert!(batch.get("batch").is_some(), "Missing 'batch' field");
		assert!(batch.get("sentAt").is_some(), "Missing 'sentAt' field");
		assert!(batch.get("writeKey").is_some(), "Missing 'writeKey' field");
		assert_eq!(
			batch["writeKey"].as_str(),
			Some("test-key"),
			"Wrong writeKey"
		);
	}
}

// =============================================================================
// FIFO Behavior
// =============================================================================

#[wasm_bindgen_test]
async fn test_transientdb_fifo_order() {
	let store = WebStore::new(test_config("test-fifo")).await;
	let db = TransientDB::new(store);

	for i in 0..5 {
		db.append(json!({"order": i})).unwrap();
	}

	if let Some(result) = db.fetch(None, None).unwrap() {
		let batch: Value = result.data.unwrap();
		let items = batch["batch"].as_array().unwrap();

		for (i, item) in items.iter().enumerate() {
			assert_eq!(item["order"], i as i64, "Items should be in FIFO order");
		}
	}
}

#[wasm_bindgen_test]
async fn test_transientdb_max_items_eviction() {
	let config = WebConfig {
		write_key: "test-key".to_string(),
		database_name: "test-max-items".to_string(),
		max_items: 5, // Small limit
		max_fetch_size: 1024 * 1024,
	};
	let store = WebStore::new(config).await;
	let db = TransientDB::new(store);

	// Add more items than max
	for i in 0..10 {
		db.append(json!({"index": i})).unwrap();
	}

	// Should only have last 5 items
	if let Some(result) = db.fetch(None, None).unwrap() {
		let batch: Value = result.data.unwrap();
		let items = batch["batch"].as_array().unwrap();
		assert_eq!(items.len(), 5, "Should only have max_items");

		// Should be items 5-9 (oldest evicted)
		assert_eq!(items[0]["index"], 5);
		assert_eq!(items[4]["index"], 9);
	}
}

// =============================================================================
// Rapid Operations (Stress-lite for single-threaded WASM)
// =============================================================================

#[wasm_bindgen_test]
async fn test_transientdb_rapid_append() {
	let store = WebStore::new(test_config("test-rapid-append")).await;
	let db = TransientDB::new(store);

	// Rapid-fire appends
	for i in 0..100 {
		db.append(json!({"rapid": i})).unwrap();
	}

	if let Some(result) = db.fetch(None, None).unwrap() {
		let batch: Value = result.data.unwrap();
		let items = batch["batch"].as_array().unwrap();
		assert_eq!(items.len(), 100);
	}
}

#[wasm_bindgen_test]
async fn test_transientdb_rapid_fetch_remove_cycle() {
	let store = WebStore::new(test_config("test-rapid-cycle")).await;
	let db = TransientDB::new(store);

	// Multiple append/fetch/remove cycles
	for cycle in 0..10 {
		// Append batch
		for i in 0..10 {
			db.append(json!({"cycle": cycle, "item": i})).unwrap();
		}

		// Fetch and remove all
		while db.has_data() {
			if let Some(result) = db.fetch(Some(5), None).unwrap() {
				if let Some(removable) = result.removable {
					db.remove(&removable).unwrap();
				}
			}
		}
	}

	assert!(!db.has_data(), "Should be empty after all cycles");
}

#[wasm_bindgen_test]
async fn test_transientdb_interleaved_operations() {
	let store = WebStore::new(test_config("test-interleaved")).await;
	let db = TransientDB::new(store);

	// Interleave appends and partial removes
	for i in 0..20 {
		db.append(json!({"index": i})).unwrap();

		// Every 5 items, remove 2
		if i > 0 && i % 5 == 0 {
			if let Some(result) = db.fetch(Some(2), None).unwrap() {
				if let Some(removable) = result.removable {
					db.remove(&removable).unwrap();
				}
			}
		}
	}

	// Should have some items remaining
	assert!(db.has_data());
}

// =============================================================================
// Edge Cases
// =============================================================================

#[wasm_bindgen_test]
async fn test_transientdb_empty_object() {
	let store = WebStore::new(test_config("test-empty-obj")).await;
	let db = TransientDB::new(store);

	db.append(json!({})).unwrap();

	if let Some(result) = db.fetch(None, None).unwrap() {
		let batch: Value = result.data.unwrap();
		let items = batch["batch"].as_array().unwrap();
		assert_eq!(items[0], json!({}));
	}
}

#[wasm_bindgen_test]
async fn test_transientdb_empty_array() {
	let store = WebStore::new(test_config("test-empty-arr")).await;
	let db = TransientDB::new(store);

	db.append(json!([])).unwrap();

	if let Some(result) = db.fetch(None, None).unwrap() {
		let batch: Value = result.data.unwrap();
		let items = batch["batch"].as_array().unwrap();
		assert_eq!(items[0], json!([]));
	}
}

#[wasm_bindgen_test]
async fn test_transientdb_large_payload() {
	let store = WebStore::new(test_config("test-large")).await;
	let db = TransientDB::new(store);

	// Create a large JSON object
	let mut large = json!({});
	for i in 0..100 {
		large[format!("field_{}", i)] = json!({
			"value": "x".repeat(100),
			"number": i
		});
	}

	db.append(large.clone()).unwrap();

	if let Some(result) = db.fetch(None, None).unwrap() {
		let batch: Value = result.data.unwrap();
		let items = batch["batch"].as_array().unwrap();
		assert_eq!(items[0]["field_50"]["number"], 50);
	}
}

#[wasm_bindgen_test]
async fn test_transientdb_special_characters() {
	let store = WebStore::new(test_config("test-special-chars")).await;
	let db = TransientDB::new(store);

	db.append(json!({
		"quotes": "He said \"hello\"",
		"backslash": "path\\to\\file",
		"newlines": "line1\nline2\r\nline3",
		"tabs": "col1\tcol2",
		"null_char": "before\u{0000}after"
	}))
	.unwrap();

	if let Some(result) = db.fetch(None, None).unwrap() {
		let batch: Value = result.data.unwrap();
		let items = batch["batch"].as_array().unwrap();
		assert_eq!(items[0]["quotes"], "He said \"hello\"");
		assert_eq!(items[0]["newlines"], "line1\nline2\r\nline3");
	}
}

#[wasm_bindgen_test]
async fn test_transientdb_numeric_edge_cases() {
	let store = WebStore::new(test_config("test-numeric")).await;
	let db = TransientDB::new(store);

	db.append(json!({
		"zero": 0,
		"negative": -42,
		"float": 3.14159265358979,
		"large": 9007199254740991_i64,  // Max safe JS integer
		"small_float": 0.0000001
	}))
	.unwrap();

	if let Some(result) = db.fetch(None, None).unwrap() {
		let batch: Value = result.data.unwrap();
		let items = batch["batch"].as_array().unwrap();
		assert_eq!(items[0]["zero"], 0);
		assert_eq!(items[0]["negative"], -42);
	}
}
