//! Reading items back: limits, order, and the checks every item passes as it is read: its
//! CRC and UTF-8, not its JSON, which was checked at append.

use super::support::*;
use crate::file::entry::encode_live;
use crate::file::faults::Op;

/// A store at `path` holding `lens.len()` items of those lengths, tags 0 up.
fn stored(path: &std::path::Path, lens: &[usize]) -> crate::file::core::FileCore {
	let mut core = open(path);
	for (tag, &len) in lens.iter().enumerate() {
		core.append_text(&item(len, tag)).unwrap();
	}
	core
}

#[test]
fn test_count_and_max_bytes_bound_the_items_read() {
	let (_dir, path) = temp_path();
	let mut core = stored(&path, &[4, 5, 6, 7]);
	let read = |core: &mut crate::file::core::FileCore, count, max| -> Vec<u64> {
		let items = core.read_items(count, max, |_| false).unwrap();
		items.into_iter().map(|(s, _)| s).collect()
	};
	assert_eq!(read(&mut core, 0, 100), Vec::<u64>::new());
	assert_eq!(read(&mut core, 1, 100), vec![1]);
	assert_eq!(read(&mut core, 3, 100), vec![1, 2, 3]);
	assert_eq!(read(&mut core, 100, 100), vec![1, 2, 3, 4]);
	// Exactly fitting is in; one byte short is out.
	assert_eq!(read(&mut core, 100, 9), vec![1, 2]);
	assert_eq!(read(&mut core, 100, 8), vec![1]);
	assert_eq!(read(&mut core, 100, 4), vec![1]);
	// Smaller than the oldest item: nothing.
	assert_eq!(read(&mut core, 100, 3), Vec::<u64>::new());
	assert_eq!(read(&mut core, 100, 0), Vec::<u64>::new());
	assert_eq!(read(&mut core, 2, usize::MAX), vec![1, 2]);
}

#[test]
fn test_reading_stops_at_the_first_item_that_does_not_fit() {
	let (_dir, path) = temp_path();
	let mut core = stored(&path, &[4, 12, 3]);
	// The third would fit after the first, but would come out of order.
	let items = core.read_items(10, 8, |_| false).unwrap();
	assert_eq!(items, vec![(1, item(4, 0))]);
}

#[test]
fn test_skipped_items_are_passed_over_and_do_not_count() {
	let (_dir, path) = temp_path();
	let mut core = stored(&path, &[4, 4, 4, 4]);
	let items = core.read_items(2, 8, |s| s % 2 == 1).unwrap();
	assert_eq!(items, vec![(2, item(4, 1)), (4, item(4, 3))]);
	assert!(core.read_items(5, 100, |_| true).unwrap().is_empty());
}

#[test]
fn test_order_is_by_sequence_whatever_the_slots() {
	let (_dir, path) = temp_path();
	let mut core = stored(&path, &[4, 4, 4]);
	core.free(1).unwrap();
	core.free(2).unwrap();
	core.append_text(&item(4, 7)).unwrap();
	core.append_text(&item(4, 8)).unwrap();
	assert_eq!(slots_of(&core), vec![2, 0, 1]);
	assert_eq!(texts(&mut core), vec![item(4, 2), item(4, 7), item(4, 8)]);
}

#[test]
fn test_reading_writes_nothing_and_changes_nothing() {
	let (_dir, path) = temp_path();
	let mut core = stored(&path, &[4, 5, 6]);
	let hook = crate::file::faults::Faults::shared();
	drop(core);
	let config = config(&path, SLOT, 100);
	core = crate::file::core::FileCore::open(&path, &config, hook.clone()).unwrap();
	let index = core.index.clone();
	let before = bytes_of(&path);
	for _ in 0..3 {
		assert_eq!(texts(&mut core).len(), 3);
	}
	assert!(drain(&hook).is_empty());
	assert_eq!(bytes_of(&path), before);
	assert_eq!(core.index, index);
}

/// Changes the second of three 5 byte items: damages it in each way an item can fail its
/// checks ("crc", "utf8"), or writes text that is not JSON ("json"). Past the CRC, the
/// entry is rewritten to match the new payload.
fn damage(path: &std::path::Path, how: &str) {
	let slot = 1;
	let entry = layout().entry_offset(slot);
	let payload = layout().payload_offset(slot);
	match how {
		"crc" => poke(path, payload + 1, b"Z"),
		"utf8" => {
			let bad = [b'"', 0xFF, 0xFE, b'"', b' '];
			poke(path, payload, &bad);
			poke(path, entry, &encode_live(2, &bad));
		}
		"json" => {
			let bad = b"[1,2,";
			poke(path, payload, bad);
			poke(path, entry, &encode_live(2, bad));
		}
		_ => unreachable!(),
	}
}

