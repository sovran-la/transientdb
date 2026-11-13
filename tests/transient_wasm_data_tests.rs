//! WASM tests for TransientDB with WebStore backend: every kind of JSON value comes back
//! exactly as it was appended.

#![cfg(target_arch = "wasm32")]

use serde_json::{json, Value};
use transientdb::{TransientDB, WebConfig, WebStore};
use wasm_bindgen_test::*;

mod common;
use common::{available, settle};

wasm_bindgen_test_configure!(run_in_browser);

fn test_config(db_name: &str) -> WebConfig {
	WebConfig {
		database_name: db_name.to_string(),
		max_item_size: 64 * 1024,
		max_items: 1000,
		orphan_interval: std::time::Duration::from_secs(30),
	}
}

/// A TransientDB over a WebStore on `db_name`, emptied of anything a previous run left.
async fn db(db_name: &str) -> TransientDB {
	let db = TransientDB::new(WebStore::new(test_config(db_name)).await);
	db.reset();
	db
}

// =============================================================================
// Data Types
// =============================================================================

#[wasm_bindgen_test]
async fn test_transientdb_json_types() {
	let db = db("test-json-types").await;
	let items = vec![
		json!(null),
		json!(true),
		json!(false),
		json!(42),
		json!(-17),
		json!(1.23),
		json!("string"),
		json!([1, 2, 3]),
		json!({"nested": "object"}),
	];
	for item in &items {
		db.append(item.clone()).unwrap();
	}
	assert_eq!(available(&db), items);
}

#[wasm_bindgen_test]
async fn test_transientdb_nested_json() {
	let db = db("test-nested").await;
	let nested = json!({"level1": {"level2": {"level3": {"data": [1, 2, {"deep": true}]}}}});
	db.append(nested.clone()).unwrap();
	assert_eq!(available(&db), vec![nested]);
}

#[wasm_bindgen_test]
async fn test_transientdb_unicode() {
	let db = db("test-unicode").await;
	let item = json!({"emoji": "🦀💥👾", "japanese": "日本語", "arabic": "مرحبا", "math": "∑∏∫"});
	db.append(item.clone()).unwrap();
	assert_eq!(available(&db), vec![item]);
}

#[wasm_bindgen_test]
async fn test_transientdb_items_json_structure() {
	let db = db("test-items-json").await;
	db.append(json!({"test": "data"})).unwrap();
	db.append(json!([1, 2])).unwrap();
	let items = db.fetch(10, 1024).unwrap().unwrap();
	assert_eq!(items.json(), r#"[{"test":"data"},[1,2]]"#);
	let body: Value = serde_json::from_str(&format!("{{\"batch\":{}}}", items.json())).unwrap();
	assert_eq!(body, json!({"batch": [{"test": "data"}, [1, 2]]}));
	assert_eq!(
		items.iter().collect::<Vec<_>>(),
		[r#"{"test":"data"}"#, "[1,2]"]
	);
}

// =============================================================================
// Edge Cases
// =============================================================================

#[wasm_bindgen_test]
async fn test_transientdb_empty_object() {
	let db = db("test-empty-obj").await;
	db.append(json!({})).unwrap();
	assert_eq!(available(&db), vec![json!({})]);
}

#[wasm_bindgen_test]
async fn test_transientdb_empty_array() {
	let db = db("test-empty-arr").await;
	db.append(json!([])).unwrap();
	assert_eq!(available(&db), vec![json!([])]);
}

#[wasm_bindgen_test]
async fn test_transientdb_large_payload() {
	let db = db("test-large").await;
	let mut large = json!({});
	for i in 0..100 {
		large[format!("field_{}", i)] = json!({"value": "x".repeat(100), "number": i});
	}
	db.append(large.clone()).unwrap();
	let items = available(&db);
	assert_eq!(items[0]["field_50"]["number"], 50);
	assert_eq!(items, vec![large]);
}

#[wasm_bindgen_test]
async fn test_transientdb_special_characters() {
	let db = db("test-special-chars").await;
	let item = json!({
		"quotes": "He said \"hello\"",
		"backslash": "path\\to\\file",
		"newlines": "line1\nline2\r\nline3",
		"tabs": "col1\tcol2",
		"null_char": "before\u{0000}after"
	});
	db.append(item.clone()).unwrap();
	assert_eq!(available(&db), vec![item]);
}

#[wasm_bindgen_test]
async fn test_transientdb_numeric_edge_cases() {
	let db_name = "test-numeric";
	let db = db(db_name).await;
	let item = json!({
		"zero": 0,
		"negative": -42,
		"float": 1.23456789012345,
		"large": 9007199254740991_i64,  // Max safe JS integer
		"beyond": u64::MAX,
		"whole_float": 42.0,
		"small_float": 0.0000001
	});
	db.append(item.clone()).unwrap();
	// Exactly, after a reload too.
	drop(db);
	let reloaded = TransientDB::new(WebStore::new(test_config(db_name)).await);
	let fetched = reloaded.fetch(1, 1024).unwrap().unwrap();
	assert_eq!(fetched.values(), vec![item.clone()]);
	assert_eq!(fetched.iter().next().unwrap(), item.to_string());
	assert!(fetched.json().contains("18446744073709551615") && fetched.json().contains("42.0"));
	drop(fetched);
	reloaded.reset();
	settle().await;
}
