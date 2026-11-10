//! Unit tests for TransientDB - moved from src/transient.rs
//!
//! These tests verify the thread-safety and correctness of TransientDB
//! with both MemoryStore and DirectoryStore backends.

#![cfg(not(target_arch = "wasm32"))]

use rand::Rng;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::Result;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::{fs, thread};
use tempfile::TempDir;
use transientdb::{
	DirectoryConfig, DirectoryStore, Envelope, MemoryConfig, MemoryStore, TransientDB,
};

#[test]
fn test_concurrent_appends() -> Result<()> {
	let config = MemoryConfig {
		envelope: Envelope::default(),
		max_items: Some(10000),
		max_fetch_size: 1024,
		max_fetch_items: None,
	};
	let store = MemoryStore::new(config);
	let db = Arc::new(TransientDB::new(store));
	let mut handles = vec![];

	// Track total successful appends
	let append_count = Arc::new(AtomicUsize::new(0));

	// Create a clone for final verification
	let final_db = db.clone();

	// Spawn 10 threads that each append 100 items
	for t in 0..10 {
		let db = db.clone();
		let append_counter = append_count.clone();
		let handle = thread::spawn(move || {
			for i in 0..100 {
				let event = json!({
					"thread": t,
					"index": i,
					"data": "test data"
				});
				if db.append(event).is_ok() {
					append_counter.fetch_add(1, Ordering::SeqCst);
				}
				if i % 10 == 0 {
					thread::sleep(Duration::from_millis(1));
				}
			}
		});
		handles.push(handle);
	}

	// Wait for all threads to complete
	for handle in handles {
		handle.join().unwrap();
	}

	// Verify we got a reasonable number of successful appends
	let total_appends = append_count.load(Ordering::SeqCst);
	assert!(total_appends > 0, "No successful appends recorded");

	// Final verification that data exists
	assert!(final_db.has_data(), "Should have at least some data");

	Ok(())
}

#[test]
fn test_concurrent_append_and_fetch() -> Result<()> {
	let config = MemoryConfig {
		envelope: Envelope::default(),
		max_items: Some(1000),
		max_fetch_size: 1024,
		max_fetch_items: None,
	};

	let store = MemoryStore::new(config);
	let db = Arc::new(TransientDB::new(store));

	// Spawn thread for continuous appending
	let db_append = db.clone();
	let append_handle = thread::spawn(move || {
		for i in 0..500 {
			let event = json!({"index": i});
			db_append.append(event).unwrap();
			thread::sleep(std::time::Duration::from_micros(10));
		}
	});

	// Spawn thread for continuous fetching
	let db_fetch = db.clone();
	let fetch_handle = thread::spawn(move || {
		let mut total_fetched = 0;
		while total_fetched < 100 {
			if let Ok(Some(result)) = db_fetch.fetch(Some(10), None) {
				let batch: Value = result.data.unwrap();
				let items = batch["items"].as_array().unwrap();
				total_fetched += items.len();
			}
			thread::sleep(std::time::Duration::from_millis(1));
		}
		total_fetched
	});

	// Wait for both operations to complete
	append_handle.join().unwrap();
	let total_fetched = fetch_handle.join().unwrap();
	assert!(
		total_fetched >= 100,
		"Should have fetched at least 100 items"
	);

	Ok(())
}

#[test]
fn test_concurrent_reset() -> Result<()> {
	let config = MemoryConfig {
		envelope: Envelope::default(),
		max_items: Some(1000),
		max_fetch_size: 1024,
		max_fetch_items: None,
	};
	let store = MemoryStore::new(config);
	let db = Arc::new(TransientDB::new(store));

	// Fill with some initial data
	for i in 0..100 {
		db.append(json!({"index": i}))?;
	}

	// Spawn threads for concurrent operations
	let mut handles = vec![];

	// Thread that resets
	let db_reset = db.clone();
	handles.push(thread::spawn(move || {
		thread::sleep(std::time::Duration::from_millis(50));
		db_reset.reset();
	}));

	// Thread that appends
	let db_append = db.clone();
	handles.push(thread::spawn(move || {
		for i in 0..100 {
			let event = json!({"new_index": i});
			let _ = db_append.append(event);
			thread::sleep(std::time::Duration::from_micros(100));
		}
	}));

	// Thread that fetches
	let db_fetch = db.clone();
	handles.push(thread::spawn(move || {
		let mut _fetch_count = 0;
		for _ in 0..10 {
			if let Ok(Some(result)) = db_fetch.fetch(None, None) {
				let batch: Value = result.data.unwrap();
				let items = batch["items"].as_array().unwrap();
				_fetch_count += items.len();
			}
			thread::sleep(std::time::Duration::from_millis(10));
		}
	}));

	// Wait for all operations to complete
	for handle in handles {
		handle.join().unwrap();
	}

	// The final state should be consistent (no panics or deadlocks)
	// everything has been handled, so there's no data.
	assert!(!db.has_data());

	Ok(())
}

