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
use transientdb::{DirectoryConfig, DirectoryStore, MemoryConfig, MemoryStore, TransientDB};

#[test]
fn test_concurrent_appends() -> Result<()> {
	let config = MemoryConfig {
		write_key: "test-key-concurrent-appends".to_string(),
		max_items: 10000,
		max_fetch_size: 1024,
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
		write_key: "test-key-append-and-fetch".to_string(), // Unique key
		max_items: 1000,
		max_fetch_size: 1024,
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
				let items = batch["batch"].as_array().unwrap();
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
		write_key: "test-key-reset".to_string(), // Unique key
		max_items: 1000,
		max_fetch_size: 1024,
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
				let items = batch["batch"].as_array().unwrap();
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
		write_key: "test-key-directory".to_string(),
		storage_location: temp_dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size: 200, // Small size to force rotation
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
		write_key: "test-key".to_string(),
		storage_location: temp_dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size: 1024 * 10, // 10KB files to force lots of rotation
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
		write_key: "test-key".to_string(),
		max_items: 100_000,
		max_fetch_size: 1024 * 1024,
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
		write_key: "test-key".to_string(),
		storage_location: temp_dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size: 1024 * 2, // Tiny 2KB files for maximum pain
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
		write_key: "test-key".to_string(),
		storage_location: temp_dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size: 1024,
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
						content.starts_with("{ \"batch\": [")
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
		write_key: "test-key".to_string(),
		max_items: 100,
		max_fetch_size: 1024,
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
		write_key: "test-key".to_string(),
		storage_location: temp_dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size: 256, // Small size to force more file rotations
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
				if let Some(batch) = value["batch"].as_array() {
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
		write_key: "test-key".to_string(),
		max_items: 100_000,
		max_fetch_size: 1024 * 1024,
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
					let items = batch["batch"].as_array().unwrap();
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
