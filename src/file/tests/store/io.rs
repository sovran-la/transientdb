//! An I/O error on every read and write, through the store: the call fails, and the store
//! is usable, with the same items, for the next call.

use super::*;
use crate::contract::{append_all, available, tagged};
use crate::file::config::FileConfig;
use crate::file::faults::Op;
use crate::{DataStore, Error};
use serde_json::json;
use std::collections::HashSet;

/// A store holding items 0, 1 and 3 in slots 0, 1 and 3, with slot 2 free, so an append
/// reuses a slot.
fn store_with_a_free_slot(path: &Path) -> (FileStore, Hook) {
	let (mut store, hook) = store_with(path, 10);
	append_all(&mut store, &tagged(0..4));
	let first = store.fetch(2, 100).unwrap().unwrap();
	let third = store.fetch(1, 100).unwrap().unwrap();
	drop(first);
	store.remove(third).unwrap();
	drain(&hook);
	(store, hook)
}

/// One call on the store, returning whether it reported a failure.
type Call = fn(&mut FileStore) -> bool;

/// Each call, and whether the free slot is filled first, so an append grows the file.
const CALLS: [(&str, bool, Call); 6] = [
	("append into a free slot", false, |store| {
		store.append(json!({"i": 7})).is_err()
	}),
	("append growing the file", true, |store| {
		store.append(json!({"i": 8})).is_err()
	}),
	("fetch", false, |store| store.fetch(10, 100).is_err()),
	("remove", false, |store| {
		// The fetch it removes fails first when reading is what fails.
		let Ok(Some(items)) = store.fetch(1, 100) else {
			return true;
		};
		store.remove(items).is_err()
	}),
	("reset", false, |store| {
		store.reset();
		store.core().unwrap().index.live_len() > 0
	}),
	("has_data", false, |store| !store.has_data()),
];

#[test]
fn test_every_failing_op_leaves_the_store_usable_with_the_same_items() {
	let mut caught = HashSet::new();
	for op in Op::ALL {
		for (name, fill, call) in CALLS {
			let (_dir, path) = temp_path();
			let (mut store, hook) = store_with_a_free_slot(&path);
			if fill {
				store.append(json!({"i": 2})).unwrap();
			}
			let before = stored(&mut store);
			hook.lock().unwrap().fail(op);
			let failed = call(&mut store);
			hook.lock().unwrap().fail.clear();
			if failed {
				caught.insert(op);
				assert_eq!(stored(&mut store), before, "{op:?} in {name}");
				// Nothing is left held back by the failed call.
				assert_eq!(available(&mut store), before, "{op:?} in {name}");
			}
			// The next call works, and so does the next store.
			store.append(json!({"i": 9})).unwrap();
			let now = stored(&mut store);
			assert_eq!(now.last(), Some(&json!({"i": 9})), "{op:?} in {name}");
			assert_eq!(available(&mut store), now, "{op:?} in {name}");
			drop(store);
			assert_eq!(reopened(&path), now, "{op:?} in {name}");
		}
	}
	let expected: HashSet<Op> = [Op::Payload, Op::Entry, Op::State, Op::Cut, Op::ReadItem].into();
	assert_eq!(caught, expected);
}

#[test]
fn test_a_fetch_that_fails_part_way_hands_out_nothing() {
	let (_dir, path) = temp_path();
	let (mut store, hook) = store_with(&path, 10);
	append_all(&mut store, &tagged(0..4));
	hook.lock().unwrap().fail_after(Op::ReadItem, 2);
	assert!(matches!(store.fetch(10, 100), Err(Error::Io(_))));
	hook.lock().unwrap().fail.clear();
	assert!(store.has_data());
	assert_eq!(available(&mut store), tagged(0..4));
}

/// The config a store is reopened with in each case, and the ops to fail at its open.
fn open_cases(path: &Path) -> Vec<(FileConfig, Op)> {
	let same = config(path, SLOT, 10);
	let other = config(path, SLOT * 2, 10);
	let mut cases: Vec<(FileConfig, Op)> = [Op::Open, Op::Length, Op::ReadHeader, Op::ReadEntry]
		.into_iter()
		.map(|op| (same.clone(), op))
		.collect();
	// A file that does not match is deleted and created again.
	cases.extend([Op::Delete, Op::Create, Op::Header].map(|op| (other.clone(), op)));
	cases
}

#[test]
fn test_every_failing_op_at_open_fails_each_call_until_it_succeeds() {
	let (_dir, probe) = temp_path();
	for (config, op) in open_cases(&probe) {
		let (_dir, path) = temp_path();
		let mut first = store(&path, 10);
		append_all(&mut first, &tagged(0..3));
		drop(first);
		let config = FileConfig {
			path: path.clone(),
			..config
		};
		let hook = Faults::shared();
		hook.lock().unwrap().fail(op);
		let mut store = FileStore::with_hook(config.clone(), hook.clone());
		assert!(
			matches!(store.append(json!(1)), Err(Error::Io(_))),
			"{op:?}"
		);
		assert!(matches!(store.fetch(1, 100), Err(Error::Io(_))), "{op:?}");
		assert!(!store.has_data(), "{op:?}");
		assert!(store.core().is_none(), "{op:?}");
		hook.lock().unwrap().fail.clear();
		// The next call opens the file: the same items, or a fresh file if it did not
		// match.
		let expected = if config.max_item_size == SLOT {
			tagged(0..3)
		} else {
			Vec::new()
		};
		assert_eq!(available(&mut store), expected, "{op:?}");
		store.append(json!(1)).unwrap();
	}
}