#[test]
fn test_concurrent_directory_store() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let config = DirectoryConfig {
		envelope: Envelope::default(),
		storage_location: temp_dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size: 200, // Small size to force rotation
		max_file_items: None,
	};

	let store = DirectoryStore::new(config)?;
	let db = Arc::new(TransientDB::new(store));

	// Spawn multiple threads for concurrent operations
	let mut handles = vec![];

	// Thread for appending
	let db_append = db.clone();
	handles.push(thread::spawn(move || {
		for i in 0..50 {
			let event = json!({
				"index": i,
				"data": "some test data to force file rotation"
			});
			db_append.append(event).unwrap();
			thread::sleep(std::time::Duration::from_millis(1));
		}
	}));

	// Thread for fetching
	let db_fetch = db.clone();
	handles.push(thread::spawn(move || {
		let mut _total_files = 0;
		for _ in 0..5 {
			if let Ok(Some(result)) = db_fetch.fetch(None, None) {
				if let Some(files) = result.data {
					_total_files += files.len();
				}
			}
			thread::sleep(std::time::Duration::from_millis(10));
		}
	}));

	// Thread for removing files
	let db_remove = db.clone();
	handles.push(thread::spawn(move || {
		for _ in 0..5 {
			if let Ok(Some(result)) = db_remove.fetch(Some(2), None) {
				if let Some(removable) = result.removable {
					db_remove.remove(&removable).unwrap();
				}
			}
			thread::sleep(std::time::Duration::from_millis(10));
		}
	}));

	// Wait for all operations to complete
	for handle in handles {
		handle.join().unwrap();
	}

	// Verify final state
	let final_files = std::fs::read_dir(temp_dir.path())?
		.filter_map(Result::ok)
		.count();

	// We should have some files but not all (due to removal)
	assert!(final_files < 50, "Some files should have been removed");
	assert!(final_files > 0, "Should still have some files");

	Ok(())
}

#[test]
fn test_directory_store_stress() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let config = DirectoryConfig {
		envelope: Envelope::default(),
		storage_location: temp_dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size: 1024 * 10, // 10KB files to force lots of rotation
		max_file_items: None,
	};

	let store = DirectoryStore::new(config)?;
	let db = Arc::new(TransientDB::new(store));
	let mut handles = vec![];

	// Spawn 30 append threads
	for t in 0..30 {
		let db = db.clone();
		handles.push(thread::spawn(move || {
			for i in 0..1000 {
				let event = json!({
					"thread": t,
					"index": i,
					"timestamp": chrono::Utc::now().to_rfc3339(),
					"data": "padding data to force file rotations frequently..."
				});
				db.append(event).unwrap();
				if i % 100 == 0 {
					thread::sleep(std::time::Duration::from_micros(100));
				}
			}
		}));
	}

	// Spawn 10 aggressive fetch/remove threads
	for _ in 0..10 {
		let db = db.clone();
		handles.push(thread::spawn(move || {
			for _ in 0..50 {
				if let Ok(Some(result)) = db.fetch(Some(5), None) {
					if let Some(removable) = result.removable {
						db.remove(&removable).unwrap();
					}
				}
				thread::sleep(std::time::Duration::from_millis(10));
			}
		}));
	}

	// Single thread doing periodic full fetches
	let db_fetch = db.clone();
	handles.push(thread::spawn(move || {
		for _ in 0..10 {
			if let Ok(Some(result)) = db_fetch.fetch(None, Some(1024 * 1024)) {
				if let Some(removable) = result.removable {
					db_fetch.remove(&removable).unwrap();
				}
			}
			thread::sleep(std::time::Duration::from_millis(100));
		}
	}));

	// Wait for all operations to complete
	for handle in handles {
		handle.join().unwrap();
	}

	// Try one final operation of each type
	db.append(json!({"final": "test"}))?;
	if let Some(result) = db.fetch(None, None)? {
		if let Some(removable) = result.removable {
			db.remove(&removable)?;
		}
	}

	Ok(())
}

