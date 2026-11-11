//! Creating and opening the file: steps 1 to 6 of Open, without the lock.

use super::support::*;
use crate::file::core::FileCore;
use crate::file::faults::{Faults, Op};
use crate::file::header::encode;
use crate::file::layout::HEADER_LEN;

#[test]
fn test_create_writes_the_header_and_nothing_else() {
	let (_dir, path) = temp_path();
	let (core, hook) = open_with(&path, 10);
	assert_eq!(bytes_of(&path), encode(SLOT as u32, ID));
	assert_eq!(drain(&hook), vec![Op::Create, Op::Header]);
	assert_eq!(core.index.slots(), 0);
	assert_eq!(core.index.next_sequence(), Some(1));
}

#[test]
fn test_reopening_keeps_every_item_and_its_sequence() {
	let (_dir, path) = temp_path();
	let mut core = open(&path);
	for tag in 0..5 {
		assert_eq!(core.append_text(&item(6, tag)).unwrap(), tag as u64 + 1);
	}
	let before = all(&mut core);
	drop(core);
	let (mut core, hook) = open_with(&path, 100);
	// Opening a matching file writes nothing.
	assert!(drain(&hook).is_empty());
	assert_eq!(all(&mut core), before);
	assert_eq!(slots_of(&core), vec![0, 1, 2, 3, 4]);
	assert_eq!(core.append_text(&item(6, 9)).unwrap(), 6);
}

#[test]
fn test_next_sequence_follows_the_highest_entry_live_or_free() {
	let (_dir, path) = temp_path();
	let mut core = open(&path);
	for tag in 0..4 {
		core.append_text(&item(4, tag)).unwrap();
	}
	// The newest two are removed: their entries are free but keep their sequences.
	assert!(core.free(4).unwrap());
	assert!(core.free(3).unwrap());
	drop(core);
	let mut core = open(&path);
	assert_eq!(core.index.next_sequence(), Some(5));
	assert_eq!(core.append_text(&item(4, 7)).unwrap(), 5);
	drop(core);
	// Slot 3 holds item 4's free entry. A free entry with a valid length counts,
	// whatever its sequence.
	poke(&path, layout().entry_offset(3) + 8, &900u64.to_le_bytes());
	assert_eq!(open(&path).index.next_sequence(), Some(901));
}

#[test]
fn test_an_invalid_entrys_sequence_does_not_count() {
	// An entry that open frees as invalid carries no trustworthy sequence; one at or
	// near u64::MAX would otherwise use up every sequence after it.
	let damage: [(&str, u8, Option<u32>); 4] = [
		("state 9", 9, None),
		("state 255", 255, None),
		("live, length 0", 1, Some(0)),
		("free, length past the slot", 0, Some(SLOT as u32 + 1)),
	];
	for (what, state, length) in damage {
		let (_dir, path) = temp_path();
		let mut core = open(&path);
		for tag in 0..3 {
			core.append_text(&item(4, tag)).unwrap();
		}
		drop(core);
		let entry = layout().entry_offset(2);
		poke(&path, entry + 8, &u64::MAX.to_le_bytes());
		poke(&path, entry, &[state]);
		if let Some(length) = length {
			poke(&path, entry + 4, &length.to_le_bytes());
		}
		let mut core = open(&path);
		assert_eq!(core.index.next_sequence(), Some(3), "{what}");
		assert_eq!(core.append_text(&item(4, 7)).unwrap(), 3, "{what}");
		assert_eq!(core.append_text(&item(4, 8)).unwrap(), 4, "{what}");
	}
}

#[test]
fn test_entries_neither_free_nor_validly_live_are_freed() {
	let damage: [(&str, u8, Option<u32>); 4] = [
		("state 2", 2, None),
		("state 255", 255, None),
		("length 0", 1, Some(0)),
		("length past the slot", 1, Some(SLOT as u32 + 1)),
	];
	for (what, state, length) in damage {
		let (_dir, path) = temp_path();
		let mut core = open(&path);
		for tag in 0..3 {
			core.append_text(&item(5, tag)).unwrap();
		}
		drop(core);
		let entry = layout().entry_offset(1);
		poke(&path, entry, &[state]);
		if let Some(length) = length {
			poke(&path, entry + 4, &length.to_le_bytes());
		}
		let (mut core, hook) = open_with(&path, 100);
		assert_eq!(drain(&hook), vec![Op::State], "{what}");
		assert_eq!(bytes_of(&path)[entry as usize], 0, "{what}");
		assert_eq!(texts(&mut core), vec![item(5, 0), item(5, 2)], "{what}");
		assert_eq!(core.index.free().collect::<Vec<_>>(), vec![1], "{what}");
		// The freed slot is the next one used.
		core.append_text(&item(5, 9)).unwrap();
		assert_eq!(slots_of(&core), vec![0, 2, 1], "{what}");
	}
}

