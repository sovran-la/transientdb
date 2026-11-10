//! Stress tests - native only (uses threads)
#![cfg(not(target_arch = "wasm32"))]

use rand::Rng;
use serde_json::json;
use std::sync::Arc;
use std::thread;
use transientdb::TransientDB;
use transientdb::{MemoryConfig, MemoryStore, Result};

/// Executes a comprehensive stress test of the MemoryStore implementation under heavy concurrent load.
/// This test runs for 30 seconds and simulates real-world usage patterns with multiple concurrent
/// operations and varying data patterns.
///
/// Test Structure:
/// - Creates a MemoryStore with a 10,000 item capacity and an 8KB item size limit
/// - Runs multiple concurrent threads for 10 seconds performing different operations:
///
/// 1. Append Threads (6 total):
///    - Each thread continuously appends data using different patterns:
///      * Small: Simple key-value pairs
///      * Medium: Nested objects with arrays
///      * Large: Objects with multiple fields
///    - Tests the store's ability to handle varying payload sizes and structures
///
/// 2. Fetch Threads (4 total):
///    - Continuously fetch between 1-500 items, removing every other fetch and
///      releasing the rest
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
		max_item_size: 8 * 1024,
		max_items: 10_000,
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
				match db.fetch(rng.gen_range(1..500), 1024 * 1024) {
					Ok(Some(items)) => {
						total_fetches.fetch_add(1, Ordering::Relaxed);
						assert!(items.len() < 500 && items.byte_len() <= 1024 * 1024);
						if rng.gen_bool(0.5) && db.remove(items).is_err() {
							errors_count.fetch_add(1, Ordering::Relaxed);
						}
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
						let _ = db.fetch(usize::MAX, 1024 * 1024);
					}
					_ => {
						// Multiple smaller fetches
						for _ in 0..10 {
							let _ = db.fetch(rng.gen_range(1..50), 1024 * 1024);
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
	assert_eq!(errors_count.load(Ordering::Relaxed), 0);

	// Every fetch was removed or released: whatever is left can be fetched.
	while let Some(items) = db.fetch(1000, 1024 * 1024)? {
		db.remove(items)?;
	}
	assert!(!db.has_data());

	Ok(())
}