#[test]
fn test_memory_store_chaos() -> Result<()> {
	let config = MemoryConfig {
		envelope: Envelope::default(),
		max_items: Some(100_000),
		max_fetch_size: 1024 * 1024,
		max_fetch_items: None,
	};

	let store = MemoryStore::new(config);
	let db = Arc::new(TransientDB::new(store));
	let mut handles = vec![];

	// Create a clone for final verification
	let final_db = db.clone();

	// Evil JSON generator (remains unchanged)
	let generate_evil_json = |i: u64| -> Value {
		match i % 5 {
			0 => json!({
				"normal": "boring",
				"timestamp": chrono::Utc::now().to_rfc3339(),
			}),
			1 => json!({
				"nested": {
					"deeply": {
						"nested": {
							"value": "here",
							"with": ["arrays", "of", "doom", "that", "go", "on", "forever"]
						}
					}
				}
			}),
			2 => json!({
				"unicode": "🦀💥👾👿",
				"weird_spaces": "    \n\t\r    ",
				"empty": "",
			}),
			3 => {
				let mut huge = json!({});
				for n in 0..100 {
					huge[format!("field_{}", n)] = json!("value");
				}
				huge
			}
			_ => {
				let mut tiny = json!({});
				tiny["x"] = json!("y");
				tiny
			}
		}
	};

	let random_delay = || {
		let mut rng = rand::thread_rng();
		if rng.gen_bool(0.2) {
			thread::sleep(std::time::Duration::from_millis(rng.gen_range(0..50)));
		}
	};

	// Spawn heavy append threads
	for _t in 0..30 {
		let db = db.clone();
		handles.push(thread::spawn(move || {
			let mut rng = rand::thread_rng();
			for i in 0..1000 {
				let evil_json = generate_evil_json(i as u64);
				random_delay();
				let _ = db.append(evil_json);

				if i % 100 == 0 {
					let _ = db.fetch(Some(rng.gen_range(0..10)), None);
				}
			}
		}));
	}

	// Spawn chaos threads
	for _ in 0..15 {
		let db = db.clone();
		handles.push(thread::spawn(move || {
			let mut rng = rand::thread_rng();
			for _ in 0..200 {
				match rng.gen_range(0..3) {
					0 => {
						db.reset();
					}
					1 => {
						// Random sized fetches
						let count = Some(rng.gen_range(0..1000));
						let bytes = Some(rng.gen_range(0..(1024 * 100)));
						let _ = db.fetch(count, bytes);
					}
					_ => {
						// Rapid-fire tiny appends
						for _ in 0..50 {
							let tiny_json = json!({"x": "y"});
							let _ = db.append(tiny_json);
						}
					}
				}
				random_delay();
			}
		}));
	}

	for handle in handles {
		handle.join().unwrap();
	}

	// Use final_db for a final operation to verify functionality
	final_db.append(json!({"final": "test"}))?;
	assert!(
		final_db.has_data(),
		"Store should have data after final append"
	);

	Ok(())
}

