//! TransientDB over MemoryStore under random and heavy concurrent use: appends,
//! fetches, removals, releases and resets from many threads at once.

#![cfg(not(target_arch = "wasm32"))]

use rand::Rng;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use transientdb::{MemoryConfig, MemoryStore, Result, TransientDB};

fn memory_db(max_item_size: usize, max_items: usize) -> TransientDB {
	TransientDB::new(MemoryStore::new(MemoryConfig {
		max_item_size,
		max_items,
	}))
}

#[test]
fn test_memory_store_chaos() -> Result<()> {
	let db = Arc::new(memory_db(8 * 1024, 100_000));
	let evil_json = |i: u64| -> Value {
		match i % 5 {
			0 => json!({"normal": "boring", "timestamp": "2026-11-10T12:00:00.000Z"}),
			1 => json!({"nested": {"deeply": {"nested": {"value": "here",
				"with": ["arrays", "of", "doom", "that", "go", "on", "forever"]}}}}),
			2 => json!({"unicode": "🦀💥👾👿", "weird_spaces": "    \n\t\r    ", "empty": ""}),
			3 => {
				let mut huge = json!({});
				for n in 0..100 {
					huge[format!("field_{n}")] = json!("value");
				}
				huge
			}
			_ => json!({"x": "y"}),
		}
	};
	let random_delay = || {
		let mut rng = rand::thread_rng();
		if rng.gen_bool(0.2) {
			thread::sleep(Duration::from_millis(rng.gen_range(0..20)));
		}
	};

	let mut handles = vec![];
	for _ in 0..30 {
		let db = db.clone();
		handles.push(thread::spawn(move || {
			let mut rng = rand::thread_rng();
			for i in 0..1000 {
				random_delay();
				db.append(evil_json(i)).unwrap();
				if i % 100 == 0 {
					if let Some(items) = db.fetch(rng.gen_range(0..10), 64 * 1024).unwrap() {
						db.remove(items).unwrap();
					}
				}
			}
		}));
	}
	for _ in 0..15 {
		let db = db.clone();
		handles.push(thread::spawn(move || {
			let mut rng = rand::thread_rng();
			for _ in 0..200 {
				match rng.gen_range(0..4) {
					0 => db.reset(),
					1 => {
						// Random sized fetches, released straight away
						let _ = db.fetch(rng.gen_range(0..1000), rng.gen_range(0..100 * 1024));
					}
					2 => {
						// Random sized fetches, removed
						if let Some(items) = db.fetch(rng.gen_range(0..50), 64 * 1024).unwrap() {
							db.remove(items).unwrap();
						}
					}
					_ => {
						for _ in 0..50 {
							db.append(json!({"x": "y"})).unwrap();
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

	// Nothing is left held back, and the store still works.
	let left = db.has_data();
	db.append(json!({"final": "test"}))?;
	assert!(db.has_data());
	let mut last = None;
	while let Some(items) = db.fetch(1000, 1 << 20)? {
		last = items.values().last().cloned();
		db.remove(items)?;
	}
	assert_eq!(
		last,
		Some(json!({"final": "test"})),
		"had data before: {left}"
	);
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
	let db = Arc::new(memory_db(1024, 100_000));
	let removed = Arc::new(AtomicUsize::new(0));
	let mut handles = vec![];
	for t in 0..10 {
		let db = db.clone();
		handles.push(thread::spawn(move || {
			for i in 0..1_000 {
				db.append(json!({
					"thread": t,
					"index": i,
					"data": "some test data that takes up space and makes the json bigger"
				}))
				.unwrap();
			}
		}));
	}
	for _ in 0..5 {
		let (db, removed) = (db.clone(), removed.clone());
		handles.push(thread::spawn(move || {
			let mut total_items = 0;
			while total_items < 1_000 {
				if let Some(items) = db.fetch(100, 1 << 20).unwrap() {
					total_items += items.len();
					removed.fetch_add(items.len(), Ordering::SeqCst);
					db.remove(items).unwrap();
				}
				thread::sleep(Duration::from_micros(100));
			}
		}));
	}
	for handle in handles {
		handle.join().unwrap();
	}
	// No reset ran, so what was removed and what is left add up to every append.
	let mut left = 0;
	while let Some(items) = db.fetch(1000, 1 << 20)? {
		left += items.len();
		db.remove(items)?;
	}
	assert_eq!(removed.load(Ordering::SeqCst) + left, 10_000);
	Ok(())
}
