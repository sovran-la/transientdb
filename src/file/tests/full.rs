//! A full store drops its oldest item; `max_items` raised and lowered between runs.

use super::support::*;
use crate::file::faults::Op;

#[test]
fn test_a_full_store_drops_the_oldest_into_its_slot() {
	let (_dir, path) = temp_path();
	let (mut core, _) = open_with(&path, 3);
	for tag in 0..3 {
		core.append_text(&item(6, tag)).unwrap();
	}
	for tag in 3..10 {
		core.append_text(&item(6, tag)).unwrap();
		let expected: Vec<String> = (tag - 2..=tag).map(|t| item(6, t)).collect();
		assert_eq!(texts(&mut core), expected, "after {tag}");
		// Each drop reuses the dropped item's slot: the file never grows.
		assert_eq!(file_len(&path), len_of(3), "after {tag}");
		assert_eq!(
			slots_of(&core).last(),
			Some(&(tag as u64 % 3)),
			"after {tag}"
		);
	}
	drop(core);
	assert_eq!(
		texts(&mut open_with(&path, 3).0),
		vec![item(6, 7), item(6, 8), item(6, 9)]
	);
}

#[test]
fn test_the_oldest_is_by_sequence_not_slot() {
	let (_dir, path) = temp_path();
	let (mut core, _) = open_with(&path, 3);
	for tag in 0..3 {
		core.append_text(&item(6, tag)).unwrap();
	}
	// Slot 1 is freed and refilled: its item is now the newest.
	core.free(2).unwrap();
	core.append_text(&item(6, 3)).unwrap();
	core.append_text(&item(6, 4)).unwrap();
	assert_eq!(texts(&mut core), vec![item(6, 2), item(6, 3), item(6, 4)]);
	assert_eq!(slots_of(&core), vec![2, 1, 0]);
	core.append_text(&item(6, 5)).unwrap();
	assert_eq!(slots_of(&core), vec![1, 0, 2]);
}

#[test]
fn test_full_means_max_items_live_even_with_slots_free() {
	let (_dir, path) = temp_path();
	let (mut core, _) = open_with(&path, 4);
	for tag in 0..4 {
		core.append_text(&item(6, tag)).unwrap();
	}
	drop(core);
	// Lowered to 2: the oldest two are dropped, their slots free.
	let (mut core, _) = open_with(&path, 2);
	assert_eq!(core.index.free().collect::<Vec<_>>(), vec![0, 1]);
	core.append_text(&item(6, 4)).unwrap();
	// Full again, with slots 0 and 1 free: the oldest is dropped, no free slot used.
	assert_eq!(slots_of(&core), vec![3, 2]);
	assert_eq!(texts(&mut core), vec![item(6, 3), item(6, 4)]);
}

#[test]
fn test_removing_an_item_dropped_after_it_was_fetched_is_ignored() {
	let (_dir, path) = temp_path();
	let (mut core, hook) = open_with(&path, 2);
	core.append_text(&item(6, 0)).unwrap();
	core.append_text(&item(6, 1)).unwrap();
	let fetched = core.read_items(1, usize::MAX, |_| false).unwrap();
	assert_eq!(fetched, vec![(1, item(6, 0))]);
	core.append_text(&item(6, 2)).unwrap();
	drain(&hook);
	assert!(!core.free(1).unwrap());
	assert!(drain(&hook).is_empty());
	assert_eq!(texts(&mut core), vec![item(6, 1), item(6, 2)]);
}

#[test]
fn test_max_items_lowered_between_runs_drops_the_oldest() {
	let (_dir, path) = temp_path();
	let (mut core, _) = open_with(&path, 5);
	for tag in 0..5 {
		core.append_text(&item(6, tag)).unwrap();
	}
	drop(core);
	let (mut core, hook) = open_with(&path, 2);
	assert_eq!(drain(&hook), vec![Op::State; 3]);
	assert_eq!(texts(&mut core), vec![item(6, 3), item(6, 4)]);
	// The file keeps its length: those slots are free for reuse.
	assert_eq!(file_len(&path), len_of(5));
	core.append_text(&item(6, 5)).unwrap();
	assert_eq!(texts(&mut core), vec![item(6, 4), item(6, 5)]);
	drop(core);
	// Raised again: what was dropped stays dropped.
	let (mut core, hook) = open_with(&path, 5);
	assert!(drain(&hook).is_empty());
	assert_eq!(texts(&mut core), vec![item(6, 4), item(6, 5)]);
}

#[test]
fn test_a_failed_drop_at_open_is_logged_and_the_next_open_drops_again() {
	let (_dir, path) = temp_path();
	let (mut core, _) = open_with(&path, 3);
	for tag in 0..3 {
		core.append_text(&item(6, tag)).unwrap();
	}
	drop(core);
	let hook = crate::file::faults::Faults::shared();
	hook.lock().unwrap().fail(Op::State);
	let config = config(&path, SLOT, 1);
	let mut core = crate::file::core::FileCore::open(&path, &config, hook.clone()).unwrap();
	hook.lock().unwrap().fail.clear();
	assert_eq!(texts(&mut core), vec![item(6, 2)]);
	assert_eq!(core.index.free().collect::<Vec<_>>(), vec![0, 1]);
	drop(core);
	let (mut core, hook) = open_with(&path, 1);
	assert_eq!(drain(&hook), vec![Op::State; 2]);
	assert_eq!(texts(&mut core), vec![item(6, 2)]);
}

#[test]
fn test_max_items_raised_between_runs_lets_the_file_grow() {
	let (_dir, path) = temp_path();
	let (mut core, _) = open_with(&path, 3);
	for tag in 0..4 {
		core.append_text(&item(6, tag)).unwrap();
	}
	assert_eq!(file_len(&path), len_of(3));
	drop(core);
	let (mut core, hook) = open_with(&path, 5);
	assert!(drain(&hook).is_empty());
	assert_eq!(texts(&mut core), vec![item(6, 1), item(6, 2), item(6, 3)]);
	core.append_text(&item(6, 4)).unwrap();
	core.append_text(&item(6, 5)).unwrap();
	assert_eq!(file_len(&path), len_of(5));
	core.append_text(&item(6, 6)).unwrap();
	assert_eq!(file_len(&path), len_of(5));
	let expected: Vec<String> = (2..=6).map(|t| item(6, t)).collect();
	assert_eq!(texts(&mut core), expected);
}
