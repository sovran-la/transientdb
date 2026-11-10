//! A kill at every byte boundary of an append, simulated by cutting the write short:
//! into an existing slot, into a new slot at the end of the file, and into the oldest
//! item's slot when the store is full.

use super::support::*;
use crate::file::core::FileCore;
use crate::file::entry::LEN as ENTRY_LEN;
use crate::file::faults::Faults;
use std::path::Path;

/// The bytes of an entry that must reach the file before the item validates: everything
/// up to and including its CRC. The reserved bytes after it are zero already.
const ENTRY_COMMITTED: usize = 20;

/// Appends `text` to a copy of the store file `template` with `budget` bytes allowed to
/// reach it, then reopens it, as the next launch would, and returns the reopened core.
fn killed_append(
	template: &[u8],
	path: &Path,
	max_items: usize,
	text: &str,
	budget: usize,
) -> FileCore {
	std::fs::write(path, template).unwrap();
	let config = config(path, SLOT, max_items);
	let hook = Faults::shared();
	let mut core = FileCore::open(path, &config, hook.clone()).unwrap();
	hook.lock().unwrap().write_budget = Some(budget);
	let result = core.append_text(text);
	drop(core);
	let total = text.len() + ENTRY_LEN;
	assert_eq!(
		result.is_ok(),
		budget >= total,
		"budget {budget} of {total}"
	);
	open_with(path, max_items).0
}

#[test]
fn test_a_kill_at_every_byte_of_an_append_into_an_existing_slot() {
	let (dir, path) = temp_path();
	let mut core = open(&path);
	core.append_text(&item(10, 1)).unwrap();
	core.append_text(&item(10, 2)).unwrap();
	core.free(1).unwrap();
	drop(core);
	let template = bytes_of(&path);
	let text = item(10, 3);
	let total = text.len() + ENTRY_LEN;
	for budget in 0..=total {
		let run = dir.path().join(format!("existing-{budget}.tdb"));
		let mut core = killed_append(&template, &run, 10, &text, budget);
		let mut expected = vec![item(10, 2)];
		if budget >= text.len() + ENTRY_COMMITTED {
			expected.push(text.clone());
		}
		assert_eq!(texts(&mut core), expected, "budget {budget}");
		assert_eq!(file_len(&run), len_of(2), "budget {budget}");
		// The store carries on: the next append takes the free slot, or grows.
		core.append_text(&item(10, 4)).unwrap();
		expected.push(item(10, 4));
		assert_eq!(texts(&mut open(&run)), expected, "budget {budget}");
	}
}

#[test]
fn test_a_kill_at_every_byte_of_an_append_into_a_new_slot() {
	let (dir, path) = temp_path();
	let mut core = open(&path);
	core.append_text(&item(10, 1)).unwrap();
	drop(core);
	let template = bytes_of(&path);
	let text = item(10, 2);
	// A new slot's payload is written whole.
	let total = SLOT + ENTRY_LEN;
	for budget in 0..=total {
		let run = dir.path().join(format!("new-{budget}.tdb"));
		std::fs::write(&run, &template).unwrap();
		let hook = Faults::shared();
		let mut core = FileCore::open(&run, &config(&run, SLOT, 10), hook.clone()).unwrap();
		hook.lock().unwrap().write_budget = Some(budget);
		assert_eq!(
			core.append_text(&text).is_ok(),
			budget == total,
			"budget {budget}"
		);
		drop(core);
		// The file ends wherever the kill cut it.
		let cut = if budget == 0 {
			len_of(1)
		} else if budget <= SLOT {
			len_of(1) + (ENTRY_LEN + budget) as u64
		} else {
			len_of(2)
		};
		assert_eq!(file_len(&run), cut, "budget {budget}");
		let mut core = open(&run);
		let committed = budget >= SLOT + ENTRY_COMMITTED;
		let mut expected = vec![item(10, 1)];
		if committed {
			expected.push(text.clone());
		}
		assert_eq!(texts(&mut core), expected, "budget {budget}");
		// Bytes short of a whole slot are not a slot.
		let whole = if budget >= SLOT { 2 } else { 1 };
		assert_eq!(core.index.slots(), whole, "budget {budget}");
		// The next append overwrites them, and the file ends on a slot again.
		core.append_text(&item(10, 3)).unwrap();
		let slots = if committed { 3 } else { 2 };
		assert_eq!(file_len(&run), len_of(slots), "budget {budget}");
		expected.push(item(10, 3));
		assert_eq!(texts(&mut open(&run)), expected, "budget {budget}");
	}
}

/// The length of the common prefix of `a` and `b`.
fn common_prefix(a: &str, b: &str) -> usize {
	a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

#[test]
fn test_a_kill_at_every_byte_of_an_append_that_drops_the_oldest() {
	let (dir, path) = temp_path();
	let (mut core, _) = open_with(&path, 2);
	core.append_text(&item(10, 1)).unwrap();
	core.append_text(&item(10, 2)).unwrap();
	drop(core);
	let template = bytes_of(&path);
	let text = format!("[{}]", "9".repeat(8));
	let oldest = item(10, 1);
	let same = common_prefix(&oldest, &text);
	let total = text.len() + ENTRY_LEN;
	for budget in 0..=total {
		let run = dir.path().join(format!("drop-{budget}.tdb"));
		let mut core = killed_append(&template, &run, 2, &text, budget);
		// The oldest item survives only while nothing it differs in was overwritten.
		let mut expected = Vec::new();
		if budget <= same {
			expected.push(oldest.clone());
		}
		expected.push(item(10, 2));
		if budget >= text.len() + ENTRY_COMMITTED {
			expected.push(text.clone());
		}
		assert_eq!(texts(&mut core), expected, "budget {budget}");
		assert_eq!(file_len(&run), len_of(2), "budget {budget}");
	}
}

#[test]
fn test_a_kill_between_the_two_writes_of_a_drop() {
	let (_dir, path) = temp_path();
	let (mut core, _) = open_with(&path, 2);
	core.append_text(&item(10, 1)).unwrap();
	core.append_text(&item(10, 2)).unwrap();
	drop(core);
	let template = bytes_of(&path);
	let text = format!("[{}]", "9".repeat(8));
	// Exactly the payload: the oldest item's entry now meets a payload it does not belong
	// with, fails its CRC and is freed at fetch. The new item was never acknowledged.
	let mut core = killed_append(&template, &path, 2, &text, text.len());
	assert_eq!(core.index.live_len(), 2);
	assert_eq!(texts(&mut core), vec![item(10, 2)]);
	assert_eq!(core.index.free().collect::<Vec<_>>(), vec![0]);
	assert_eq!(bytes_of(&path)[layout().entry_offset(0) as usize], 0);
	drop(core);
	let mut core = open_with(&path, 2).0;
	assert_eq!(texts(&mut core), vec![item(10, 2)]);
	core.append_text("[3]").unwrap();
	assert_eq!(slots_of(&core), vec![1, 0]);
}
