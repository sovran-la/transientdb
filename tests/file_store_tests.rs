//! FileStore through `TransientDB`: the items outlive the process's store, and threads
//! share one store without losing or repeating an item.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use common::*;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use transientdb::{DataStore, Error, FileConfig, FileStore, TransientDB};

fn config(path: &Path) -> FileConfig {
	FileConfig {
		path: path.to_path_buf(),
		id: "write-key".to_string(),
		max_item_size: 1024,
		max_items: 10_000,
	}
}

fn file_db(path: &Path) -> TransientDB {
	TransientDB::new(FileStore::new(config(path)))
}

#[test]
fn test_append_fetch_remove_through_transient_db() {
	let dir = tempfile::tempdir().unwrap();
	let db = file_db(&dir.path().join("events.tdb"));
	assert!(!db.has_data());
	for item in tagged(0..5) {
		db.append(item).unwrap();
	}
	let first = db.fetch(2, 4096).unwrap().unwrap();
	let second = db.fetch(10, 4096).unwrap().unwrap();
	assert_eq!(first.json(), r#"[{"i":0},{"i":1}]"#);
	assert_eq!(second.values(), tagged(2..5));
	assert!(!db.has_data());
	db.remove(first).unwrap();
	drop(second);
	assert_eq!(available(&db), tagged(2..5));
	db.reset();
	assert!(!db.has_data());
	let err = db.append(json!({"big": "x".repeat(2000)})).unwrap_err();
	assert!(
		matches!(err, Error::ItemTooLarge { max: 1024, .. }),
		"{err}"
	);
}

#[test]
fn test_items_survive_from_one_instance_to_the_next() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("nested").join("events.tdb");
	let db = file_db(&path);
	for item in tagged(0..10) {
		db.append(item).unwrap();
	}
	let delivered = db.fetch(3, 4096).unwrap().unwrap();
	db.remove(delivered).unwrap();
	// Fetched but not removed when the process ends: sent again by the next one.
	let in_flight = db.fetch(3, 4096).unwrap().unwrap();
	drop(db);
	drop(in_flight);

	let db = file_db(&path);
	assert!(db.has_data());
	assert_eq!(available(&db), tagged(3..10));
	db.append(json!({"i": 10})).unwrap();
	drop(db);

	let db = file_db(&path);
	let mut seen = Vec::new();
	while let Some(items) = db.fetch(4, 4096).unwrap() {
		seen.extend(items.values());
		db.remove(items).unwrap();
	}
	assert_eq!(seen, tagged(3..11));
	drop(db);
	// Drained: the file is back to its header.
	assert_eq!(std::fs::metadata(&path).unwrap().len(), 256);
}

#[test]
fn test_another_id_discards_what_was_queued() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("events.tdb");
	let db = file_db(&path);
	db.append(json!(1)).unwrap();
	drop(db);
	let db = TransientDB::new(FileStore::new(FileConfig {
		id: "another-write-key".to_string(),
		..config(&path)
	}));
	assert!(!db.has_data());
}

#[test]
fn test_a_second_db_on_one_file_is_in_use_until_the_first_is_dropped() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("events.tdb");
	let first = file_db(&path);
	first.append(json!(1)).unwrap();
	let second = file_db(&dir.path().join(".").join("events.tdb"));
	assert!(matches!(second.append(json!(2)), Err(Error::InUse)));
	assert!(matches!(second.fetch(1, 100), Err(Error::InUse)));
	assert!(!second.has_data());
	let items = first.fetch(1, 100).unwrap().unwrap();
	assert!(matches!(second.remove(items), Err(Error::InUse)));
	// The Items went back to the first store.
	assert!(first.has_data());
	drop(first);
	assert!(second.has_data());
	assert_eq!(available(&second), vec![json!(1)]);
}

#[test]
fn test_threads_share_one_store_and_every_item_is_delivered_once() {
	const WRITERS: usize = 4;
	const PER_WRITER: usize = 250;
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("events.tdb");
	let db = Arc::new(file_db(&path));
	let writers: Vec<_> = (0..WRITERS)
		.map(|w| {
			let db = Arc::clone(&db);
			thread::spawn(move || {
				for n in 0..PER_WRITER {
					db.append(json!({ "w": w, "n": n })).unwrap();
				}
			})
		})
		.collect();
	// Senders fetch concurrently with the writers: each fetch is different items, and
	// every other one fails and is released for a later fetch.
	let senders: Vec<_> = (0..3)
		.map(|_| {
			let db = Arc::clone(&db);
			thread::spawn(move || {
				let mut delivered = Vec::new();
				let mut succeeds = false;
				let mut idle = 0;
				while idle < 1000 {
					match db.fetch(7, 4096).unwrap() {
						Some(items) => {
							idle = 0;
							succeeds = !succeeds;
							if succeeds {
								delivered.extend(items.values());
								db.remove(items).unwrap();
							}
						}
						None => {
							idle += 1;
							thread::yield_now();
						}
					}
				}
				delivered
			})
		})
		.collect();
	for writer in writers {
		writer.join().unwrap();
	}
	let mut delivered: Vec<Value> = senders
		.into_iter()
		.flat_map(|s| s.join().unwrap())
		.collect();
	// Whatever the senders left, delivered now.
	while let Some(items) = db.fetch(100, 64 * 1024).unwrap() {
		delivered.extend(items.values());
		db.remove(items).unwrap();
	}
	assert_eq!(
		delivered.len(),
		WRITERS * PER_WRITER,
		"nothing lost or repeated"
	);
	let unique: BTreeSet<String> = delivered.iter().map(Value::to_string).collect();
	assert_eq!(unique.len(), WRITERS * PER_WRITER);
	assert!(!db.has_data());
	drop(db);
	assert_eq!(std::fs::metadata(&path).unwrap().len(), 256);
}

#[test]
fn test_a_store_used_directly_and_then_through_a_db() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("events.tdb");
	let mut store = FileStore::new(config(&path));
	store.append(json!("direct")).unwrap();
	let db = TransientDB::new(store);
	assert_eq!(available(&db), vec![json!("direct")]);
}
