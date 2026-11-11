//! Sequences are never handed out twice: an invalid entry's sequence is not counted, a
//! valid one at `u64::MAX` exhausts them loudly, and what lets them start again.

use super::support::*;
use crate::file::core::FileCore;
use crate::file::faults::{Faults, Op};
use crate::file::layout::HEADER_LEN;
use std::io::ErrorKind;
use std::path::Path;

/// Writes `sequence` into slot `slot`'s entry, leaving the rest of it as it is.
fn set_sequence(path: &Path, slot: u64, sequence: u64) {
	poke(
		path,
		layout().entry_offset(slot) + 8,
		&sequence.to_le_bytes(),
	);
}

/// A file holding items 0, 1 and 2 (sequences 1 to 3, slots 0 to 2) with item 1
/// removed, its free entry carrying sequence `u64::MAX`.
fn exhausting_file(path: &Path) {
	let mut core = open(path);
	for tag in 0..3 {
		core.append_text(&item(6, tag)).unwrap();
	}
	assert!(core.free(2).unwrap());
	drop(core);
	set_sequence(path, 1, u64::MAX);
}

fn assert_exhausted(core: &mut FileCore, text: &str) {
	let e = core.append_text(text).unwrap_err();
	assert_eq!(e.kind(), ErrorKind::Other);
	assert_eq!(e.to_string(), "sequence numbers exhausted");
}

#[test]
fn test_an_all_ff_entry_leaves_later_appends_and_every_item_intact() {
	let (_dir, path) = temp_path();
	let mut core = open(&path);
	for tag in 0..3 {
		core.append_text(&item(6, tag)).unwrap();
	}
	drop(core);
	// Slot 1's entry is overwritten with 0xFF: state 255, sequence u64::MAX.
	poke(&path, layout().entry_offset(1), &[0xFF; 32]);
	let mut core = open(&path);
	assert_eq!(core.index.next_sequence(), Some(4));
	let appended: Vec<u64> = (10..14)
		.map(|tag| core.append_text(&item(6, tag)).unwrap())
		.collect();
	assert_eq!(appended, vec![4, 5, 6, 7]);
	// The damaged slot is reused, and nothing acknowledged is lost or leaked.
	let expected = vec![
		item(6, 0),
		item(6, 2),
		item(6, 10),
		item(6, 11),
		item(6, 12),
		item(6, 13),
	];
	assert_eq!(texts(&mut core), expected);
	assert_eq!(
		core.index.live_len() as u64 + core.index.free().count() as u64,
		core.index.slots()
	);
	drop(core);
	let mut core = open(&path);
	assert_eq!(texts(&mut core), expected);
	assert_eq!(core.index.next_sequence(), Some(8));
}

#[test]
fn test_a_free_entry_at_u64_max_makes_every_append_fail_with_nothing_written() {
	for max_items in [100, 2] {
		let (_dir, path) = temp_path();
		exhausting_file(&path);
		let (mut core, hook) = open_with(&path, max_items);
		assert_eq!(core.index.next_sequence(), None);
		drain(&hook);
		let bytes = bytes_of(&path);
		let index = core.index.clone();
		// With room, an append would reuse slot 1; with `max_items` 2, it would drop the
		// oldest. Either way nothing is written and nothing is dropped.
		for tag in 5..8 {
			assert_exhausted(&mut core, &item(6, tag));
		}
		assert!(drain(&hook).is_empty(), "max_items {max_items}");
		assert_eq!(bytes_of(&path), bytes, "max_items {max_items}");
		assert_eq!(core.index, index, "max_items {max_items}");
		assert_eq!(texts(&mut core), vec![item(6, 0), item(6, 2)]);
	}
}

#[test]
fn test_the_last_sequence_is_used_once_then_appends_fail() {
	let (_dir, path) = temp_path();
	exhausting_file(&path);
	set_sequence(&path, 1, u64::MAX - 1);
	let mut core = open(&path);
	assert_eq!(core.append_text(&item(6, 5)).unwrap(), u64::MAX);
	assert_exhausted(&mut core, &item(6, 6));
	drop(core);
	// The live item at u64::MAX is kept, and still exhausts the sequences.
	let mut core = open(&path);
	assert_eq!(texts(&mut core), vec![item(6, 0), item(6, 2), item(6, 5)]);
	assert_exhausted(&mut core, &item(6, 6));
}

#[test]
fn test_a_reset_starts_exhausted_sequences_again() {
	let (_dir, path) = temp_path();
	exhausting_file(&path);
	let mut core = open(&path);
	assert!(core.reset());
	assert_eq!(file_len(&path), HEADER_LEN);
	assert_eq!(core.append_text(&item(6, 5)).unwrap(), 1);
	assert_eq!(core.append_text(&item(6, 6)).unwrap(), 2);
	drop(core);
	assert_eq!(texts(&mut open(&path)), vec![item(6, 5), item(6, 6)]);
}

#[test]
fn test_a_failed_reset_leaves_sequences_exhausted() {
	let (_dir, path) = temp_path();
	exhausting_file(&path);
	let (mut core, hook) = open_with(&path, 100);
	hook.lock().unwrap().fail(Op::Cut);
	assert!(!core.reset());
	hook.lock().unwrap().fail.clear();
	assert_eq!(core.index.next_sequence(), None);
	assert_exhausted(&mut core, &item(6, 5));
}

#[test]
fn test_emptying_the_store_keeps_sequences_exhausted_until_it_is_opened_again() {
	let (_dir, path) = temp_path();
	exhausting_file(&path);
	let mut core = open(&path);
	assert!(core.free(1).unwrap());
	assert!(core.free(3).unwrap());
	// The file is cut, but an item this store handed out may still be held: its
	// sequence must not be given to a new item.
	assert_eq!(file_len(&path), HEADER_LEN);
	assert_exhausted(&mut core, &item(6, 5));
	drop(core);
	let mut core = open(&path);
	assert_eq!(core.index.next_sequence(), Some(1));
	assert_eq!(core.append_text(&item(6, 5)).unwrap(), 1);
}

/// `exhausting_file`, with items 0 and 2 removed as well but the file not cut, so it
/// holds only free entries, one of them at `u64::MAX`.
fn only_free_entries(path: &Path) {
	exhausting_file(path);
	let (mut core, hook) = open_with(path, 100);
	hook.lock().unwrap().fail(Op::Cut);
	assert!(core.free(1).unwrap());
	assert!(core.free(3).unwrap());
	drop(core);
	assert_eq!(file_len(path), len_of(3));
}

#[test]
fn test_opening_a_file_with_no_live_item_starts_exhausted_sequences_again() {
	let (_dir, path) = temp_path();
	only_free_entries(&path);
	let (mut core, hook) = open_with(&path, 100);
	assert_eq!(drain(&hook), vec![Op::Cut]);
	assert_eq!(file_len(&path), HEADER_LEN);
	assert_eq!(core.index.next_sequence(), Some(1));
	assert_eq!(core.append_text(&item(6, 5)).unwrap(), 1);
}

#[test]
fn test_a_failed_cut_at_open_leaves_sequences_exhausted() {
	let (_dir, path) = temp_path();
	only_free_entries(&path);
	let hook = Faults::shared();
	hook.lock().unwrap().fail(Op::Cut);
	let mut core = FileCore::open(&path, &config(&path, SLOT, 100), hook.clone()).unwrap();
	assert_eq!(core.index.next_sequence(), None);
	assert_exhausted(&mut core, &item(6, 5));
	drop(core);
	// The next open makes the cut, and sequences start again.
	assert_eq!(open(&path).append_text(&item(6, 5)).unwrap(), 1);
}
