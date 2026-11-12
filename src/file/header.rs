//! The file's 256 byte header: what the file is, whose data it holds, and its slot size.
//!
//! | Bytes    | Field          |
//! |----------|----------------|
//! | 0..8     | magic `TDBFILE\0` |
//! | 8..12    | format version |
//! | 12..16   | slot size      |
//! | 16..20   | reserved, zero |
//! | 20..24   | id length      |
//! | 24..248  | id, zero padded |
//! | 248..252 | reserved, zero |
//! | 252..256 | CRC-32 of 0..252 |

use super::crc::crc32;
use super::layout::HEADER_LEN;
use std::fmt;

pub(super) const MAGIC: &[u8; 8] = b"TDBFILE\0";
/// The format version this build writes, and the only one it reads.
pub(super) const VERSION: u32 = 1;
/// The longest id the header holds, in bytes.
pub(super) const MAX_ID_LEN: usize = 224;

const LEN: usize = HEADER_LEN as usize;
const ID_START: usize = 24;
const CRC_START: usize = 252;

/// The header this build writes for a file of `slot_size` byte slots held for `id`.
///
/// `id` must be 1 to [`MAX_ID_LEN`] bytes; the config checks that before any file is
/// touched.
pub(super) fn encode(slot_size: u32, id: &str) -> [u8; LEN] {
	assert!(
		(1..=MAX_ID_LEN).contains(&id.len()),
		"id must be 1 to {MAX_ID_LEN} bytes"
	);
	let mut header = [0u8; LEN];
	header[0..8].copy_from_slice(MAGIC);
	header[8..12].copy_from_slice(&VERSION.to_le_bytes());
	header[12..16].copy_from_slice(&slot_size.to_le_bytes());
	header[20..24].copy_from_slice(&(id.len() as u32).to_le_bytes());
	header[ID_START..ID_START + id.len()].copy_from_slice(id.as_bytes());
	let crc = crc32(&header[..CRC_START]);
	header[CRC_START..].copy_from_slice(&crc.to_le_bytes());
	header
}

/// Why an existing file's header is not the one this build would write, named in the one
/// line logged when the file is discarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Mismatch {
	/// The file is shorter than a header: cut short while it was being created, say.
	Short { len: u64 },
	/// The file does not start with the magic: it is not a FileStore file.
	Magic,
	/// The header fails its CRC: it is damaged.
	Crc,
	/// The header is from an older format version.
	OlderVersion { found: u32 },
	/// The header is from a newer format version.
	NewerVersion { found: u32 },
	/// The file is held for another id.
	Id,
	/// The file's slots are another size: `max_item_size` changed.
	SlotSize { found: u32, expected: u32 },
	/// Every field matches, but a reserved byte or a byte of the id's padding is not
	/// zero. This build writes none, so the file was not written by it.
	Reserved,
}

impl fmt::Display for Mismatch {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Short { len } => write!(f, "it is {len} bytes, shorter than a header"),
			Self::Magic => write!(f, "it is not a FileStore file"),
			Self::Crc => write!(f, "its header is damaged"),
			Self::OlderVersion { found } => {
				write!(f, "it is format version {found}, older than {VERSION}")
			}
			Self::NewerVersion { found } => {
				write!(f, "it is format version {found}, newer than {VERSION}")
			}
			Self::Id => write!(f, "it is held for another id"),
			Self::SlotSize { found, expected } => write!(
				f,
				"its slots hold {found} bytes, not the {expected} configured"
			),
			Self::Reserved => write!(f, "its header has unexpected reserved bytes"),
		}
	}
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
	u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// Checks a file's first bytes, `bytes` (all of them if the file is shorter than a
/// header), against the header this build would write for `slot_size` and `id`. It
/// matches only if it is exactly that header.
pub(super) fn check(bytes: &[u8], slot_size: u32, id: &str) -> Result<(), Mismatch> {
	if bytes.len() < LEN {
		return Err(Mismatch::Short {
			len: bytes.len() as u64,
		});
	}
	let bytes = &bytes[..LEN];
	if &bytes[0..8] != MAGIC {
		return Err(Mismatch::Magic);
	}
	if crc32(&bytes[..CRC_START]) != u32_at(bytes, CRC_START) {
		return Err(Mismatch::Crc);
	}
	let version = u32_at(bytes, 8);
	if version < VERSION {
		return Err(Mismatch::OlderVersion { found: version });
	}
	if version > VERSION {
		return Err(Mismatch::NewerVersion { found: version });
	}
	let id_len = u32_at(bytes, 20) as usize;
	if id_len != id.len() || &bytes[ID_START..ID_START + id_len] != id.as_bytes() {
		return Err(Mismatch::Id);
	}
	let found = u32_at(bytes, 12);
	if found != slot_size {
		return Err(Mismatch::SlotSize {
			found,
			expected: slot_size,
		});
	}
	if bytes != encode(slot_size, id) {
		return Err(Mismatch::Reserved);
	}
	Ok(())
}
