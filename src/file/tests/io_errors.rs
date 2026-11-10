//! An I/O error on every read and write leaves the store usable, with the same items,
//! for the next call.

use super::support::*;
use crate::file::core::FileCore;
use crate::file::faults::{Faults, Hook, Op};
use std::collections::HashSet;

/// A store with items in slots 0 and 2 and slot 1 free, so an append reuses a slot.
fn store(path: &std::path::Path) -> (FileCore, Hook) {
	let (mut core, hook) = open_with(path, 10);
	for tag in 0..3 {
		core.append_text(&item(6, tag)).unwrap();
	}
	core.free(2).unwrap();
	drain(&hook);
	(core, hook)
}

/// One operation on the store, returning whether it reported a failure.
type Operation = fn(&mut FileCore) -> bool;

/// Each operation, and whether the free slot is filled first, so an append grows the file.
const OPERATIONS: [(&str, bool, Operation); 5] = [
	("append into a free slot", false, |core| {
		core.append_text(&item(6, 7)).is_err()
	}),
	("append growing the file", true, |core| {
		core.append_text(&item(6, 8)).is_err()
	}),
	("read", false, |core| {
		core.read_items(10, 100, |_| false).is_err()
	}),
	("remove", false, |core| core.free(1).is_err()),
	("reset", false, |core| !core.reset()),
];

#[test]
fn test_every_failing_op_leaves_every_operation_usable_with_the_same_items() {
	let mut caught = HashSet::new();
	for op in Op::ALL {
		for (name, fill, operation) in OPERATIONS {
			let (_dir, path) = temp_path();
			let (mut core, hook) = store(&path);
			if fill {
				core.append_text(&item(6, 7)).unwrap();
			}
			let before = texts(&mut core);
			hook.lock().unwrap().fail(op);
			let failed = operation(&mut core);
			hook.lock().unwrap().fail.clear();
			if failed {
				caught.insert(op);
				assert_eq!(texts(&mut core), before, "{op:?} in {name}");
			}
			// The next call works, and so does the next store.
			core.append_text(&item(6, 9)).unwrap();
			let now = texts(&mut core);
			assert_eq!(now.last(), Some(&item(6, 9)), "{op:?} in {name}");
			drop(core);
			assert_eq!(texts(&mut open(&path)), now, "{op:?} in {name}");
		}
	}
	let expected: HashSet<Op> = [Op::Payload, Op::Entry, Op::State, Op::Cut, Op::ReadItem].into();
	assert_eq!(caught, expected);
}

#[test]
fn test_every_failing_op_at_open_is_an_error_and_the_next_open_works() {
	let (_dir, path) = temp_path();
	drop(store(&path));
	let bytes = bytes_of(&path);
	let config = config(&path, SLOT, 10);
	for op in [Op::Open, Op::Length, Op::ReadHeader, Op::ReadEntry] {
		let hook = Faults::shared();
		hook.lock().unwrap().fail(op);
		assert!(
			FileCore::open(&path, &config, hook.clone()).is_err(),
			"{op:?}"
		);
		assert!(drain(&hook).is_empty(), "{op:?}");
		assert_eq!(bytes_of(&path), bytes, "{op:?}");
	}
	// Failing on the last entry, after the others were read.
	let hook = Faults::shared();
	hook.lock().unwrap().fail_after(Op::ReadEntry, 2);
	assert!(FileCore::open(&path, &config, hook).is_err());
	assert_eq!(texts(&mut open(&path)), vec![item(6, 0), item(6, 2)]);
}

#[test]
fn test_a_failure_part_way_through_reading_entries_clears_nothing() {
	let (_dir, path) = temp_path();
	drop(store(&path));
	// Slot 0's entry is damaged; reading slot 2's fails.
	poke(&path, layout().entry_offset(0), &[7]);
	let bytes = bytes_of(&path);
	let hook = Faults::shared();
	hook.lock().unwrap().fail_after(Op::ReadEntry, 2);
	assert!(FileCore::open(&path, &config(&path, SLOT, 10), hook).is_err());
	assert_eq!(bytes_of(&path), bytes);
}