#[test]
fn test_directory_store_chaos() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let config = DirectoryConfig {
		envelope: Envelope::default(),
		storage_location: temp_dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size: 1024 * 2, // Tiny 2KB files for maximum pain
		max_file_items: None,
	};

	let store = DirectoryStore::new(config)?;
	let db = Arc::new(TransientDB::new(store));
	let mut handles = vec![];

	// Evil JSON generator (same as before)
	let generate_evil_json = |i: u64| -> Value {
		match i % 5 {
			0 => json!({
				"normal": "boring",
				"timestamp": chrono::Utc::now().to_rfc3339(),
			}),
			1 => json!({
				"nested": {
					"deeply": {
						"nested": {
							"value": "here",
							"with": ["arrays", "of", "doom", "that", "go", "on", "forever"]
						}
					}
				}
			}),
			2 => json!({
				"unicode": "🦀💥👾👿",
				"weird_spaces": "    \n\t\r    ",
				"empty": "",
			}),
			3 => {
				let mut huge = json!({});
				for n in 0..100 {
					huge[format!("field_{}", n)] = json!("value");
				}
				huge
			}
			_ => {
				let mut tiny = json!({});
				tiny["x"] = json!("y");
				tiny
			}
		}
	};

	let random_delay = || {
		let mut rng = rand::thread_rng();
		if rng.gen_bool(0.2) {
			thread::sleep(std::time::Duration::from_millis(rng.gen_range(0..50)));
		}
	};

	// Spawn append threads
	for _t in 0..20 {
		let db = db.clone();
		handles.push(thread::spawn(move || {
			let mut rng = rand::thread_rng();
			for i in 0..500 {
				let evil_json = generate_evil_json(i as u64);
				random_delay();
				let _ = db.append(evil_json);

				if i % 50 == 0 {
					if let Ok(Some(result)) = db.fetch(Some(rng.gen_range(0..5)), None) {
						if let Some(removable) = result.removable {
							let _ = db.remove(&removable);
						}
					}
				}
			}
		}));
	}

	// Monitoring thread checks directory still exists
	let temp_dir_path = temp_dir.path().to_owned();
	handles.push(thread::spawn(move || {
		for _ in 0..1000 {
			assert!(temp_dir_path.exists(), "Temp directory was removed");
			thread::sleep(std::time::Duration::from_millis(1));
		}
	}));

	// Wait for all threads to finish
	for handle in handles {
		handle.join().unwrap();
	}

	// Final verification - just check we can still append
	db.append(json!({"final": "test"}))?;

	// Keep temp_dir alive until end
	assert!(temp_dir.path().exists());

	Ok(())
}

#[test]
fn test_directory_store_malformed_files() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let config = DirectoryConfig {
		envelope: Envelope::default(),
		storage_location: temp_dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size: 1024,
		max_file_items: None,
	};

	let store = DirectoryStore::new(config)?;
	let db = Arc::new(TransientDB::new(store));

	// First add some valid data
	db.append(json!({"valid": "data"}))?;

	// Force a fetch to complete the current file
	let _ = db.fetch(None, None)?;

	// Create some malformed files in the directory with .temp extension
	let malformed_path = temp_dir.path().join("999-events.temp");
	std::fs::write(&malformed_path, "{ invalid json")?;

	let empty_path = temp_dir.path().join("998-events.temp");
	std::fs::write(&empty_path, "")?;

	// Add more valid data
	db.append(json!({"more": "valid data"}))?;

	// Verify the store is still functional
	assert!(db.has_data(), "Store should still have data");

	// Check if we can still fetch data
	if let Some(result) = db.fetch(None, None)? {
		if let Some(files) = result.data {
			assert!(!files.is_empty(), "Should have at least one file");

			// Count valid files
			let valid_files = files
				.iter()
				.filter(|f| {
					if let Ok(content) = std::fs::read_to_string(f) {
						content.starts_with("{\"items\":[")
					} else {
						false
					}
				})
				.count();

			assert!(valid_files > 0, "Should have at least one valid file");
		}
	}

	Ok(())
}

#[test]
fn test_transient_db_panic_recovery() -> Result<()> {
	use std::panic::{catch_unwind, AssertUnwindSafe};

	let config = MemoryConfig {
		envelope: Envelope::default(),
		max_items: Some(100),
		max_fetch_size: 1024,
		max_fetch_items: None,
	};

	let store = MemoryStore::new(config);
	let db = Arc::new(TransientDB::new(store));

	// Simulate a panic in one thread
	let db_panic = db.clone();
	let panic_result = catch_unwind(AssertUnwindSafe(|| {
		let _ = db_panic.append(json!({"before": "panic"}));
		panic!("Simulated panic");
	}));
	assert!(panic_result.is_err());

	// DB should still be usable
	db.append(json!({"after": "panic"}))?;
	assert!(db.has_data());

	Ok(())
}

