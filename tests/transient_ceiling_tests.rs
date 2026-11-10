//! Byte-ceiling behavior through the TransientDB wrapper, for the native stores.

#![cfg(not(target_arch = "wasm32"))]

use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{ErrorKind, Result};
use std::path::PathBuf;
use std::sync::Arc;
use std::{fs, thread};
use tempfile::TempDir;
use transientdb::{
	DirectoryConfig, DirectoryStore, Envelope, MemoryConfig, MemoryStore, TransientDB,
};

fn directory_db(dir: &TempDir, max_file_size: usize) -> Result<TransientDB<Vec<PathBuf>>> {
	Ok(TransientDB::new(DirectoryStore::new(DirectoryConfig {
		envelope: Envelope::new("items")
			.field("source", "test-key")
			.timestamp("sentAt"),
		storage_location: dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size,
		max_file_items: None,
	})?))
}

fn memory_db(max_fetch_size: usize) -> TransientDB<Value> {
	TransientDB::new(MemoryStore::new(MemoryConfig {
		envelope: Envelope::default(),
		max_items: Some(100_000),
		max_fetch_size,
		max_fetch_items: None,
	}))
}

/// Fetches every finished file, checks each is within `max` bytes and well formed,
/// and returns all items in file order.
fn directory_items(db: &TransientDB<Vec<PathBuf>>, max: usize) -> Result<Vec<Value>> {
	let Some(result) = db.fetch(None, None)? else {
		return Ok(Vec::new());
	};
	let mut items = Vec::new();
	for file in result.data.unwrap() {
		let len = fs::metadata(&file)?.len();
		assert!(len <= max as u64, "{file:?} is {len} bytes, over {max}");
		let parsed: Value = serde_json::from_str(&fs::read_to_string(&file)?)?;
		assert!(parsed["sentAt"].is_string());
		assert_eq!(parsed["source"], "test-key");
		let batch = parsed["items"].as_array().unwrap();
		assert!(!batch.is_empty());
		items.extend(batch.iter().cloned());
	}
	db.remove(&result.removable.unwrap())?;
	Ok(items)
}

/// Fetches and removes batches until empty, checking each is within `max` bytes.
fn memory_items(db: &TransientDB<Value>, max: usize) -> Result<Vec<Value>> {
	let mut items = Vec::new();
	while let Some(result) = db.fetch(None, None)? {
		let batch = result.data.unwrap();
		let len = serde_json::to_string(&batch)?.len();
		assert!(len <= max, "batch is {len} bytes, over {max}");
		items.extend(batch["items"].as_array().unwrap().iter().cloned());
		db.remove(&result.removable.unwrap())?;
	}
	assert!(!db.has_data());
	Ok(items)
}

#[test]
fn test_directory_oversized_append_through_transientdb() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let db = directory_db(&temp_dir, 200)?;
	db.append(json!({"i": 0}))?;
	let err = db.append(json!({"big": "x".repeat(500)})).unwrap_err();
	assert_eq!(err.kind(), ErrorKind::InvalidInput);
	assert!(err.to_string().contains("max_file_size"), "{err}");

	let mut expected = vec![json!({"i": 0})];
	for i in 1..20 {
		db.append(json!({"i": i, "pad": "p".repeat(i * 3)}))?;
		expected.push(json!({"i": i, "pad": "p".repeat(i * 3)}));
	}
	assert_eq!(directory_items(&db, 200)?, expected);
	assert!(!db.has_data());
	Ok(())
}

#[test]
fn test_directory_concurrent_appends_respect_ceiling() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let db = Arc::new(directory_db(&temp_dir, 512)?);
	let handles: Vec<_> = (0..8)
		.map(|t| {
			let db = db.clone();
			thread::spawn(move || {
				for i in 0..100 {
					let item = json!({"t": t, "i": i, "s": "🦀".repeat(i % 9)});
					db.append(item).unwrap();
					if i % 10 == 0 {
						// Oversized items are rejected without disturbing other threads.
						let big = json!({"t": t, "big": "x".repeat(600)});
						assert_eq!(db.append(big).unwrap_err().kind(), ErrorKind::InvalidInput);
					}
				}
			})
		})
		.collect();
	for handle in handles {
		handle.join().unwrap();
	}

	let items = directory_items(&db, 512)?;
	assert_eq!(items.len(), 800);
	let unique: HashSet<(i64, i64)> = items
		.iter()
		.map(|v| (v["t"].as_i64().unwrap(), v["i"].as_i64().unwrap()))
		.collect();
	assert_eq!(unique.len(), 800);
	// Each thread's items stay in its own append order.
	for t in 0..8 {
		let order: Vec<i64> = items
			.iter()
			.filter(|v| v["t"] == t)
			.map(|v| v["i"].as_i64().unwrap())
			.collect();
		assert_eq!(order, (0..100).collect::<Vec<_>>());
	}
	Ok(())
}

#[test]
fn test_memory_oversized_append_through_transientdb() -> Result<()> {
	let db = memory_db(200);
	db.append(json!({"i": 0}))?;
	let err = db.append(json!({"big": "x".repeat(500)})).unwrap_err();
	assert_eq!(err.kind(), ErrorKind::InvalidInput);
	assert!(err.to_string().contains("max_fetch_size"), "{err}");

	let mut expected = vec![json!({"i": 0})];
	for i in 1..20 {
		db.append(json!({"i": i, "pad": "p".repeat(i * 3)}))?;
		expected.push(json!({"i": i, "pad": "p".repeat(i * 3)}));
	}
	assert_eq!(memory_items(&db, 200)?, expected);
	Ok(())
}

#[test]
fn test_memory_concurrent_appends_respect_ceiling() -> Result<()> {
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
						assert_eq!(db.append(big).unwrap_err().kind(), ErrorKind::InvalidInput);
					}
				}
			})
		})
		.collect();
	for handle in handles {
		handle.join().unwrap();
	}

	let items = memory_items(&db, 512)?;
	assert_eq!(items.len(), 800);
	let unique: HashSet<(i64, i64)> = items
		.iter()
		.map(|v| (v["t"].as_i64().unwrap(), v["i"].as_i64().unwrap()))
		.collect();
	assert_eq!(unique.len(), 800);
	Ok(())
}
