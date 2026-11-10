//! A slot's 32 byte entry: whether the slot is live, and which item its payload is.
//!
//! | Bytes  | Field |
//! |--------|-------|
//! | 0      | state: `0` free, `1` live |
//! | 1..4   | reserved, zero |
//! | 4..8   | payload length |
//! | 8..16  | sequence |
//! | 16..20 | CRC-32 of sequence, length, then payload |
//! | 20..32 | reserved, zero |
//!
//! The state byte is outside the CRC, so freeing a slot is one one-byte write.

use super::crc::Crc32;
use super::layout::ENTRY_LEN;

pub(super) const FREE: u8 = 0;
pub(super) const LIVE: u8 = 1;

pub(super) const LEN: usize = ENTRY_LEN as usize;

/// An entry as read from the file, before anything about it is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Entry {
	pub(super) state: u8,
	pub(super) length: u32,
	pub(super) sequence: u64,
	pub(super) crc: u32,
}

impl Entry {
	pub(super) fn decode(bytes: &[u8; LEN]) -> Self {
		Self {
			state: bytes[0],
			length: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
			sequence: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
			crc: u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
		}
	}

	/// Whether the entry is live with a length a slot of `slot_size` bytes can hold. Any
	/// other entry is treated as free.
	pub(super) fn is_valid_live(&self, slot_size: u32) -> bool {
		self.state == LIVE && (1..=slot_size).contains(&self.length)
	}

	/// Whether `payload` is the payload this entry was written for.
	pub(super) fn matches(&self, payload: &[u8]) -> bool {
		payload.len() == self.length as usize && crc(self.sequence, payload) == self.crc
	}
}

/// The CRC an entry records for item `sequence` with `payload`: of the sequence, the
/// length, then the payload, so an entry and a payload that do not belong together never
/// validate.
pub(super) fn crc(sequence: u64, payload: &[u8]) -> u32 {
	Crc32::new()
		.update(&sequence.to_le_bytes())
		.update(&(payload.len() as u32).to_le_bytes())
		.update(payload)
		.finish()
}

/// The live entry for item `sequence` with `payload`. The commit point of an append.
pub(super) fn encode_live(sequence: u64, payload: &[u8]) -> [u8; LEN] {
	let mut bytes = [0u8; LEN];
	bytes[0] = LIVE;
	bytes[4..8].copy_from_slice(&(payload.len() as u32).to_le_bytes());
	bytes[8..16].copy_from_slice(&sequence.to_le_bytes());
	bytes[16..20].copy_from_slice(&crc(sequence, payload).to_le_bytes());
	bytes
}
