//! WASM tests for TransientDB with WebStore backend
//!
//! These tests validate that TransientDB works correctly with WebStore
//! in a browser environment. This is a regression test for the MaybeSend
//! fix that allows Rc<IdbDatabase> to be used with TransientDB on WASM.

#![cfg(target_arch = "wasm32")]

use serde_json::json;
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
	}
}

/// A TransientDB over a WebStore on `db_name`, emptied of anything a previous run left.
async fn fresh_db(config: WebConfig) -> TransientDB {
	let db = TransientDB::new(WebStore::new(config).await);
	db.reset();
	db
}

async fn db(db_name: &str) -> TransientDB {
	fresh_db(test_config(db_name)).await
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
	let db = db("test-empty").await;
	assert!(!db.has_data(), "New TransientDB should be empty");
}

#[wasm_bindgen_test]
async fn test_transientdb_append_single() {
	let db = db("test-append-single").await;
	db.append(json!({"event": "test", "value": 42})).unwrap();
	assert!(db.has_data(), "Should have data after append");
}

#[wasm_bindgen_test]
async fn test_transientdb_append_multiple() {
	let db = db("test-append-multi").await;
	for i in 0..10 {
		db.append(json!({"index": i})).unwrap();
	}
	assert!(db.has_data());
	assert_eq!(db.fetch(100, 64 * 1024).unwrap().unwrap().len(), 10);
}

// =============================================================================
// Fetch Operations
// =============================================================================

#[wasm_bindgen_test]
async fn test_transientdb_fetch_empty() {
	let db = db("test-fetch-empty").await;
	assert!(db.fetch(100, 64 * 1024).unwrap().is_none());
	assert!(db.fetch(0, 0).unwrap().is_none());
}

#[wasm_bindgen_test]
async fn test_transientdb_fetch_with_count_limit() {
	let db = db("test-fetch-count").await;
	for i in 0..20 {
		db.append(json!({"index": i})).unwrap();
	}
	let items = db.fetch(5, 64 * 1024).unwrap().unwrap().values();
	assert_eq!(items.len(), 5, "Should respect count limit");
	for (i, item) in items.iter().enumerate() {
		assert_eq!(item["index"], i as i64);
	}
}

#[wasm_bindgen_test]
async fn test_transientdb_fetch_with_byte_limit() {
	let db = db("test-fetch-bytes").await;
	for i in 0..20 {
		db.append(json!({"index": i, "padding": "x".repeat(100)}))
			.unwrap();
	}
	// The first ten items are 124 bytes each: four fit in 500, a fifth does not.
	let fetched = db.fetch(100, 500).unwrap().unwrap();
	assert_eq!((fetched.len(), fetched.byte_len()), (4, 496));
	drop(fetched);
	assert_eq!(db.fetch(100, 495).unwrap().unwrap().len(), 3);
}

#[wasm_bindgen_test]
async fn test_transientdb_fetch_holds_items_back_until_released() {
	let db = db("test-fetch-holds-back").await;
	db.append(json!({"key": "value"})).unwrap();

	let items = db.fetch(10, 1024).unwrap().unwrap();
	// Still stored, but not handed out again.
	assert!(!db.has_data());
	assert!(db.fetch(10, 1024).unwrap().is_none());
	drop(items);
	assert!(db.has_data(), "a dropped fetch gives its items back");
}

#[wasm_bindgen_test]
async fn test_transientdb_pending_items_are_available_after_a_reload() {
	let db_name = "test-transient-reload";
	let first = db(db_name).await;
	for i in 0..3 {
		first.append(json!({"index": i})).unwrap();
	}
	let held = first.fetch(10, 1024).unwrap().unwrap();
	settle().await;

	let second = TransientDB::new(WebStore::new(test_config(db_name)).await);
	assert_eq!(available(&second).len(), 3, "pending is not persisted");
	drop(held);
	first.reset();
	settle().await;
}

// =============================================================================
// Remove Operations
// =============================================================================

#[wasm_bindgen_test]
async fn test_transientdb_remove() {
	let db = db("test-remove").await;
	db.append(json!({"test": "data"})).unwrap();
	let items = db.fetch(10, 1024).unwrap().unwrap();
	db.remove(items).unwrap();
	assert!(!db.has_data(), "Should be empty after remove");
	assert!(available(&db).is_empty());
}

#[wasm_bindgen_test]
async fn test_transientdb_partial_remove() {
	let db = db("test-partial-remove").await;
	for i in 0..10 {
		db.append(json!({"index": i})).unwrap();
	}
	let items = db.fetch(3, 64 * 1024).unwrap().unwrap();
	db.remove(items).unwrap();

	let remaining = available(&db);
	assert_eq!(remaining.len(), 7, "Should have 7 items remaining");
	assert_eq!(remaining[0]["index"], 3);
}

