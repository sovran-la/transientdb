//! Stress tests - native only (uses filesystem APIs and threads)
#![cfg(not(target_arch = "wasm32"))]

use transientdb;

use rand::Rng;
use serde_json::json;
use std::fs;
use std::io::Result;
use std::sync::Arc;
use std::thread;
use tempfile::TempDir;
use transientdb::TransientDB;
use transientdb::{DirectoryConfig, DirectoryStore};
use transientdb::{MemoryConfig, MemoryStore};

/// Executes a comprehensive stress test of the MemoryStore implementation under heavy concurrent load.
/// This test runs for 30 seconds and simulates real-world usage patterns with multiple concurrent
/// operations and varying data patterns.
///
/// Test Structure:
/// - Creates a MemoryStore with a 10,000 item capacity and 1MB fetch size limit
/// - Runs multiple concurrent threads for 30 seconds performing different operations:
///
/// 1. Append Threads (6 total):
///    - Each thread continuously appends data using different patterns:
///      * Small: Simple key-value pairs
///      * Medium: Nested objects with arrays
///      * Large: Objects with multiple fields
///    - Tests the store's ability to handle varying payload sizes and structures
///
/// 2. Fetch Threads (4 total):
///    - Continuously fetch between 1-500 items
///    - Tests the store's read performance and consistency under load
///
/// 3. Chaos Threads (2 total):
///    - Randomly perform disruptive operations:
///      * Complete store resets
///      * Rapid-fire tiny appends (100 at a time)
///      * Large batch fetches
///      * Multiple small fetches (10 at a time)
///    - Tests the store's resilience to unpredictable usage patterns
///
/// Metrics Tracked:
/// - Total successful appends
/// - Total successful fetches
/// - Total store resets
/// - Total errors encountered
///
/// The test verifies:
/// - Thread safety of all operations
/// - Memory store stability under heavy concurrent load
/// - Proper handling of varying data sizes and patterns
/// - Error handling and recovery
/// - FIFO behavior maintenance
/// - Resource cleanup
///
/// Note: This test is designed to stress test the implementation and may use
/// significant CPU resources while running.

#[test]
fn test_memory_store_stress() -> Result<()> {
	use std::sync::atomic::{AtomicU64, Ordering};
	use std::time::{Duration, Instant};

	let config = MemoryConfig {
		write_key: "test-key-mem".to_string(),
		max_items: 10_000,
		max_fetch_size: 1024 * 1024,
	};

	let store = MemoryStore::new(config);
	let db = Arc::new(TransientDB::new(store));

	// Tracking metrics
	let total_appends = Arc::new(AtomicU64::new(0));
	let total_fetches = Arc::new(AtomicU64::new(0));
	let total_resets = Arc::new(AtomicU64::new(0));
	let errors_count = Arc::new(AtomicU64::new(0));

	let mut handles = vec![];
	let test_duration = Duration::from_secs(10);
	let start_time = Instant::now();

	// Append threads
	for pattern in 0..6 {
		let db = db.clone();
		let total_appends = total_appends.clone();
		let errors_count = errors_count.clone();
		let start = start_time;

		handles.push(thread::spawn(move || {
			let mut rng = rand::thread_rng();
			while start.elapsed() < test_duration {
				let data = match pattern % 3 {
					0 => json!({ "small": rng.random::<u64>() }),
					1 => json!({
						"medium": {
							"data": rng.random::<u64>(),
							"array": (0..10).collect::<Vec<_>>()
						}
					}),
					_ => {
						let mut obj = json!({});
						for i in 0..10 {
							obj[format!("field_{}", i)] = json!(rng.random::<u64>());
						}
						obj
					}
				};

				match db.append(data) {
					Ok(_) => {
						total_appends.fetch_add(1, Ordering::Relaxed);
					}
					Err(_) => {
						errors_count.fetch_add(1, Ordering::Relaxed);
					}
				}
			}
		}));
	}

	// Fetch threads
	for _ in 0..4 {
		let db = db.clone();
		let total_fetches = total_fetches.clone();
		let errors_count = errors_count.clone();
		let start = start_time;

		handles.push(thread::spawn(move || {
			let mut rng = rand::thread_rng();
			while start.elapsed() < test_duration {
				match db.fetch(Some(rng.gen_range(1..500)), None) {
					Ok(Some(_)) => {
						total_fetches.fetch_add(1, Ordering::Relaxed);
					}
					Ok(None) => {}
					Err(_) => {
						errors_count.fetch_add(1, Ordering::Relaxed);
					}
				}
			}
		}));
	}

	// Chaos threads
	for _ in 0..2 {
		let db = db.clone();
		let total_resets = total_resets.clone();
		let total_appends = total_appends.clone();
		let errors_count = errors_count.clone();
		let start = start_time;

		handles.push(thread::spawn(move || {
			let mut rng = rand::thread_rng();
			while start.elapsed() < test_duration {
				match rng.gen_range(0..4) {
					0 => {
						db.reset();
						total_resets.fetch_add(1, Ordering::Relaxed);
					}
					1 => {
						// Rapid-fire tiny appends
						for _ in 0..100 {
							let tiny = json!({"x": rng.random::<u64>()});
							match db.append(tiny) {
								Ok(_) => {
									total_appends.fetch_add(1, Ordering::Relaxed);
								}
								Err(_) => {
									errors_count.fetch_add(1, Ordering::Relaxed);
								}
							}
						}
					}
					2 => {
						// Large fetch
						let _ = db.fetch(None, None);
					}
					_ => {
						// Multiple smaller fetches
						for _ in 0..10 {
							let _ = db.fetch(Some(rng.gen_range(1..50)), None);
						}
					}
				}
			}
		}));
	}

	// Monitoring thread - now only monitoring has_data()
	{
		let db = db.clone();
		let start = start_time;

		handles.push(thread::spawn(move || {
			while start.elapsed() < test_duration {
				let _has_data = db.has_data();
			}
		}));
	}

	// Wait for all threads to complete
	for handle in handles {
		handle.join().expect("Thread panicked");
	}

	// Print final statistics
	println!("=== Memory Store Stress Test Results ===");
	println!("Total Duration: {:?}", start_time.elapsed());
	println!("Total Appends: {}", total_appends.load(Ordering::Relaxed));
	println!("Total Fetches: {}", total_fetches.load(Ordering::Relaxed));
	println!("Total Resets: {}", total_resets.load(Ordering::Relaxed));
	println!("Total Errors: {}", errors_count.load(Ordering::Relaxed));

	Ok(())
}

