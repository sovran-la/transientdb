//! Sequences through the store: one damaged entry loses only its own item, and once
//! sequences are exhausted `append` fails loudly until a reset, or until the store is
//! emptied and the file opened again.

use super::*;
use crate::contract::{append_all, available, tagged};
use crate::file::config::FileConfig;
use crate::{DataStore, Error};
use serde_json::json;

fn assert_exhausted(store: &mut FileStore) {
	let result = store.append(json!({"i": 99}));
	assert!(
		matches!(&result, Err(Error::Io(e)) if e.to_string() == "sequence numbers exhausted"),
		"{result:?}"
	);
}

/// A file holding items 0, 1 and 2, with item 1 removed and its free entry carrying
/// sequence `u64::MAX`.
fn exhausting_file(path: &Path) {
	let mut core = open(path);
	for tag in 0..3 {
		core.append_text(&json!({ "i": tag }).to_string()).unwrap();
	}
	assert!(core.free(2).unwrap());
	drop(core);
	poke(path, layout().entry_offset(1) + 8, &u64::MAX.to_le_bytes());
}

#[test]
fn test_an_all_ff_entry_loses_only_its_own_item() {
	// The review's probe: 64 byte items, slot 1's entry overwritten with 0xFF.
	let (_dir, path) = temp_path();
	let config = FileConfig {
		max_item_size: 64,
		..config(&path, SLOT, 100)
	};
	let mut store = FileStore::new(config.clone());
	store.append(json!(1)).unwrap();
	store.append(json!(2)).unwrap();
	drop(store);
	let layout = crate::file::layout::Layout { slot_size: 64 };
	poke(&path, layout.entry_offset(1), &[0xFF; 32]);
	let mut store = FileStore::new(config.clone());
	for i in 10..13 {
		store.append(json!(i)).unwrap();
	}
	let items = store.fetch(100, 10_000).unwrap().unwrap();
	assert_eq!(items.json(), "[1,10,11,12]");
	drop(items);
	drop(store);
	let mut store = FileStore::new(config);
	let items = store.fetch(100, 10_000).unwrap().unwrap();
	assert_eq!(items.json(), "[1,10,11,12]");
}

#[test]
fn test_exhausted_sequences_fail_append_and_leave_the_rest_working() {
	let (_dir, path) = temp_path();
	exhausting_file(&path);
	let mut store = store(&path, 2);
	let bytes = bytes_of(&path);
	for _ in 0..3 {
		assert_exhausted(&mut store);
	}
	// Nothing was written, not even the drop of the oldest item.
	assert_eq!(bytes_of(&path), bytes);
	assert!(store.has_data());
	assert_eq!(
		available(&mut store),
		vec![json!({"i": 0}), json!({"i": 2})]
	);
}

#[test]
fn test_a_reset_lets_appends_work_again_and_stale_items_match_nothing() {
	let (_dir, path) = temp_path();
	exhausting_file(&path);
	let mut store = store(&path, 100);
	// Holds items with sequences 1 and 3, which the new items are about to get again.
	let held = store.fetch(10, 100).unwrap().unwrap();
	assert_exhausted(&mut store);
	store.reset();
	append_all(&mut store, &tagged(5..8));
	store.remove(held).unwrap();
	assert_eq!(stored(&mut store), tagged(5..8));
	assert_eq!(available(&mut store), tagged(5..8));
	drop(store);
	assert_eq!(reopened(&path), tagged(5..8));
}

#[test]
fn test_emptying_the_store_keeps_appends_failing_until_it_is_opened_again() {
	let (_dir, path) = temp_path();
	exhausting_file(&path);
	let mut store = store(&path, 100);
	let items = store.fetch(10, 100).unwrap().unwrap();
	store.remove(items).unwrap();
	assert_eq!(file_len(&path), crate::file::layout::HEADER_LEN);
	assert_exhausted(&mut store);
	drop(store);
	let mut store = super::store(&path, 100);
	append_all(&mut store, &tagged(5..7));
	assert_eq!(available(&mut store), tagged(5..7));
}