// =============================================================================
// Reset Operations
// =============================================================================

#[wasm_bindgen_test]
async fn test_transientdb_reset() {
	let db = db("test-reset").await;
	for i in 0..5 {
		db.append(json!({"index": i})).unwrap();
	}
	let held = db.fetch(2, 1024).unwrap().unwrap();
	db.reset();
	assert!(!db.has_data(), "Should be empty after reset");
	// An Items from before the reset matches nothing.
	db.remove(held).unwrap();
	assert!(!db.has_data());
}

#[wasm_bindgen_test]
async fn test_transientdb_reset_then_append() {
	let db = db("test-reset-append").await;
	db.append(json!({"before": "reset"})).unwrap();
	db.reset();
	db.append(json!({"after": "reset"})).unwrap();
	assert_eq!(available(&db), vec![json!({"after": "reset"})]);
}

// =============================================================================
// FIFO Behavior
// =============================================================================

#[wasm_bindgen_test]
async fn test_transientdb_fifo_order() {
	let db = db("test-fifo").await;
	for i in 0..5 {
		db.append(json!({"order": i})).unwrap();
	}
	for (i, item) in available(&db).iter().enumerate() {
		assert_eq!(item["order"], i as i64, "Items should be in FIFO order");
	}
}

#[wasm_bindgen_test]
async fn test_transientdb_max_items_eviction() {
	let db = fresh_db(WebConfig {
		max_items: 5,
		..test_config("test-max-items")
	})
	.await;
	for i in 0..10 {
		db.append(json!({"index": i})).unwrap();
	}
	let items = available(&db);
	assert_eq!(items.len(), 5, "Should only have max_items");
	assert_eq!(items[0]["index"], 5);
	assert_eq!(items[4]["index"], 9);
}

// =============================================================================
// Rapid Operations (Stress-lite for single-threaded WASM)
// =============================================================================

#[wasm_bindgen_test]
async fn test_transientdb_rapid_append() {
	let db = db("test-rapid-append").await;
	for i in 0..100 {
		db.append(json!({"rapid": i})).unwrap();
	}
	assert_eq!(available(&db).len(), 100);
}

#[wasm_bindgen_test]
async fn test_transientdb_rapid_fetch_remove_cycle() {
	let db = db("test-rapid-cycle").await;
	for cycle in 0..10 {
		for i in 0..10 {
			db.append(json!({"cycle": cycle, "item": i})).unwrap();
		}
		while let Some(items) = db.fetch(5, 64 * 1024).unwrap() {
			db.remove(items).unwrap();
		}
	}
	assert!(!db.has_data(), "Should be empty after all cycles");
}

#[wasm_bindgen_test]
async fn test_transientdb_interleaved_operations() {
	let db = db("test-interleaved").await;
	// Interleave appends and partial removes: every 5 items, remove 2
	for i in 0..20 {
		db.append(json!({"index": i})).unwrap();
		if i > 0 && i % 5 == 0 {
			let items = db.fetch(2, 64 * 1024).unwrap().unwrap();
			db.remove(items).unwrap();
		}
	}
	// Three rounds of two removed: the oldest six.
	let left: Vec<i64> = available(&db)
		.iter()
		.map(|v| v["index"].as_i64().unwrap())
		.collect();
	assert_eq!(left, (6..20).collect::<Vec<_>>());
}

#[wasm_bindgen_test]
async fn test_transientdb_large_store_never_drops() {
	let db = fresh_db(WebConfig {
		max_items: 3000,
		..test_config("test-transientdb-large")
	})
	.await;
	for i in 0..3000 {
		db.append(json!({"index": i})).unwrap();
	}
	let mut seen = Vec::new();
	while let Some(items) = db.fetch(256, 16 * 1024).unwrap() {
		seen.extend(items.values().iter().map(|v| v["index"].as_i64().unwrap()));
		db.remove(items).unwrap();
	}
	assert_eq!(seen, (0..3000).collect::<Vec<_>>());
	assert!(!db.has_data());
}

#[wasm_bindgen_test]
async fn test_transientdb_fetch_count_walks_the_store() {
	let db = db("test-transient-count-walk").await;
	for i in 0..20 {
		db.append(json!({"index": i})).unwrap();
	}
	let mut sizes = Vec::new();
	while let Some(items) = db.fetch(8, 64 * 1024).unwrap() {
		sizes.push(items.len());
		db.remove(items).unwrap();
	}
	assert_eq!(sizes, vec![8, 8, 4]);
	assert!(!db.has_data());
}
