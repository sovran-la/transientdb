//! `Items` across threads, through `TransientDB`: it can be removed or released from any
//! thread, and dropping it is harmless after the store is gone. Dropping it while the
//! store is busy is in `items_lock_tests.rs`.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use common::*;
use serde_json::json;
use std::sync::Arc;
use std::thread;
use transientdb::{MemoryConfig, MemoryStore, TransientDB};

#[test]
fn test_drop_and_release_after_the_store_is_dropped() {
	let db = db_holding(3);
	let dropped = db.fetch(1, 4096).unwrap().unwrap();
	let released = db.fetch(1, 4096).unwrap().unwrap();
	let sent = db.fetch(1, 4096).unwrap().unwrap();
	drop(db);
	// Still readable, and giving them back does nothing harmful.
	assert_eq!(dropped.values(), tagged(0..1));
	assert_eq!(released.json(), r#"[{"i":1}]"#);
	drop(dropped);
	released.release();
	within_deadline(
		"dropping Items on another thread after the store",
		move || {
			assert_eq!(sent.values(), tagged(2..3));
			drop(sent);
		},
	);
}

#[test]
fn test_remove_on_another_thread_than_fetch() {
	let db = Arc::new(db_holding(6));
	let items = db.fetch(3, 4096).unwrap().unwrap();
	let remover = db.clone();
	within_deadline("remove on another thread", move || {
		remover.remove(items).unwrap()
	});
	assert_eq!(available(&db), tagged(3..6));

	// Fetched on another thread, removed here.
	let fetcher = db.clone();
	let items = within_deadline("fetch on another thread", move || {
		fetcher.fetch(2, 4096).unwrap().unwrap()
	});
	assert_eq!(items.values(), tagged(3..5));
	db.remove(items).unwrap();
	assert_eq!(available(&db), tagged(5..6));
}

#[test]
fn test_release_on_another_thread_than_fetch() {
	let db = Arc::new(db_holding(4));
	let items = db.fetch(4, 4096).unwrap().unwrap();
	assert!(!db.has_data());
	within_deadline("release on another thread", move || items.release());
	assert_eq!(available(&db), tagged(0..4));
}

#[test]
fn test_panic_on_a_thread_holding_items_releases_them() {
	let db = Arc::new(db_holding(5));
	let panicking = db.clone();
	let outcome = thread::spawn(move || {
		let _items = panicking.fetch(3, 4096).unwrap().unwrap();
		panic!("the send failed");
	})
	.join();
	assert!(outcome.is_err());
	assert!(db.has_data());
	assert_eq!(available(&db), tagged(0..5));
	// The store is not poisoned: the panic never happened inside it.
	db.append(json!({"i": 5})).unwrap();
	assert_eq!(available(&db), tagged(0..6));
}

#[test]
fn test_items_from_before_a_reset_match_nothing() {
	let db = db_holding(4);
	let old_release = db.fetch(2, 4096).unwrap().unwrap();
	let old_remove = db.fetch(2, 4096).unwrap().unwrap();
	db.reset();
	assert!(!db.has_data());
	for item in tagged(10..12) {
		db.append(item).unwrap();
	}
	let held = db.fetch(1, 4096).unwrap().unwrap();
	old_release.release();
	db.remove(old_remove).unwrap();
	assert_eq!(available(&db), tagged(11..12));
	drop(held);
	assert_eq!(available(&db), tagged(10..12));
}

#[test]
fn test_another_transientdbs_items_are_ignored_and_released() {
	let (a, b) = (db_holding(4), db_holding(4));
	let from_a = a.fetch(2, 4096).unwrap().unwrap();
	b.remove(from_a).unwrap();
	assert_eq!(available(&b), tagged(0..4));
	// Back in a, not deleted.
	assert_eq!(available(&a), tagged(0..4));

	let from_a = a.fetch(2, 4096).unwrap().unwrap();
	a.remove(from_a).unwrap();
	assert_eq!(available(&a), tagged(2..4));
	assert_eq!(available(&b), tagged(0..4));
}

#[test]
fn test_concurrent_fetchers_never_share_an_item() {
	const THREADS: usize = 8;
	const ITEMS: usize = 4000;
	let db = Arc::new(TransientDB::new(MemoryStore::new(MemoryConfig {
		max_item_size: 1024,
		max_items: ITEMS,
	})));
	for item in tagged(0..ITEMS) {
		db.append(item).unwrap();
	}
	let handles: Vec<_> = (0..THREADS)
		.map(|t| {
			let db = db.clone();
			thread::spawn(move || {
				let mut seen = Vec::new();
				while let Some(items) = db.fetch(1 + t % 5, 4096).unwrap() {
					let values = items.values();
					// Every other batch fails to send and is released for someone else.
					if seen.len() % 2 == 1 {
						drop(items);
						seen.push(Vec::new());
						continue;
					}
					db.remove(items).unwrap();
					seen.push(values);
				}
				seen.concat()
			})
		})
		.collect();
	let mut delivered: Vec<u64> = handles
		.into_iter()
		.flat_map(|h| h.join().unwrap())
		.map(|v| v["i"].as_u64().unwrap())
		.collect();
	delivered.sort_unstable();
	assert_eq!(delivered, (0..ITEMS as u64).collect::<Vec<_>>());
	assert!(!db.has_data());
}