#[test]
fn test_a_free_entry_is_left_as_it_is_whatever_else_it_holds() {
	let (_dir, path) = temp_path();
	let mut core = open(&path);
	core.append_text(&item(5, 0)).unwrap();
	core.append_text(&item(5, 1)).unwrap();
	drop(core);
	let entry = layout().entry_offset(0);
	poke(&path, entry, &[0]);
	poke(&path, entry + 4, &u32::MAX.to_le_bytes());
	let (mut core, hook) = open_with(&path, 100);
	assert!(drain(&hook).is_empty());
	assert_eq!(texts(&mut core), vec![item(5, 1)]);
}

#[test]
fn test_two_live_entries_with_one_sequence_keep_the_first() {
	let (_dir, path) = temp_path();
	let mut core = open(&path);
	core.append_text(&item(5, 0)).unwrap();
	core.append_text(&item(5, 1)).unwrap();
	drop(core);
	// Slot 1 becomes a copy of slot 0, sequence and all.
	let bytes = bytes_of(&path);
	let slot0 = layout().entry_offset(0) as usize..layout().entry_offset(1) as usize;
	poke(&path, layout().entry_offset(1), &bytes[slot0]);
	let (mut core, hook) = open_with(&path, 100);
	assert_eq!(drain(&hook), vec![Op::State]);
	assert_eq!(all(&mut core), vec![(1, item(5, 0))]);
	assert_eq!(slots_of(&core), vec![0]);
}

#[test]
fn test_no_payload_is_read_at_open() {
	let (_dir, path) = temp_path();
	let mut core = open(&path);
	core.append_text(&item(5, 0)).unwrap();
	core.append_text(&item(5, 1)).unwrap();
	drop(core);
	// A damaged payload is not noticed at open.
	poke(&path, layout().payload_offset(0), b"!");
	let hook = Faults::shared();
	hook.lock().unwrap().fail(Op::ReadItem);
	let config = config(&path, SLOT, 100);
	let mut core = FileCore::open(&path, &config, hook.clone()).unwrap();
	assert_eq!(core.index.live_len(), 2);
	hook.lock().unwrap().fail.clear();
	// It is at fetch.
	assert_eq!(texts(&mut core), vec![item(5, 1)]);
}

#[test]
fn test_a_path_that_is_a_directory_is_an_error() {
	let (dir, _path) = temp_path();
	let config = config(dir.path(), SLOT, 10);
	assert!(FileCore::open(dir.path(), &config, Faults::shared()).is_err());
	assert!(dir.path().is_dir());
}

#[test]
fn test_every_file_length_from_0_to_two_whole_slots() {
	let (dir, reference) = temp_path();
	let mut core = open(&reference);
	core.append_text(&item(3, 0)).unwrap();
	core.append_text(&item(SLOT, 1)).unwrap();
	drop(core);
	let full = bytes_of(&reference);
	assert_eq!(full.len() as u64, len_of(2));
	for len in 0..=full.len() {
		let path = dir.path().join(format!("len-{len}.tdb"));
		std::fs::write(&path, &full[..len]).unwrap();
		let mut core = open(&path);
		let whole = layout().slots_in(len as u64);
		let expected: Vec<String> = [item(3, 0), item(SLOT, 1)][..whole as usize].to_vec();
		assert_eq!(texts(&mut core), expected, "length {len}");
		assert_eq!(core.index.slots(), whole, "length {len}");
		let after_open = if (len as u64) < HEADER_LEN || whole == 0 {
			// Cut short in the header: created again. No whole slot: no item is live, so
			// the file is cut back to its header.
			HEADER_LEN
		} else {
			len as u64
		};
		assert_eq!(file_len(&path), after_open, "length {len}");
		if (len as u64) < HEADER_LEN {
			assert_eq!(bytes_of(&path), encode(SLOT as u32, ID), "length {len}");
		}
		// The next item goes in the first slot past the whole ones, overwriting any
		// partial slot, and the file then ends on a slot boundary.
		core.append_text(&item(4, 7)).unwrap();
		assert_eq!(slots_of(&core).last(), Some(&whole), "length {len}");
		assert_eq!(file_len(&path), len_of(whole + 1), "length {len}");
		drop(core);
		let mut expected = expected;
		expected.push(item(4, 7));
		assert_eq!(texts(&mut open(&path)), expected, "length {len}");
	}
}

#[test]
fn test_max_items_is_not_part_of_the_layout() {
	let (_dir, path) = temp_path();
	let (mut core, _) = open_with(&path, 3);
	core.append_text(&item(4, 0)).unwrap();
	drop(core);
	for max_items in [1, 2, 3, 1000] {
		let (mut core, hook) = open_with(&path, max_items);
		assert!(drain(&hook).is_empty(), "max_items {max_items}");
		assert_eq!(texts(&mut core), vec![item(4, 0)]);
	}
}
