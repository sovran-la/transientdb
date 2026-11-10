//! Where things are in a store's file: a 256 byte header, then slots of one size.

/// Bytes in the header at the start of the file.
pub(super) const HEADER_LEN: u64 = 256;
/// Bytes in each slot's entry, before its payload.
pub(super) const ENTRY_LEN: u64 = 32;

/// The positions of a file's slots, for one slot size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Layout {
	/// Payload bytes per slot: the largest item the file holds.
	pub(super) slot_size: u32,
}

impl Layout {
	/// Bytes per slot, entry and payload.
	pub(super) fn slot_len(self) -> u64 {
		ENTRY_LEN + u64::from(self.slot_size)
	}

	/// Where slot `index` starts: its entry.
	pub(super) fn entry_offset(self, index: u64) -> u64 {
		HEADER_LEN + self.slot_len() * index
	}

	/// Where slot `index`'s payload starts, right after its entry.
	pub(super) fn payload_offset(self, index: u64) -> u64 {
		self.entry_offset(index) + ENTRY_LEN
	}

	/// The number of whole slots in a file of `len` bytes. Bytes past the last whole slot
	/// are not a slot.
	pub(super) fn slots_in(self, len: u64) -> u64 {
		len.saturating_sub(HEADER_LEN) / self.slot_len()
	}
}
