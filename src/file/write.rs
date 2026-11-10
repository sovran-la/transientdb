//! Everything that writes to a store's file after open: appending, freeing, cutting it
//! back to its header.

use super::core::FileCore;
use super::entry::{self, FREE};
use super::faults::Op;
use super::index::Target;
use super::layout::HEADER_LEN;
use std::io;

impl FileCore {
	/// Stores `text`, one item's serialized JSON, and returns its sequence.
	///
	/// The item goes into the oldest item's slot if `max_items` are live, dropping that
	/// item; otherwise into the lowest free slot; otherwise into a new slot at the end of
	/// the file. The payload is written first, then the entry, which is the commit point.
	///
	/// A new slot's payload is written zero padded to the whole slot, so the one write
	/// that grows the file leaves it ending on a slot boundary and the slot is counted at
	/// the next open, however short the item.
	///
	/// If the payload write fails, the entry was never touched: nothing changed, though
	/// when dropping, the oldest item's payload may be damaged, which its next fetch
	/// catches. If the entry write fails, the slot's state is written back to free and the
	/// error returned; when dropping, the oldest item is gone either way.
	///
	/// `text` must be 1 to `max_item_size` bytes; anything else is `InvalidInput`, with
	/// nothing written.
	pub(super) fn append_text(&mut self, text: &str) -> io::Result<u64> {
		let payload = text.as_bytes();
		let slot_size = self.layout.slot_size as usize;
		if payload.is_empty() || payload.len() > slot_size {
			return Err(io::Error::new(
				io::ErrorKind::InvalidInput,
				format!(
					"an item must be 1 to {slot_size} bytes, not {}",
					payload.len()
				),
			));
		}
		let target = self.index.target(self.max_items);
		let slot = target.slot();
		let payload_offset = self.layout.payload_offset(slot);
		if let Target::New(_) = target {
			let mut whole = vec![0u8; slot_size];
			whole[..payload.len()].copy_from_slice(payload);
			self.file.write_at(payload_offset, &whole, Op::Payload)?;
			self.index.add_slot(slot);
		} else {
			self.file.write_at(payload_offset, payload, Op::Payload)?;
		}

		let sequence = self.index.take_sequence();
		let entry_offset = self.layout.entry_offset(slot);
		let committed = self.file.write_at(
			entry_offset,
			&entry::encode_live(sequence, payload),
			Op::Entry,
		);
		if let Target::Oldest {
			sequence: oldest, ..
		} = target
		{
			self.index.remove(oldest);
			self.log(format_args!(
				"dropped item {oldest}, the oldest: the store holds {} items at most",
				self.max_items
			));
		}
		if let Err(e) = committed {
			if let Err(state) = self.file.write_at(entry_offset, &[FREE], Op::State) {
				self.log(format_args!(
					"writing item {sequence}'s entry failed ({e}), and so did freeing slot {slot}: {state}"
				));
			}
			self.shrink_if_empty();
			return Err(e);
		}
		self.index.set_live(slot, sequence, payload.len() as u32);
		Ok(sequence)
	}

	/// Removes item `sequence`: writes its slot's state byte as free. Returns whether it
	/// was live; anything else is ignored.
	///
	/// If the write fails, that is logged and returned, and the item stays live. If this
	/// was the last live item, the file is cut back to its header.
	pub(super) fn free(&mut self, sequence: u64) -> io::Result<bool> {
		let Some(live) = self.index.get(sequence) else {
			return Ok(false);
		};
		let offset = self.layout.entry_offset(live.slot);
		if let Err(e) = self.file.write_at(offset, &[FREE], Op::State) {
			self.log(format_args!("removing item {sequence} failed: {e}"));
			return Err(e);
		}
		self.index.remove(sequence);
		self.shrink_if_empty();
		Ok(true)
	}

	/// Frees a damaged item found at fetch, logging why. Writing its state byte is
	/// attempted, and logged if it fails; the item is gone from the index either way, so
	/// it is never fetched again by this store. The next open finds it damaged again.
	pub(super) fn discard(&mut self, sequence: u64, why: &str) {
		let Some(slot) = self.index.remove(sequence) else {
			return;
		};
		let offset = self.layout.entry_offset(slot);
		match self.file.write_at(offset, &[FREE], Op::State) {
			Ok(()) => self.log(format_args!("freed item {sequence} in slot {slot}: {why}")),
			Err(e) => self.log(format_args!(
				"skipping item {sequence} in slot {slot} ({why}), but freeing it failed: {e}"
			)),
		}
		self.shrink_if_empty();
	}

	/// Cuts the file back to its header if no item is live.
	pub(super) fn shrink_if_empty(&mut self) {
		if self.index.live_len() == 0 && self.index.slots() > 0 {
			self.cut_to_header();
		}
	}

	/// Cuts the file back to its header, removing every slot. Whether or not it
	/// completes, the file is valid. If it fails, that is logged and nothing changes.
	pub(super) fn cut_to_header(&mut self) -> bool {
		match self.file.set_len(HEADER_LEN) {
			Ok(()) => {
				self.index.clear_slots();
				true
			}
			Err(e) => {
				self.log(format_args!(
					"cutting the file back to its header failed: {e}"
				));
				false
			}
		}
	}

	/// Removes every item by cutting the file back to its header. Sequences carry on
	/// from where they were. If the cut fails, that is logged and nothing changes.
	pub(super) fn reset(&mut self) -> bool {
		self.cut_to_header()
	}
}