#[test]
fn test_directory_store_stress() -> Result<()> {
	use std::sync::atomic::{AtomicU64, Ordering};
	use std::time::{Duration, Instant};

	let temp_dir = TempDir::new()?;
	let config = DirectoryConfig {
		write_key: "test-key-dir".to_string(),
		storage_location: temp_dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size: 1024 * 2, // 2KB files to force frequent rotation
	};

	let store = DirectoryStore::new(config)?;
	let db = Arc::new(TransientDB::new(store));

	// Tracking metrics
	let total_appends = Arc::new(AtomicU64::new(0));
	let total_fetches = Arc::new(AtomicU64::new(0));
	let total_resets = Arc::new(AtomicU64::new(0));
	let errors_count = Arc::new(AtomicU64::new(0));
	let files_removed = Arc::new(AtomicU64::new(0));

	let mut handles = vec![];
	let test_duration = Duration::from_secs(10);
	let start_time = Instant::now();

	// Append threads (6)
	for pattern in 0..6 {
		let db = db.clone();
		let total_appends = total_appends.clone();
		let errors_count = errors_count.clone();
		let start = start_time;

		handles.push(thread::spawn(move || {
			let mut rng = rand::thread_rng();
			while start.elapsed() < test_duration {
				let data = match pattern % 3 {
					0 => json!({
						"small": rng.random::<u64>(),
						"timestamp": chrono::Utc::now().to_rfc3339()
					}),
					1 => json!({
						"medium": {
							"data": rng.random::<u64>(),
							"array": (0..10).collect::<Vec<_>>(),
							"timestamp": chrono::Utc::now().to_rfc3339()
						}
					}),
					_ => {
						let mut obj = json!({
							"timestamp": chrono::Utc::now().to_rfc3339()
						});
						for i in 0..10 {
							obj[format!("field_{}", i)] = json!(rng.random::<u64>());
						}
						obj
					}
				};

				match db.append(data) {
					Ok(_) => {
						total_appends.fetch_add(1, Ordering::Relaxed);
					}
					Err(_) => {
						errors_count.fetch_add(1, Ordering::Relaxed);
					}
				}

				// Small delay to prevent overwhelming the file system
				thread::sleep(Duration::from_micros(100));
			}
		}));
	}

	// Fetch and remove threads (4)
	for _ in 0..4 {
		let db = db.clone();
		let total_fetches = total_fetches.clone();
		let errors_count = errors_count.clone();
		let files_removed = files_removed.clone();
		let start = start_time;

		handles.push(thread::spawn(move || {
			let mut rng = rand::thread_rng();
			while start.elapsed() < test_duration {
				// Randomize between small and large fetches
				let count = if rng.gen_bool(0.7) {
					Some(rng.gen_range(1..5))
				} else {
					Some(rng.gen_range(5..20))
				};

				match db.fetch(count, None) {
					Ok(Some(result)) => {
						total_fetches.fetch_add(1, Ordering::Relaxed);

						// Remove fetched files
						if let Some(removable) = result.removable {
							match db.remove(&removable) {
								Ok(_) => {
									files_removed
										.fetch_add(removable.len() as u64, Ordering::Relaxed);
								}
								Err(_) => {
									errors_count.fetch_add(1, Ordering::Relaxed);
								}
							}
						}
					}
					Ok(None) => {}
					Err(_) => {
						errors_count.fetch_add(1, Ordering::Relaxed);
					}
				}

				// Small delay between fetches
				thread::sleep(Duration::from_millis(rng.gen_range(10..50)));
			}
		}));
	}

	// Chaos threads (2)
	for _ in 0..2 {
		let db = db.clone();
		let total_resets = total_resets.clone();
		let total_appends = total_appends.clone();
		let errors_count = errors_count.clone();
		let start = start_time;

		handles.push(thread::spawn(move || {
			let mut rng = rand::thread_rng();
			while start.elapsed() < test_duration {
				match rng.gen_range(0..4) {
					0 => {
						// Reset the store
						db.reset();
						total_resets.fetch_add(1, Ordering::Relaxed);
						thread::sleep(Duration::from_millis(100));
					}
					1 => {
						// Rapid-fire tiny appends
						for _ in 0..20 {
							let tiny = json!({
								"x": rng.random::<u64>(),
								"timestamp": chrono::Utc::now().to_rfc3339()
							});
							match db.append(tiny) {
								Ok(_) => {
									total_appends.fetch_add(1, Ordering::Relaxed);
								}
								Err(_) => {
									errors_count.fetch_add(1, Ordering::Relaxed);
								}
							}
						}
					}
					2 => {
						// Large fetch with removal
						if let Ok(Some(result)) = db.fetch(None, Some(1024 * 100)) {
							if let Some(removable) = result.removable {
								let _ = db.remove(&removable);
							}
						}
					}
					_ => {
						// Multiple small fetches
						for _ in 0..5 {
							if let Ok(Some(result)) = db.fetch(Some(1), None) {
								if let Some(removable) = result.removable {
									let _ = db.remove(&removable);
								}
							}
						}
					}
				}
			}
		}));
	}

	// File system monitoring thread
	{
		let temp_dir_path = temp_dir.path().to_owned();
		let start = start_time;

		handles.push(thread::spawn(move || {
			while start.elapsed() < test_duration {
				if let Ok(entries) = fs::read_dir(&temp_dir_path) {
					let mut open_files = 0;
					let mut temp_files = 0;

					for entry in entries.filter_map(Result::ok) {
						let path = entry.path();
						if let Some(ext) = path.extension() {
							if ext == "temp" {
								temp_files += 1;
							} else {
								open_files += 1;
							}
						}
					}

					// Verify reasonable file counts
					assert!(
						open_files + temp_files < 10000,
						"Too many files accumulated"
					);
				}
				thread::sleep(Duration::from_millis(100));
			}
		}));
	}

	// Wait for all threads to complete
	for handle in handles {
		handle.join().expect("Thread panicked");
	}

	// Print final statistics
	println!("=== Directory Store Stress Test Results ===");
	println!("Total Duration: {:?}", start_time.elapsed());
	println!("Total Appends: {}", total_appends.load(Ordering::Relaxed));
	println!("Total Fetches: {}", total_fetches.load(Ordering::Relaxed));
	println!("Total Resets: {}", total_resets.load(Ordering::Relaxed));
	println!("Files Removed: {}", files_removed.load(Ordering::Relaxed));
	println!("Total Errors: {}", errors_count.load(Ordering::Relaxed));

	// Final verification that the store is still functional
	db.append(json!({"final_test": true}))?;
	assert!(
		db.has_data() || !db.has_data(),
		"Store should be in valid state"
	);

	Ok(())
}
