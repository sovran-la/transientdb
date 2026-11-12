//! An I/O error on every read and write, through the store: the call fails, and the store
//! is usable, with the same items, for the next call.

use super::*;
use crate::contract::{append_all, available, tagged};
use crate::file::config::FileConfig;
use crate::file::faults::Op;
use crate::{DataStore, Error, Items};
use serde_json::{json, Value};
use std::collections::HashSet;

/// How a case's store is set up before its call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setup {
	/// Items 0, 1 and 3 in slots 0, 1 and 3, slot 2 free: an append reuses a slot.
	FreeSlot,
	/// As `FreeSlot`, with item 2 appended into slot 2: an append grows the file.
	NoFreeSlot,
	/// Items 0, 1 and 2 with `max_items` 3: an append drops item 0, the oldest.
	Full,
	/// As `Full`, with item 0 held in an `Items` while the call runs.
	FullHoldingOldest,
}

/// A store set up as `setup`, and the `Items` holding its oldest item if it holds one.
fn store_for(path: &Path, setup: Setup) -> (FileStore, Hook, Option<Items>) {
	let full = matches!(setup, Setup::Full | Setup::FullHoldingOldest);
	let (mut store, hook) = store_with(path, if full { 3 } else { 10 });
	let mut held = None;
	if full {
		append_all(&mut store, &tagged(0..3));
		if setup == Setup::FullHoldingOldest {
			held = store.fetch(1, 100).unwrap();
			assert_eq!(held.as_ref().unwrap().values(), tagged(0..1));
		}
	} else {
		append_all(&mut store, &tagged(0..4));
		let first = store.fetch(2, 100).unwrap().unwrap();
		let third = store.fetch(1, 100).unwrap().unwrap();
		drop(first);
		store.remove(third).unwrap();
		if setup == Setup::NoFreeSlot {
			store.append(json!({"i": 2})).unwrap();
		}
	}
	drain(&hook);
	(store, hook, held)
}

/// One call on the store, returning whether it reported a failure.
type Call = fn(&mut FileStore) -> bool;

/// Each call, and how the store is set up for it.
const CALLS: [(&str, Setup, Call); 8] = [
	("append into a free slot", Setup::FreeSlot, |store| {
		store.append(json!({"i": 7})).is_err()
	}),
	("append growing the file", Setup::NoFreeSlot, |store| {
		store.append(json!({"i": 8})).is_err()
	}),
	("append dropping the oldest", Setup::Full, |store| {
		store.append(json!({"i": 7})).is_err()
	}),
	(
		"append dropping the held oldest",
		Setup::FullHoldingOldest,
		|store| store.append(json!({"i": 7})).is_err(),
	),
	("fetch", Setup::FreeSlot, |store| {
		store.fetch(10, 100).is_err()
	}),
	("remove", Setup::FreeSlot, |store| {
		// The fetch it removes fails first when reading is what fails.
		let Ok(Some(items)) = store.fetch(1, 100) else {
			return true;
		};
		store.remove(items).is_err()
	}),
	("reset", Setup::FreeSlot, |store| {
		store.reset();
		store.core().unwrap().index.live_len() > 0
	}),
	("has_data", Setup::FreeSlot, |store| !store.has_data()),
];

#[test]
fn test_every_failing_op_leaves_the_store_usable_with_the_same_items() {
	let mut caught = HashSet::new();
	let mut caught_dropping = HashSet::new();
	for op in Op::ALL {
		for (name, setup, call) in CALLS {
			let (_dir, path) = temp_path();
			let (mut store, hook, held) = store_for(&path, setup);
			let full = matches!(setup, Setup::Full | Setup::FullHoldingOldest);
			let before = stored(&mut store);
			hook.lock().unwrap().fail(op);
			let failed = call(&mut store);
			hook.lock().unwrap().fail.clear();
			if failed {
				caught.insert(op);
				if full {
					caught_dropping.insert(op);
				}
				// A failed entry write while dropping has dropped the oldest item: it is
				// gone either way. Any other failure leaves every item stored.
				let expected = if full && op == Op::Entry {
					before[1..].to_vec()
				} else {
					before.clone()
				};
				assert_eq!(stored(&mut store), expected, "{op:?} in {name}");
				// Nothing is left held back by the failed call, and a held item still
				// stored is still held: it is not handed out a second time.
				let unheld: Vec<Value> = match &held {
					Some(held) => {
						let held = held.values();
						expected.into_iter().filter(|v| !held.contains(v)).collect()
					}
					None => expected,
				};
				assert_eq!(available(&mut store), unheld, "{op:?} in {name}");
			}
			drop(held);
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
	// Each of the two writes of a drop was failed. A failed state byte or cut alone
	// cannot fail one: the state byte is written only after a failed entry write, and
	// two items are left live, so nothing is cut.
	assert_eq!(caught_dropping, [Op::Payload, Op::Entry].into());
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
