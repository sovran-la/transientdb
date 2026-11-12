//! TransientDB over MemoryStore under concurrent use: appends, fetches, removals and
//! resets from many threads lose, duplicate and reorder nothing they should not.

#![cfg(not(target_arch = "wasm32"))]

use serde_json::json;
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
fn test_concurrent_appends() -> Result<()> {
	let db = Arc::new(memory_db(1024, 10_000));
	let append_count = Arc::new(AtomicUsize::new(0));
	let handles: Vec<_> = (0..10)
		.map(|t| {
			let (db, counter) = (db.clone(), append_count.clone());
			thread::spawn(move || {
				for i in 0..100 {
					let event = json!({"thread": t, "index": i, "data": "test data"});
					db.append(event).unwrap();
					counter.fetch_add(1, Ordering::SeqCst);
				}
			})
		})
		.collect();
	for handle in handles {
		handle.join().unwrap();
	}
	assert_eq!(append_count.load(Ordering::SeqCst), 1000);

	// Every append is there, each thread's in its own order.
	let mut next = [0u64; 10];
	while let Some(items) = db.fetch(64, 64 * 1024)? {
		for item in items.values() {
			let t = item["thread"].as_u64().unwrap() as usize;
			assert_eq!(item["index"].as_u64().unwrap(), next[t]);
			next[t] += 1;
		}
		db.remove(items)?;
	}
	assert_eq!(next, [100; 10]);
	Ok(())
}

#[test]
fn test_concurrent_append_and_fetch() -> Result<()> {
	let db = Arc::new(memory_db(1024, 1000));
	let appender = db.clone();
	let append_handle = thread::spawn(move || {
		for i in 0..500 {
			appender.append(json!({"index": i})).unwrap();
			thread::sleep(Duration::from_micros(10));
		}
	});
	let fetcher = db.clone();
	let fetch_handle = thread::spawn(move || {
		let mut fetched = Vec::new();
		while fetched.len() < 500 {
			if let Some(items) = fetcher.fetch(10, 1024).unwrap() {
				fetched.extend(items.values());
				fetcher.remove(items).unwrap();
			}
			thread::sleep(Duration::from_micros(100));
		}
		fetched
	});
	append_handle.join().unwrap();
	let fetched = fetch_handle.join().unwrap();
	// One appender, one fetcher removing as it goes: everything, once, in order.
	let indexes: Vec<i64> = fetched
		.iter()
		.map(|v| v["index"].as_i64().unwrap())
		.collect();
	assert_eq!(indexes, (0..500).collect::<Vec<_>>());
	assert!(!db.has_data());
	Ok(())
}

#[test]
fn test_concurrent_reset() -> Result<()> {
	let db = Arc::new(memory_db(1024, 1000));
	for i in 0..100 {
		db.append(json!({"index": i}))?;
	}

	let mut handles = vec![];
	let resetter = db.clone();
	handles.push(thread::spawn(move || {
		thread::sleep(Duration::from_millis(50));
		resetter.reset();
	}));
	let appender = db.clone();
	handles.push(thread::spawn(move || {
		for i in 0..100 {
			appender.append(json!({"new_index": i})).unwrap();
			thread::sleep(Duration::from_micros(100));
		}
	}));
	let fetcher = db.clone();
	handles.push(thread::spawn(move || {
		for _ in 0..10 {
			// Fetched and released at once: nothing is held back past this loop.
			let _ = fetcher.fetch(1000, 1 << 20).unwrap();
			thread::sleep(Duration::from_millis(10));
		}
	}));
	for handle in handles {
		handle.join().unwrap();
	}

	// When the reset lands relative to the appends depends on scheduling. What is
	// certain: it came after the initial fill (which finished before any thread started)
	// and wiped all of it, and the fetcher removed nothing, so what remains is exactly the
	// appends after the reset: an in-order tail of new_index ending at 99, or nothing.
	let mut survivors = Vec::new();
	while let Some(items) = db.fetch(1000, 1 << 20)? {
		for item in items.values() {
			assert!(item.get("index").is_none(), "survived the reset: {item}");
			survivors.push(item["new_index"].as_i64().unwrap());
		}
		db.remove(items)?;
	}
	if let Some(&first) = survivors.first() {
		assert_eq!(survivors, (first..100).collect::<Vec<_>>());
	}
	assert!(!db.has_data());
	Ok(())
}

#[test]
fn test_transient_db_panic_recovery() -> Result<()> {
	use std::panic::{catch_unwind, AssertUnwindSafe};
	let db = Arc::new(memory_db(1024, 100));
	let panicking = db.clone();
	let outcome = catch_unwind(AssertUnwindSafe(|| {
		panicking.append(json!({"before": "panic"})).unwrap();
		let _items = panicking.fetch(10, 1024).unwrap();
		panic!("Simulated panic");
	}));
	assert!(outcome.is_err());

	// Usable, and the fetched item was given back as the panic unwound.
	db.append(json!({"after": "panic"}))?;
	let items = db.fetch(10, 1024)?.unwrap();
	assert_eq!(
		items.values(),
		vec![json!({"before": "panic"}), json!({"after": "panic"})]
	);
	Ok(())
}

#[test]
fn test_fetch_count_through_transientdb() -> Result<()> {
	let db = memory_db(1024, 1000);
	for i in 0..20 {
		db.append(json!({"index": i}))?;
	}
	let mut sizes = Vec::new();
	let mut seen = Vec::new();
	while let Some(items) = db.fetch(8, 1 << 20)? {
		sizes.push(items.len());
		seen.extend(items.values().iter().map(|v| v["index"].as_i64().unwrap()));
		db.remove(items)?;
	}
	assert_eq!(sizes, vec![8, 8, 4]);
	assert_eq!(seen, (0..20).collect::<Vec<_>>());
	assert!(!db.has_data());
	Ok(())
}

#[test]
fn test_large_capacity_never_drops_under_concurrent_appends() -> Result<()> {
	const THREADS: usize = 8;
	const PER_THREAD: usize = 500;
	let db = Arc::new(memory_db(1024, THREADS * PER_THREAD));
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
	while let Some(items) = db.fetch(500, 16 * 1024)? {
		for item in items.values() {
			let t = item["t"].as_u64().unwrap() as usize;
			assert_eq!(item["n"].as_u64().unwrap() as usize, next[t], "{item}");
			next[t] += 1;
			total += 1;
		}
		db.remove(items)?;
	}
	assert_eq!(next, [PER_THREAD; THREADS]);
	assert_eq!(total, THREADS * PER_THREAD);
	assert!(!db.has_data());
	Ok(())
}
