//! Size limits through the TransientDB wrapper, for MemoryStore: `max_item_size` on
//! append, and `max_bytes` on every fetch, also under concurrent appends.

#![cfg(not(target_arch = "wasm32"))]

use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;
use std::thread;
use transientdb::{Error, MemoryConfig, MemoryStore, Result, TransientDB};

fn memory_db(max_item_size: usize) -> TransientDB {
	TransientDB::new(MemoryStore::new(MemoryConfig {
		max_item_size,
		max_items: 100_000,
	}))
}

/// Fetches and removes until empty, checking each fetch is within `max_bytes`, and that
/// the request body a caller would build from it is exactly what its length promises.
fn drained(db: &TransientDB, max_bytes: usize) -> Result<Vec<Value>> {
	let mut items = Vec::new();
	while let Some(fetched) = db.fetch(usize::MAX, max_bytes)? {
		assert!(
			fetched.byte_len() <= max_bytes,
			"{} bytes",
			fetched.byte_len()
		);
		let json = fetched.json();
		assert_eq!(json.len(), fetched.byte_len() + fetched.len() + 1);
		let body: Value = serde_json::from_str(&format!("{{\"items\":{json}}}")).unwrap();
		assert_eq!(body["items"], Value::Array(fetched.values()));
		items.extend(fetched.values());
		db.remove(fetched)?;
	}
	assert!(!db.has_data());
	Ok(items)
}

#[test]
fn test_memory_oversized_append_through_transientdb() -> Result<()> {
	let db = memory_db(200);
	db.append(json!({"i": 0}))?;
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
	assert!(err.to_string().contains("max_item_size = 200"), "{err}");

	let mut expected = vec![json!({"i": 0})];
	for i in 1..20 {
		db.append(json!({"i": i, "pad": "p".repeat(i * 3)}))?;
		expected.push(json!({"i": i, "pad": "p".repeat(i * 3)}));
	}
	assert_eq!(drained(&db, 200)?, expected);
	Ok(())
}

#[test]
fn test_memory_concurrent_appends_respect_limits() -> Result<()> {
	let db = Arc::new(memory_db(512));
	let handles: Vec<_> = (0..8)
		.map(|t| {
			let db = db.clone();
			thread::spawn(move || {
				for i in 0..100 {
					db.append(json!({"t": t, "i": i, "s": "漢字".repeat(i % 9)}))
						.unwrap();
					if i % 10 == 0 {
						let big = json!({"t": t, "big": "x".repeat(600)});
						let err = db.append(big).unwrap_err();
						assert!(matches!(err, Error::ItemTooLarge { max: 512, .. }));
					}
				}
			})
		})
		.collect();
	for handle in handles {
		handle.join().unwrap();
	}

	let items = drained(&db, 512)?;
	assert_eq!(items.len(), 800);
	let unique: HashSet<(i64, i64)> = items
		.iter()
		.map(|v| (v["t"].as_i64().unwrap(), v["i"].as_i64().unwrap()))
		.collect();
	assert_eq!(unique.len(), 800);
	Ok(())
}

#[test]
fn test_max_bytes_smaller_than_the_oldest_item_hands_over_nothing() -> Result<()> {
	let db = memory_db(512);
	db.append(json!("x".repeat(100)))?;
	db.append(json!(1))?;
	assert!(db.fetch(10, 101)?.is_none());
	assert!(db.has_data());
	assert_eq!(db.fetch(10, 102)?.unwrap().len(), 1);
	assert_eq!(db.fetch(10, 103)?.unwrap().len(), 2);
	Ok(())
}