#[test]
fn test_directory_store_concurrent_validation() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let config = DirectoryConfig {
		envelope: Envelope::default(),
		storage_location: temp_dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size: 256, // Small size to force more file rotations
		max_file_items: None,
	};

	let mut store = DirectoryStore::new(config)?;

	// Track validation calls with atomic counter
	let validation_count = Arc::new(AtomicUsize::new(0));
	let validation_count_clone = validation_count.clone();

	store.set_file_validator(move |_| {
		validation_count_clone.fetch_add(1, Ordering::SeqCst);
		thread::sleep(Duration::from_millis(50));
		Ok(())
	});

	let db = Arc::new(TransientDB::new(store));
	let append_count = Arc::new(AtomicUsize::new(0));
	let mut handles = vec![];

	// Multiple append threads
	for thread_id in 0..3 {
		let db_append = db.clone();
		let append_counter = append_count.clone();
		handles.push(thread::spawn(move || {
			for i in 0..10 {
				let event = json!({
					"thread": thread_id,
					"index": i,
					"value": format!("data_{}_{}",thread_id, i)
				});
				if let Ok(()) = db_append.append(event) {
					append_counter.fetch_add(1, Ordering::SeqCst);
				}
				// Small sleep to reduce contention
				thread::sleep(Duration::from_millis(1));
			}
		}));
	}

	// Multiple fetch threads
	for _ in 0..2 {
		let db_fetch = db.clone();
		handles.push(thread::spawn(move || {
			for _ in 0..5 {
				if let Ok(Some(result)) = db_fetch.fetch(Some(2), None) {
					if let Some(removable) = result.removable {
						let _ = db_fetch.remove(&removable);
					}
				}
				thread::sleep(Duration::from_millis(10));
			}
		}));
	}

	// Monitor thread to check file uniqueness
	let _db_monitor = db.clone();
	let temp_dir_path = temp_dir.path().to_owned();
	handles.push(thread::spawn(move || {
		for _ in 0..10 {
			let files: Vec<_> = fs::read_dir(&temp_dir_path)
				.unwrap()
				.filter_map(Result::ok)
				.map(|e| e.path())
				.filter(|p| p.extension().and_then(|ext| ext.to_str()) == Some("temp"))
				.collect();

			// Check for unique indices
			let mut seen_indices = HashSet::new();
			for file in files {
				if let Some(file_name) = file.file_name().and_then(|n| n.to_str()) {
					if let Some(num_str) = file_name.split('-').next() {
						if let Ok(num) = num_str.parse::<u32>() {
							assert!(
								seen_indices.insert(num),
								"Duplicate file index found: {}",
								num
							);
						}
					}
				}
			}
			thread::sleep(Duration::from_millis(20));
		}
	}));

	// Wait for all operations to complete
	for handle in handles {
		handle.join().unwrap();
	}

	// Verify final state
	assert!(
		validation_count.load(Ordering::SeqCst) > 0,
		"No validations occurred"
	);
	assert!(
		append_count.load(Ordering::SeqCst) > 0,
		"No appends succeeded"
	);

	// Final fetch to verify data integrity
	if let Some(result) = db.fetch(None, None)? {
		if let Some(files) = result.data {
			// Verify file contents
			let mut seen_events = HashSet::new();
			for file in &files {
				let content = fs::read_to_string(file)?;
				let value: Value = serde_json::from_str(&content)?;
				if let Some(batch) = value["items"].as_array() {
					for event in batch {
						if let (Some(thread_id), Some(index)) =
							(event["thread"].as_i64(), event["index"].as_i64())
						{
							seen_events.insert((thread_id, index));
						}
					}
				}
			}

			// Check we got events from at least one thread
			let thread_counts: HashSet<_> =
				seen_events.iter().map(|(thread_id, _)| thread_id).collect();
			assert!(!thread_counts.is_empty(), "No events were recorded");
		}
	}

	Ok(())
}

