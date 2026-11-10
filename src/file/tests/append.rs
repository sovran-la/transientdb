//! Appending: slot choice, write order, growth, and a failure at each write.

use super::support::*;
use crate::file::faults::Op;
use crate::file::layout::HEADER_LEN;
use std::io::ErrorKind;

#[test]
fn test_items_of_one_byte_to_the_slot_size_are_stored() {
	let (_dir, path) = temp_path();
	let mut core = open(&path);
	let mut expected = vec!["1".to_string()];
	core.append_text("1").unwrap();
	for len in 2..=SLOT {
		let text = format!("\"{}\"", "a".repeat(len - 2));
		core.append_text(&text).unwrap();
		expected.push(text);
	}
	assert_eq!(texts(&mut core), expected);
	drop(core);
	assert_eq!(texts(&mut open(&path)), expected);
}

#[test]
fn test_an_empty_or_too_long_item_is_refused_with_nothing_written() {
	let (_dir, path) = temp_path();
	let (mut core, hook) = open_with(&path, 10);
	drain(&hook);
	for text in [String::new(), item(SLOT + 1, 0), "x".repeat(SLOT * 4)] {
		let e = core.append_text(&text).unwrap_err();
		assert_eq!(e.kind(), ErrorKind::InvalidInput, "{} bytes", text.len());
	}
	assert!(drain(&hook).is_empty());
	assert_eq!(file_len(&path), HEADER_LEN);
	assert_eq!(core.index.next_sequence(), 1);
}

#[test]
fn test_payload_then_entry_and_nothing_else() {
	let (_dir, path) = temp_path();
	let (mut core, hook) = open_with(&path, 10);
	drain(&hook);
	core.append_text(&item(5, 0)).unwrap();
	assert_eq!(drain(&hook), vec![Op::Payload, Op::Entry]);
	core.append_text(&item(5, 1)).unwrap();
	core.free(1).unwrap();
	drain(&hook);
	// Into a free slot: the same two writes.
	core.append_text(&item(5, 2)).unwrap();
	assert_eq!(drain(&hook), vec![Op::Payload, Op::Entry]);
}

#[test]
fn test_a_new_slot_is_written_whole_so_the_file_ends_on_a_slot() {
	let (_dir, path) = temp_path();
	let mut core = open(&path);
	core.append_text("7").unwrap();
	assert_eq!(file_len(&path), len_of(1));
	let bytes = bytes_of(&path);
	let payload = layout().payload_offset(0) as usize;
	assert_eq!(bytes[payload], b'7');
	assert!(bytes[payload + 1..].iter().all(|&b| b == 0));
	assert_eq!(bytes[layout().entry_offset(0) as usize], 1);
	core.append_text("8").unwrap();
	assert_eq!(file_len(&path), len_of(2));
}

#[test]
fn test_a_reused_slot_gets_only_the_payload_and_the_file_keeps_its_length() {
	let (_dir, path) = temp_path();
	let mut core = open(&path);
	core.append_text(&item(SLOT, 0)).unwrap();
	core.append_text(&item(SLOT, 1)).unwrap();
	core.free(1).unwrap();
	core.append_text("[]").unwrap();
	assert_eq!(slots_of(&core), vec![1, 0]);
	assert_eq!(file_len(&path), len_of(2));
	let bytes = bytes_of(&path);
	let payload = layout().payload_offset(0) as usize;
	// The rest of the old payload is left; the entry's length says where the item ends.
	assert_eq!(&bytes[payload..payload + 2], b"[]");
	assert_eq!(
		&bytes[payload + 2..payload + SLOT],
		&item(SLOT, 0).as_bytes()[2..]
	);
	assert_eq!(texts(&mut core), vec![item(SLOT, 1), "[]".to_string()]);
}

#[test]
fn test_sequences_count_up_from_one() {
	let (_dir, path) = temp_path();
	let mut core = open(&path);
	for n in 1..=20u64 {
		assert_eq!(core.append_text(&item(4, n as usize)).unwrap(), n);
	}
}

#[test]
fn test_a_failed_payload_write_changes_nothing() {
	for (free_slot, what) in [(true, "into a free slot"), (false, "into a new slot")] {
		let (_dir, path) = temp_path();
		let (mut core, hook) = open_with(&path, 10);
		core.append_text(&item(5, 0)).unwrap();
		core.append_text(&item(5, 1)).unwrap();
		if free_slot {
			// The newest item, in slot 1.
			core.free(2).unwrap();
		}
		let index = core.index.clone();
		let len = file_len(&path);
		drain(&hook);
		hook.lock().unwrap().fail(Op::Payload);
		assert!(core.append_text(&item(5, 2)).is_err(), "{what}");
		assert!(drain(&hook).is_empty(), "{what}");
		assert_eq!(core.index, index, "{what}");
		assert_eq!(file_len(&path), len, "{what}");
		hook.lock().unwrap().fail.clear();
		core.append_text(&item(5, 3)).unwrap();
		assert_eq!(texts(&mut open(&path)).last(), Some(&item(5, 3)), "{what}");
	}
}

#[test]
fn test_a_failed_entry_write_writes_the_state_back_to_free() {
	for (free_slot, what) in [(true, "into a free slot"), (false, "into a new slot")] {
		let (_dir, path) = temp_path();
		let (mut core, hook) = open_with(&path, 10);
		core.append_text(&item(5, 0)).unwrap();
		core.append_text(&item(5, 1)).unwrap();
		if free_slot {
			// The newest item, in slot 1.
			core.free(2).unwrap();
		}
		let before = texts(&mut core);
		drain(&hook);
		hook.lock().unwrap().fail(Op::Entry);
		assert!(core.append_text(&item(5, 2)).is_err(), "{what}");
		assert_eq!(drain(&hook), vec![Op::Payload, Op::State], "{what}");
		hook.lock().unwrap().fail.clear();
		assert_eq!(texts(&mut core), before, "{what}");
		// The slot is whole and free; the sequence is spent.
		let slot = if free_slot { 1 } else { 2 };
		assert_eq!(core.index.free().collect::<Vec<_>>(), vec![slot], "{what}");
		assert_eq!(core.index.slots(), slot + 1, "{what}");
		assert_eq!(file_len(&path), len_of(slot + 1), "{what}");
		assert_eq!(core.append_text(&item(5, 3)).unwrap(), 4, "{what}");
		assert_eq!(slots_of(&core).last(), Some(&slot), "{what}");
		drop(core);
		let mut expected = before;
		expected.push(item(5, 3));
		assert_eq!(texts(&mut open(&path)), expected, "{what}");
	}
}

#[test]
fn test_a_failed_entry_write_and_failed_state_write_leave_the_slot_free() {
	let (_dir, path) = temp_path();
	let (mut core, hook) = open_with(&path, 10);
	core.append_text(&item(5, 0)).unwrap();
	hook.lock().unwrap().fail(Op::Entry);
	hook.lock().unwrap().fail(Op::State);
	assert!(core.append_text(&item(5, 1)).is_err());
	hook.lock().unwrap().fail.clear();
	assert_eq!(texts(&mut core), vec![item(5, 0)]);
	assert_eq!(core.index.free().collect::<Vec<_>>(), vec![1]);
	drop(core);
	assert_eq!(texts(&mut open(&path)), vec![item(5, 0)]);
}
