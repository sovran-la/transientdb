//! `TransientDB::end_batch` across stores: the same calls group items into the same
//! batches in `DirectoryStore` and `MemoryStore`, and concurrent appends around
//! boundaries lose, duplicate and reorder nothing.

#![cfg(not(target_arch = "wasm32"))]

use serde_json::{json, Value};
use std::io::Result;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::{fs, thread};
use tempfile::TempDir;
use transientdb::{
	DirectoryConfig, DirectoryStore, Envelope, MemoryConfig, MemoryStore, TransientDB,
};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
	Append(usize),
	End,
}

/// A tiny xorshift script: about one call in four is `end_batch`.
fn steps(seed: u32, len: usize) -> Vec<Step> {
	let mut state = seed.max(1);
	let mut tag = 0;
	(0..len)
		.map(|_| {
			state ^= state << 13;
			state ^= state >> 17;
			state ^= state << 5;
			match state % 4 {
				0 => Step::End,
				_ => {
					tag += 1;
					Step::Append(tag - 1)
				}
			}
		})
		.collect()
}

/// The appends between boundaries, empty spans dropped.
fn spans(steps: &[Step]) -> Vec<Vec<usize>> {
	let mut spans = vec![Vec::new()];
	for step in steps {
		match step {
			Step::Append(tag) => spans.last_mut().unwrap().push(*tag),
			Step::End => spans.push(Vec::new()),
		}
	}
	spans.retain(|s| !s.is_empty());
	spans
}

fn run<T>(db: &TransientDB<T>, steps: &[Step]) -> Result<()> {
	for step in steps {
		match step {
			Step::Append(tag) => db.append(json!({"tag": tag}))?,
			Step::End => db.end_batch()?,
		}
	}
	Ok(())
}

fn tags(items: &[Value]) -> Vec<usize> {
	items
		.iter()
		.map(|v| v["tag"].as_u64().unwrap() as usize)
		.collect()
}

fn memory_db() -> TransientDB<Value> {
	TransientDB::new(MemoryStore::new(MemoryConfig {
		envelope: Envelope::default(),
		max_items: None,
		max_fetch_size: 1024 * 1024,
		max_fetch_items: None,
	}))
}

fn directory_db(dir: &TempDir) -> Result<TransientDB<Vec<PathBuf>>> {
	Ok(TransientDB::new(DirectoryStore::new(DirectoryConfig {
		envelope: Envelope::default(),
		storage_location: dir.path().to_owned(),
		base_filename: "events".into(),
		max_file_size: 1024 * 1024,
		max_file_items: None,
	})?))
}

fn file_items(path: &PathBuf) -> Result<Vec<Value>> {
	let json: Value = serde_json::from_slice(&fs::read(path)?)?;
	Ok(json["items"].as_array().unwrap().clone())
}

/// Every batch, fetched and removed one at a time, as its items.
fn drain_memory(db: &TransientDB<Value>) -> Result<Vec<Vec<Value>>> {
	let mut batches = Vec::new();
	while let Some(result) = db.fetch(None, None)? {
		let items = result.data.unwrap()["items"].as_array().unwrap().clone();
		assert!(!items.is_empty());
		assert_eq!(items.len(), result.removable.as_ref().unwrap().len());
		db.remove(&result.removable.unwrap())?;
		batches.push(items);
	}
	Ok(batches)
}

/// Every file, fetched and removed one at a time, as its items.
fn drain_directory(db: &TransientDB<Vec<PathBuf>>) -> Result<Vec<Vec<Value>>> {
	let mut batches = Vec::new();
	while let Some(result) = db.fetch(Some(1), None)? {
		for path in result.data.as_ref().unwrap() {
			let items = file_items(path)?;
			assert!(!items.is_empty());
			batches.push(items);
		}
		db.remove(&result.removable.unwrap())?;
	}
	Ok(batches)
}

#[test]
fn test_directory_and_memory_group_items_the_same_way() -> Result<()> {
	for seed in [3u32, 17, 0x1234_5678, 0xfeed_f00d] {
		let script = steps(seed, 500);
		let dir = TempDir::new()?;
		let directory = directory_db(&dir)?;
		let memory = memory_db();
		run(&directory, &script)?;
		run(&memory, &script)?;

		let from_directory: Vec<Vec<usize>> = drain_directory(&directory)?
			.iter()
			.map(|b| tags(b))
			.collect();
		let from_memory: Vec<Vec<usize>> = drain_memory(&memory)?.iter().map(|b| tags(b)).collect();
		assert!(!directory.has_data() && !memory.has_data());
		assert_eq!(from_directory, from_memory, "seed {seed}");
		assert_eq!(from_memory, spans(&script), "seed {seed}");
	}
	Ok(())
}

/// `THREADS` threads append their own numbered items while another calls `end_batch`
/// over and over and a consumer fetches and removes; then the rest is drained.
fn concurrent<T: Send + 'static>(
	db: Arc<TransientDB<T>>,
	drain: fn(&TransientDB<T>) -> Result<Vec<Vec<Value>>>,
) -> Result<()> {
	const THREADS: usize = 4;
	const PER_THREAD: usize = 250;
	let done = Arc::new(AtomicBool::new(false));

	let appenders: Vec<_> = (0..THREADS)
		.map(|t| {
			let db = db.clone();
			thread::spawn(move || {
				for i in 0..PER_THREAD {
					db.append(json!({"t": t, "i": i})).unwrap();
				}
			})
		})
		.collect();
	let ender = {
		let (db, done) = (db.clone(), done.clone());
		thread::spawn(move || {
			let mut calls = 0usize;
			while !done.load(Ordering::SeqCst) {
				db.end_batch().unwrap();
				calls += 1;
				thread::yield_now();
			}
			calls
		})
	};
	let consumer = {
		let (db, done) = (db.clone(), done.clone());
		thread::spawn(move || {
			let mut delivered = Vec::new();
			while !done.load(Ordering::SeqCst) {
				delivered.extend(drain(&db).unwrap());
				thread::yield_now();
			}
			delivered
		})
	};

	for handle in appenders {
		handle.join().unwrap();
	}
	done.store(true, Ordering::SeqCst);
	assert!(ender.join().unwrap() > 0);
	let mut delivered = consumer.join().unwrap();
	delivered.extend(drain(&db)?);

	let items: Vec<Value> = delivered.concat();
	assert_eq!(
		items.len(),
		THREADS * PER_THREAD,
		"lost or duplicated items"
	);
	for t in 0..THREADS {
		let own: Vec<u64> = items
			.iter()
			.filter(|v| v["t"] == t)
			.map(|v| v["i"].as_u64().unwrap())
			.collect();
		assert_eq!(
			own,
			(0..PER_THREAD as u64).collect::<Vec<_>>(),
			"thread {t}"
		);
	}
	assert!(!db.has_data());
	Ok(())
}

#[test]
fn test_concurrent_end_batch_with_memory_store() -> Result<()> {
	concurrent(Arc::new(memory_db()), drain_memory)
}

#[test]
fn test_concurrent_end_batch_with_directory_store() -> Result<()> {
	let dir = TempDir::new()?;
	concurrent(Arc::new(directory_db(&dir)?), drain_directory)
}