#[cfg_attr(
	any(
		target_os = "tvos",
		target_os = "visionos",
		target_os = "watchos",
		target_os = "android"
	),
	ignore
)]
#[test]
fn test_heavy_concurrent_load() -> Result<()> {
	let config = MemoryConfig {
		envelope: Envelope::default(),
		max_items: Some(100_000),
		max_fetch_size: 1024 * 1024,
		max_fetch_items: None,
	};
	let store = MemoryStore::new(config);
	let db = Arc::new(TransientDB::new(store));
	let mut handles = vec![];

	// Reduced number of threads and operations
	for t in 0..10 {
		// Reduced from 50 to 10
		let db = db.clone();
		handles.push(thread::spawn(move || {
			for i in 0..1_000 {
				// Reduced from 10_000 to 1_000
				let event = json!({
					"thread": t,
					"index": i,
					"timestamp": chrono::Utc::now().to_rfc3339(),
					"data": "some test data that takes up space and makes the json bigger"
				});
				db.append(event).unwrap();
			}
		}));
	}

	// Reduced fetch threads
	for _ in 0..5 {
		// Reduced from 20 to 5
		let db = db.clone();
		handles.push(thread::spawn(move || {
			let mut total_items = 0;
			while total_items < 1_000 {
				// Reduced from 10_000 to 1_000
				if let Ok(Some(result)) = db.fetch(Some(100), None) {
					let batch: Value = result.data.unwrap();
					let items = batch["items"].as_array().unwrap();
					total_items += items.len();
				}
				thread::sleep(Duration::from_micros(100));
			}
		}));
	}

	// Just one reset thread
	let db = db.clone();
	handles.push(thread::spawn(move || {
		for _ in 0..5 {
			// Reduced from 10 to 5
			thread::sleep(Duration::from_millis(100));
			db.reset();
		}
	}));

	// Wait for threads
	for handle in handles {
		handle.join().unwrap();
	}

	Ok(())
}

/// Drains a TransientDB backed by MemoryStore, returning the size of each fetched batch
/// and every item index seen, in order.
fn drain_memory_batches(db: &TransientDB<Value>) -> Result<(Vec<usize>, Vec<i64>)> {
	let mut sizes = Vec::new();
	let mut seen = Vec::new();
	while let Some(result) = db.fetch(None, None)? {
		let batch = result.data.unwrap();
		let items = batch["items"].as_array().unwrap();
		let removable = result.removable.unwrap();
		assert_eq!(items.len(), removable.len());
		sizes.push(items.len());
		seen.extend(items.iter().map(|v| v["index"].as_i64().unwrap()));
		db.remove(&removable)?;
	}
	Ok((sizes, seen))
}

#[test]
fn test_memory_fetch_item_limit_through_transientdb() -> Result<()> {
	let db = TransientDB::new(MemoryStore::new(MemoryConfig {
		envelope: Envelope::default(),
		max_items: Some(1000),
		max_fetch_size: 1024 * 1024,
		max_fetch_items: Some(8),
	}));
	for i in 0..20 {
		db.append(json!({"index": i}))?;
	}

	// A count above the limit is clamped, below it wins.
	let batch = db.fetch(Some(100), None)?.unwrap().data.unwrap();
	assert_eq!(batch["items"].as_array().unwrap().len(), 8);
	let batch = db.fetch(Some(3), None)?.unwrap().data.unwrap();
	assert_eq!(batch["items"].as_array().unwrap().len(), 3);

	let (sizes, seen) = drain_memory_batches(&db)?;
	assert_eq!(sizes, vec![8, 8, 4]);
	assert_eq!(seen, (0..20).collect::<Vec<_>>());
	assert!(!db.has_data());
	Ok(())
}

#[test]
fn test_directory_file_item_limit_through_transientdb() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let db = TransientDB::new(DirectoryStore::new(DirectoryConfig {
		envelope: Envelope::new("items")
			.field("source", "test-key")
			.timestamp("sentAt"),
		storage_location: temp_dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size: 1024 * 1024,
		max_file_items: Some(8),
	})?);
	for i in 0..20 {
		db.append(json!({"index": i}))?;
	}

	let result = db.fetch(None, None)?.unwrap();
	let files = result.data.unwrap();
	let mut counts = Vec::new();
	let mut seen = Vec::new();
	for file in &files {
		let parsed: Value = serde_json::from_str(&fs::read_to_string(file)?)?;
		assert!(parsed["sentAt"].is_string());
		assert_eq!(parsed["source"], "test-key");
		let batch = parsed["items"].as_array().unwrap();
		counts.push(batch.len());
		seen.extend(batch.iter().map(|v| v["index"].as_i64().unwrap()));
	}
	assert_eq!(counts, vec![8, 8, 4]);
	assert_eq!(seen, (0..20).collect::<Vec<_>>());

	db.remove(&result.removable.unwrap())?;
	assert!(!db.has_data());
	Ok(())
}

