//! Shrinking: the file is cut back to its header when no item is live, and only then.

use super::support::*;
use crate::file::faults::Op;
use crate::file::header::encode;
use crate::file::layout::HEADER_LEN;

fn filled(
	path: &std::path::Path,
	n: usize,
	max_items: usize,
) -> (crate::file::core::FileCore, crate::file::faults::Hook) {
	let (mut core, hook) = open_with(path, max_items);
	for tag in 0..n {
		core.append_text(&item(6, tag)).unwrap();
	}
	drain(&hook);
	(core, hook)
}

#[test]
fn test_removing_the_last_live_item_cuts_the_file_to_its_header() {
	for order in permutations(3) {
		let (_dir, path) = temp_path();
		let (mut core, hook) = filled(&path, 3, 10);
		for (n, &tag) in order.iter().enumerate() {
			core.free(tag as u64 + 1).unwrap();
			if n < 2 {
				// Not while any item is live.
				assert_eq!(file_len(&path), len_of(3), "{order:?}");
			}
		}
		assert_eq!(drain(&hook), vec![Op::State, Op::State, Op::State, Op::Cut]);
		assert_eq!(bytes_of(&path), encode(SLOT as u32, ID), "{order:?}");
		assert_eq!(core.index.slots(), 0);
		// The next append grows it again from slot 0.
		core.append_text(&item(6, 9)).unwrap();
		assert_eq!(slots_of(&core), vec![0]);
		assert_eq!(file_len(&path), len_of(1));
		// Sequences carry on.
		assert_eq!(core.index.get(4).map(|live| live.slot), Some(0));
	}
}

#[test]
fn test_a_freed_damaged_last_item_cuts_the_file() {
	let (_dir, path) = temp_path();
	let (mut core, _) = filled(&path, 1, 10);
	poke(&path, layout().payload_offset(0) + 2, b"x");
	assert!(texts(&mut core).is_empty());
	assert_eq!(file_len(&path), HEADER_LEN);
}

#[test]
fn test_a_drop_that_takes_the_last_live_item_cuts_the_file() {
	let (_dir, path) = temp_path();
	let (mut core, hook) = filled(&path, 1, 1);
	// The drop's commit fails: the oldest item is gone and the new one never stored.
	hook.lock().unwrap().fail(Op::Entry);
	assert!(core.append_text(&item(6, 1)).is_err());
	assert_eq!(drain(&hook), vec![Op::Payload, Op::State, Op::Cut]);
	assert_eq!(file_len(&path), HEADER_LEN);
	hook.lock().unwrap().fail.clear();
	core.append_text(&item(6, 2)).unwrap();
	assert_eq!(texts(&mut core), vec![item(6, 2)]);
	assert_eq!(slots_of(&core), vec![0]);
}

#[test]
fn test_reset_cuts_the_file_with_items_stored_or_not() {
	for n in [0, 1, 4] {
		let (_dir, path) = temp_path();
		let (mut core, hook) = filled(&path, n, 10);
		let next = core.index.next_sequence();
		assert!(core.reset());
		assert_eq!(drain(&hook), vec![Op::Cut]);
		assert_eq!(bytes_of(&path), encode(SLOT as u32, ID));
		assert!(texts(&mut core).is_empty());
		assert_eq!(core.append_text(&item(6, 9)).unwrap(), next);
		assert_eq!(slots_of(&core), vec![0]);
		drop(core);
		assert_eq!(texts(&mut open(&path)), vec![item(6, 9)]);
	}
}

#[test]
fn test_a_failed_cut_changes_nothing_and_is_tried_again_next_time() {
	let (_dir, path) = temp_path();
	let (mut core, hook) = filled(&path, 2, 10);
	hook.lock().unwrap().fail(Op::Cut);
	assert!(core.free(1).unwrap());
	// The removal itself succeeded; only the cut failed.
	assert!(core.free(2).unwrap());
	assert_eq!(file_len(&path), len_of(2));
	assert_eq!(core.index.slots(), 2);
	assert_eq!(core.index.free().collect::<Vec<_>>(), vec![0, 1]);
	hook.lock().unwrap().fail.clear();
	// The free slots are used as any free slot: no growth.
	core.append_text(&item(6, 5)).unwrap();
	assert_eq!(slots_of(&core), vec![0]);
	assert_eq!(file_len(&path), len_of(2));
	// The next time no item is live, the cut is made.
	core.free(3).unwrap();
	assert_eq!(file_len(&path), HEADER_LEN);
}

#[test]
fn test_a_failed_reset_changes_nothing() {
	let (_dir, path) = temp_path();
	let (mut core, hook) = filled(&path, 3, 10);
	let index = core.index.clone();
	hook.lock().unwrap().fail(Op::Cut);
	assert!(!core.reset());
	hook.lock().unwrap().fail.clear();
	assert_eq!(core.index, index);
	assert_eq!(file_len(&path), len_of(3));
	assert_eq!(texts(&mut core).len(), 3);
	assert!(core.reset());
	assert_eq!(file_len(&path), HEADER_LEN);
}

#[test]
fn test_open_cuts_a_file_with_no_live_item() {
	let (_dir, path) = temp_path();
	let (mut core, hook) = filled(&path, 2, 10);
	hook.lock().unwrap().fail(Op::Cut);
	core.free(1).unwrap();
	core.free(2).unwrap();
	drop(core);
	assert_eq!(file_len(&path), len_of(2));
	let (core, hook) = open_with(&path, 10);
	assert_eq!(drain(&hook), vec![Op::Cut]);
	assert_eq!(file_len(&path), HEADER_LEN);
	assert_eq!(core.index.slots(), 0);
	// The free entries' sequences were read before the cut.
	assert_eq!(core.index.next_sequence(), 3);
}

#[test]
fn test_a_file_of_only_its_header_is_not_cut() {
	let (_dir, path) = temp_path();
	drop(open(&path));
	let (_core, hook) = open_with(&path, 10);
	assert!(drain(&hook).is_empty());
}
