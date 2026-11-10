//! The in-memory slot index and its choice of slot for an append.

use crate::file::index::{Live, SlotIndex, Target};

#[test]
fn test_a_new_index_has_every_slot_free() {
	let index = SlotIndex::new(3, 9);
	assert_eq!(index.slots(), 3);
	assert_eq!(index.live_len(), 0);
	assert_eq!(index.free().collect::<Vec<_>>(), vec![0, 1, 2]);
	assert_eq!(index.next_sequence(), 9);
	assert_eq!(index.oldest(), None);
}

#[test]
fn test_target_is_lowest_free_then_new_then_oldest_when_full() {
	let mut index = SlotIndex::new(0, 1);
	assert_eq!(index.target(3), Target::New(0));
	index.add_slot(0);
	assert_eq!(index.target(3), Target::Free(0));
	index.set_live(0, 1, 5);
	assert_eq!(index.target(3), Target::New(1));
	index.add_slot(1);
	index.set_live(1, 2, 5);
	index.add_slot(2);
	index.add_slot(3);
	assert_eq!(index.target(3), Target::Free(2));
	index.set_live(3, 3, 5);
	// Full: three live, the oldest is sequence 1 in slot 0, though slot 2 is free.
	assert_eq!(
		index.target(3),
		Target::Oldest {
			sequence: 1,
			slot: 0
		}
	);
	assert_eq!(index.target(4), Target::Free(2));
	// Live past the limit (it was lowered): still the oldest.
	assert_eq!(
		index.target(1),
		Target::Oldest {
			sequence: 1,
			slot: 0
		}
	);
	assert_eq!(Target::New(7).slot(), 7);
	assert_eq!(Target::Free(4).slot(), 4);
	assert_eq!(
		Target::Oldest {
			sequence: 1,
			slot: 2
		}
		.slot(),
		2
	);
}

#[test]
fn test_age_is_by_sequence_not_slot() {
	let mut index = SlotIndex::new(3, 1);
	index.set_live(2, 10, 1);
	index.set_live(0, 30, 1);
	index.set_live(1, 20, 1);
	let order: Vec<u64> = index.live().map(|(s, _)| s).collect();
	assert_eq!(order, vec![10, 20, 30]);
	assert_eq!(index.oldest(), Some((10, Live { slot: 2, len: 1 })));
	assert_eq!(
		index.target(3),
		Target::Oldest {
			sequence: 10,
			slot: 2
		}
	);
}

#[test]
fn test_live_after_walks_in_sequence_order() {
	let mut index = SlotIndex::new(3, 1);
	index.set_live(0, 5, 1);
	index.set_live(1, 7, 2);
	index.set_live(2, u64::MAX, 3);
	assert_eq!(index.live_after(None).map(|(s, _)| s), Some(5));
	assert_eq!(index.live_after(Some(5)).map(|(s, _)| s), Some(7));
	assert_eq!(index.live_after(Some(6)).map(|(s, _)| s), Some(7));
	assert_eq!(index.live_after(Some(7)).map(|(s, _)| s), Some(u64::MAX));
	assert_eq!(index.live_after(Some(u64::MAX)), None);
}

#[test]
fn test_remove_frees_the_slot_and_ignores_what_is_not_live() {
	let mut index = SlotIndex::new(2, 1);
	index.set_live(1, 4, 3);
	assert_eq!(index.get(4), Some(Live { slot: 1, len: 3 }));
	assert_eq!(index.remove(4), Some(1));
	assert_eq!(index.get(4), None);
	assert_eq!(index.remove(4), None);
	assert_eq!(index.remove(99), None);
	assert_eq!(index.free().collect::<Vec<_>>(), vec![0, 1]);
}

#[test]
fn test_add_slot_counts_whole_slots_and_frees_any_skipped() {
	let mut index = SlotIndex::new(1, 1);
	index.add_slot(0);
	assert_eq!(index.slots(), 1);
	index.add_slot(3);
	assert_eq!(index.slots(), 4);
	assert_eq!(index.free().collect::<Vec<_>>(), vec![0, 1, 2, 3]);
}

#[test]
fn test_sequences_are_never_handed_out_twice() {
	let mut index = SlotIndex::new(0, 1);
	assert_eq!(index.take_sequence(), 1);
	assert_eq!(index.take_sequence(), 2);
	assert_eq!(index.next_sequence(), 3);
}

#[test]
fn test_the_last_sequence_saturates_rather_than_wrapping_to_old_ones() {
	// Only a damaged entry can carry a sequence this high; wrapping would hand out
	// sequences of items that may still be stored.
	let mut index = SlotIndex::new(0, 1);
	index.set_next_sequence(u64::MAX);
	assert_eq!(index.take_sequence(), u64::MAX);
	assert_eq!(index.next_sequence(), u64::MAX);
}

#[test]
fn test_clear_slots_keeps_the_sequence() {
	let mut index = SlotIndex::new(2, 1);
	index.take_sequence();
	index.set_live(0, 1, 1);
	index.clear_slots();
	assert_eq!(index.slots(), 0);
	assert_eq!(index.live_len(), 0);
	assert_eq!(index.free().count(), 0);
	assert_eq!(index.next_sequence(), 2);
	assert_eq!(index.target(5), Target::New(0));
}