#[test]
fn test_directory_file_item_limit_concurrent_appends() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let db = Arc::new(TransientDB::new(DirectoryStore::new(DirectoryConfig {
		envelope: Envelope::default(),
		storage_location: temp_dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size: 1024 * 1024,
		max_file_items: Some(8),
	})?));

	let handles: Vec<_> = (0..4)
		.map(|t| {
			let db = db.clone();
			thread::spawn(move || {
				for i in 0..50 {
					db.append(json!({"thread": t, "index": i})).unwrap();
				}
			})
		})
		.collect();
	for handle in handles {
		handle.join().unwrap();
	}

	let files = db.fetch(None, None)?.unwrap().data.unwrap();
	let mut total = 0;
	let mut seen = HashSet::new();
	for file in &files {
		let parsed: Value = serde_json::from_str(&fs::read_to_string(file)?)?;
		let batch = parsed["items"].as_array().unwrap();
		assert!(
			!batch.is_empty() && batch.len() <= 8,
			"bad batch size {}",
			batch.len()
		);
		total += batch.len();
		for item in batch {
			seen.insert((item["thread"].as_i64(), item["index"].as_i64()));
		}
	}
	assert_eq!(total, 200);
	assert_eq!(seen.len(), 200);
	assert_eq!(files.len(), 25);
	Ok(())
}

#[test]
fn test_removables_pass_through_to_the_one_wrapped_store() -> Result<()> {
	let config = MemoryConfig {
		envelope: Envelope::default(),
		max_items: Some(100),
		max_fetch_size: 4096,
		max_fetch_items: None,
	};
	let (a, b) = (
		TransientDB::new(MemoryStore::new(config.clone())),
		TransientDB::new(MemoryStore::new(config)),
	);
	for i in 0..4 {
		a.append(json!({"i": i}))?;
		b.append(json!({"i": i}))?;
	}
	let batch = |db: &TransientDB<Value>| -> Result<Vec<Value>> {
		Ok(db.fetch(None, None)?.unwrap().data.unwrap()["items"]
			.as_array()
			.unwrap()
			.clone())
	};
	let all = batch(&a)?;

	// Its own removables remove exactly what it fetched, as the wrapped store does.
	let removable = a.fetch(Some(2), None)?.unwrap().removable.unwrap();
	a.remove(&removable)?;
	assert_eq!(batch(&a)?, all[2..]);
	// Another TransientDB's store ignores them.
	b.remove(&removable)?;
	assert_eq!(batch(&b)?, all);
	Ok(())
}

#[test]
fn test_unbounded_memory_store_never_drops_under_concurrent_appends() -> Result<()> {
	let store = MemoryStore::new(MemoryConfig {
		envelope: Envelope::default(),
		max_items: None,
		max_fetch_size: 16 * 1024,
		max_fetch_items: None,
	});
	let db = Arc::new(TransientDB::new(store));
	const THREADS: usize = 8;
	const PER_THREAD: usize = 500;

	let handles: Vec<_> = (0..THREADS)
		.map(|t| {
			let db = db.clone();
			thread::spawn(move || -> Result<()> {
				for n in 0..PER_THREAD {
					db.append(json!({"t": t, "n": n}))?;
				}
				Ok(())
			})
		})
		.collect();
	for handle in handles {
		handle.join().expect("append thread panicked")?;
	}

	// Every item comes back exactly once, each thread's in the order it appended them.
	let mut next = [0usize; THREADS];
	let mut total = 0;
	while let Some(result) = db.fetch(None, None)? {
		let batch: Value = result.data.unwrap();
		for item in batch["items"].as_array().unwrap() {
			let t = item["t"].as_u64().unwrap() as usize;
			assert_eq!(item["n"].as_u64().unwrap() as usize, next[t], "{item}");
			next[t] += 1;
			total += 1;
		}
		db.remove(&result.removable.unwrap())?;
	}
	assert_eq!(next, [PER_THREAD; THREADS]);
	assert_eq!(total, THREADS * PER_THREAD);
	assert!(!db.has_data());
	Ok(())
}
