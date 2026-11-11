//! What the store knows about its slots without reading the file: which are live, with
//! which item, and which are free.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;

/// A live slot's item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Live {
	/// The slot holding it.
	pub(super) slot: u64,
	/// Its payload length: the item's serialized length.
	pub(super) len: u32,
}

/// Where the next append goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
	/// The store is full: the oldest item's slot, dropping it.
	Oldest { sequence: u64, slot: u64 },
	/// The free slot with the lowest index.
	Free(u64),
	/// A new slot at the end of the file.
	New(u64),
}

impl Target {
	pub(super) fn slot(self) -> u64 {
		match self {
			Self::Oldest { slot, .. } | Self::Free(slot) | Self::New(slot) => slot,
		}
	}
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct SlotIndex {
	/// Live items by sequence, so the first is the oldest.
	live: BTreeMap<u64, Live>,
	/// Free slots by index, so the first is the lowest.
	free: BTreeSet<u64>,
	/// The slots the file holds whole. Every one is live or free.
	slots: u64,
	/// The sequence the next append gets.
	next_sequence: u64,
}

impl SlotIndex {
	/// An index for a file of `slots` whole slots, every one free, whose next item gets
	/// sequence `next_sequence`.
	pub(super) fn new(slots: u64, next_sequence: u64) -> Self {
		Self {
			live: BTreeMap::new(),
			free: (0..slots).collect(),
			slots,
			next_sequence,
		}
	}

	pub(super) fn live_len(&self) -> usize {
		self.live.len()
	}

	pub(super) fn slots(&self) -> u64 {
		self.slots
	}

	#[cfg(test)]
	pub(super) fn next_sequence(&self) -> u64 {
		self.next_sequence
	}

	pub(super) fn get(&self, sequence: u64) -> Option<Live> {
		self.live.get(&sequence).copied()
	}

	/// Live items, oldest first.
	pub(super) fn live(&self) -> impl Iterator<Item = (u64, Live)> + '_ {
		self.live.iter().map(|(&sequence, &live)| (sequence, live))
	}

	/// The oldest live item newer than `after`, or the oldest of all for `None`.
	pub(super) fn live_after(&self, after: Option<u64>) -> Option<(u64, Live)> {
		let start = after.map_or(Bound::Unbounded, Bound::Excluded);
		let mut rest = self.live.range((start, Bound::Unbounded));
		rest.next().map(|(&sequence, &live)| (sequence, live))
	}

	/// Free slots, lowest first.
	#[cfg(test)]
	pub(super) fn free(&self) -> impl Iterator<Item = u64> + '_ {
		self.free.iter().copied()
	}

	/// The oldest live item.
	pub(super) fn oldest(&self) -> Option<(u64, Live)> {
		self.live().next()
	}

	/// Where an append goes when the store holds at most `max_items`: the oldest item's
	/// slot when that many are live, otherwise the lowest free slot, otherwise a new one.
	pub(super) fn target(&self, max_items: usize) -> Target {
		if self.live.len() >= max_items {
			if let Some((sequence, live)) = self.oldest() {
				return Target::Oldest {
					sequence,
					slot: live.slot,
				};
			}
		}
		match self.free.first() {
			Some(&slot) => Target::Free(slot),
			None => Target::New(self.slots),
		}
	}

	pub(super) fn set_next_sequence(&mut self, next_sequence: u64) {
		self.next_sequence = next_sequence;
	}

	/// Takes the next sequence. It is never handed out again, whether or not the
	/// append it is for commits.
	pub(super) fn take_sequence(&mut self) -> u64 {
		let sequence = self.next_sequence;
		self.next_sequence = sequence.saturating_add(1);
		sequence
	}

	/// Records that the file now holds slot `slot` whole, as a free slot if it was not
	/// one already.
	pub(super) fn add_slot(&mut self, slot: u64) {
		if slot >= self.slots {
			self.free.extend(self.slots..=slot);
			self.slots = slot + 1;
		}
	}

	/// Records `slot`, which the file holds, as live with item `sequence`.
	pub(super) fn set_live(&mut self, slot: u64, sequence: u64, len: u32) {
		debug_assert!(slot < self.slots, "slot {slot} is not in the file");
		self.free.remove(&slot);
		self.live.insert(sequence, Live { slot, len });
	}

	/// Records item `sequence` as gone and its slot as free. Returns its slot, or `None`
	/// if it was not live.
	pub(super) fn remove(&mut self, sequence: u64) -> Option<u64> {
		let live = self.live.remove(&sequence)?;
		self.free.insert(live.slot);
		Some(live.slot)
	}

	/// Records that the file was cut back to its header: no slots, nothing live.
	pub(super) fn clear_slots(&mut self) {
		self.live.clear();
		self.free.clear();
		self.slots = 0;
	}
}
