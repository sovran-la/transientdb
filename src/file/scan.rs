//! Opening a file whose header matched: reading every slot's entry into the index.

use super::core::FileCore;
use super::entry::{self, Entry, FREE};
use super::faults::Op;
use super::handle::SlotFile;
use super::index::SlotIndex;
use super::layout::{Layout, HEADER_LEN};
use std::io;
use std::path::Path;

impl FileCore {
	/// Builds the index from the entry of every whole slot in a file of `len` bytes whose
	/// header matched. See [`FileCore::open`].
	pub(super) fn scan(
		mut file: SlotFile,
		path: &Path,
		layout: Layout,
		len: u64,
		max_items: usize,
	) -> io::Result<Self> {
		let slots = layout.slots_in(len);
		let mut index = SlotIndex::new(slots, 1);
		let mut highest = 0;
		let mut cleared = Vec::new();
		for slot in 0..slots {
			let mut bytes = [0u8; entry::LEN];
			file.read_at(layout.entry_offset(slot), &mut bytes, Op::ReadEntry)?;
			let entry = Entry::decode(&bytes);
			highest = highest.max(entry.sequence);
			if !entry.is_valid_live(layout.slot_size) {
				if entry.state != FREE {
					cleared.push((slot, "its entry is neither free nor validly live"));
				}
			} else if index.get(entry.sequence).is_some() {
				cleared.push((slot, "its item's sequence is live in another slot"));
			} else {
				index.set_live(slot, entry.sequence, entry.length);
			}
		}
		let mut core = FileCore {
			file,
			path: path.to_path_buf(),
			layout,
			max_items,
			index: SlotIndex::default(),
		};
		// Clearing waits until every entry is read, so an error reading one leaves the
		// file as it was.
		for (slot, why) in cleared {
			core.clear_state(slot, why);
		}
		index.set_next_sequence(highest.saturating_add(1));
		core.index = index;
		core.drop_excess();
		if core.index.live_len() == 0 && len > HEADER_LEN {
			core.cut_to_header();
		}
		Ok(core)
	}

	/// Writes `slot`'s state byte as free, logging why, and logging rather than returning
	/// a failure: the slot is free in the index either way.
	fn clear_state(&mut self, slot: u64, why: &str) {
		let offset = self.layout.entry_offset(slot);
		match self.file.write_at(offset, &[FREE], Op::State) {
			Ok(()) => self.log(format_args!("freed slot {slot}: {why}")),
			Err(e) => self.log(format_args!(
				"slot {slot} is treated as free ({why}), but clearing it failed: {e}"
			)),
		}
	}

	/// Drops the oldest items until no more than `max_items` are live: the limit was
	/// lowered since the file was written.
	fn drop_excess(&mut self) {
		let excess = self.index.live_len().saturating_sub(self.max_items);
		if excess == 0 {
			return;
		}
		for _ in 0..excess {
			let Some((sequence, live)) = self.index.oldest() else {
				break;
			};
			self.index.remove(sequence);
			let offset = self.layout.entry_offset(live.slot);
			if let Err(e) = self.file.write_at(offset, &[FREE], Op::State) {
				self.log(format_args!(
					"dropping item {sequence} for max_items, clearing slot {} failed: {e}",
					live.slot
				));
			}
		}
		self.log(format_args!(
			"dropped the {excess} oldest items: more were stored than max_items ({}) allows",
			self.max_items
		));
	}
}