#[test]
fn test_a_damaged_item_is_freed_and_skipped_and_the_rest_still_read() {
	for how in ["crc", "utf8"] {
		let (_dir, path) = temp_path();
		drop(stored(&path, &[5, 5, 5]));
		damage(&path, how);
		let (mut core, hook) = open_with(&path, 100);
		// The damaged item does not use up the count.
		let items = core.read_items(2, 100, |_| false).unwrap();
		assert_eq!(items, vec![(1, item(5, 0)), (3, item(5, 2))], "{how}");
		assert_eq!(drain(&hook), vec![Op::State], "{how}");
		assert_eq!(
			bytes_of(&path)[layout().entry_offset(1) as usize],
			0,
			"{how}"
		);
		assert_eq!(core.index.free().collect::<Vec<_>>(), vec![1], "{how}");
		// It is never read again, by this store or the next.
		assert_eq!(texts(&mut core).len(), 2, "{how}");
		drop(core);
		assert_eq!(texts(&mut open(&path)).len(), 2, "{how}");
	}
}

#[test]
fn test_an_item_that_is_not_json_under_a_valid_crc_is_read_as_it_was_stored() {
	let (_dir, path) = temp_path();
	drop(stored(&path, &[5, 5, 5]));
	damage(&path, "json");
	let (mut core, hook) = open_with(&path, 100);
	let before = bytes_of(&path);
	let items = core.read_items(3, 100, |_| false).unwrap();
	let expected = vec![(1, item(5, 0)), (2, "[1,2,".to_string()), (3, item(5, 2))];
	assert_eq!(items, expected);
	// Nothing is freed or written for it, and it is read the same way again.
	assert!(drain(&hook).is_empty());
	assert_eq!(bytes_of(&path), before);
	assert_eq!(all(&mut core), expected);
	drop(core);
	assert_eq!(all(&mut open(&path)), expected);
}

#[test]
fn test_a_damaged_item_whose_freeing_fails_is_still_skipped() {
	let (_dir, path) = temp_path();
	drop(stored(&path, &[5, 5]));
	damage(&path, "crc");
	let (mut core, hook) = open_with(&path, 100);
	hook.lock().unwrap().fail(Op::State);
	assert_eq!(texts(&mut core), vec![item(5, 0)]);
	assert_eq!(texts(&mut core), vec![item(5, 0)]);
	hook.lock().unwrap().fail.clear();
	drop(core);
	// Still live on disk: the next store finds it damaged, frees it and skips it.
	let (mut core, hook) = open_with(&path, 100);
	assert_eq!(core.index.live_len(), 2);
	assert_eq!(texts(&mut core), vec![item(5, 0)]);
	assert_eq!(drain(&hook), vec![Op::State]);
}

#[test]
fn test_an_item_whose_slot_holds_another_entry_is_forgotten_not_freed() {
	for change in ["sequence", "length", "state"] {
		let (_dir, path) = temp_path();
		let (mut core, hook) = open_with(&path, 100);
		core.append_text(&item(5, 0)).unwrap();
		core.append_text(&item(5, 1)).unwrap();
		drain(&hook);
		// Another writer changes slot 0 behind this store's back.
		let entry = layout().entry_offset(0);
		match change {
			"sequence" => poke(&path, entry, &encode_live(77, item(5, 9).as_bytes())),
			"length" => poke(&path, entry, &encode_live(1, b"[1]")),
			_ => poke(&path, entry, &[0]),
		}
		let before = bytes_of(&path);
		assert_eq!(texts(&mut core), vec![item(5, 1)], "{change}");
		assert!(drain(&hook).is_empty(), "{change}");
		assert_eq!(bytes_of(&path), before, "{change}");
		assert_eq!(core.index.free().collect::<Vec<_>>(), vec![0], "{change}");
	}
}

#[test]
fn test_an_item_past_the_end_of_the_file_is_forgotten() {
	let (_dir, path) = temp_path();
	let mut core = stored(&path, &[5, 5]);
	// Another writer cut the file inside slot 1.
	let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
	file.set_len(len_of(1) + 10).unwrap();
	assert_eq!(texts(&mut core), vec![item(5, 0)]);
	assert_eq!(core.index.live_len(), 1);
}

#[test]
fn test_an_io_error_part_way_returns_nothing_and_changes_nothing() {
	for at in 0..3 {
		let (_dir, path) = temp_path();
		drop(stored(&path, &[5, 5, 5]));
		let (mut core, hook) = open_with(&path, 100);
		let index = core.index.clone();
		hook.lock().unwrap().fail_after(Op::ReadItem, at);
		assert!(core.read_items(3, 100, |_| false).is_err(), "at {at}");
		hook.lock().unwrap().fail.clear();
		assert_eq!(core.index, index, "at {at}");
		assert_eq!(texts(&mut core).len(), 3, "at {at}");
	}
}
