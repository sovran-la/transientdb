//! What a FileStore is configured with.

use super::header::MAX_ID_LEN;
use std::path::PathBuf;

/// Configuration for a FileStore.
///
/// The file is never larger than `256 + (32 + max_item_size) * max_items` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileConfig {
	/// The store's file. Its parent directory is created if missing.
	pub path: PathBuf,
	/// Whose data this is: a write key, for example. It is recorded in the file, and a
	/// file recorded under a different id is discarded at open. 1 to 224 bytes.
	pub id: String,
	/// Largest serialized item, in bytes. This is the slot size, and it is part of the
	/// file's layout.
	pub max_item_size: usize,
	/// The most items the store holds at once. Appending past it drops the oldest item.
	/// Not part of the file's layout: it can change between runs.
	pub max_items: usize,
}

impl FileConfig {
	/// Panics, in the manner of the other stores, if the config cannot describe a file:
	/// a limit of 0, an empty id or one longer than 224 bytes, or a `max_item_size` the
	/// header's 32 bit slot size cannot record.
	pub(super) fn validate(&self) {
		if self.max_item_size == 0 {
			panic!("max_item_size = 0? Slots that hold nothing, in a file that holds them. Tidy.");
		}
		if self.max_items == 0 {
			panic!("max_items = 0? So... you want a store that stores nothing? That's what /dev/null is for.");
		}
		if self.id.is_empty() {
			panic!("id is empty? A file held for nobody gets discarded by everybody.");
		}
		if self.id.len() > MAX_ID_LEN {
			panic!(
				"id is {} bytes? The header holds {MAX_ID_LEN}. It's an id, not a memoir.",
				self.id.len()
			);
		}
		if u32::try_from(self.max_item_size).is_err() {
			panic!(
				"max_item_size over {} bytes? The header records it in 32 bits, and that's a single item.",
				u32::MAX
			);
		}
	}

	/// The slot size recorded in the header: `max_item_size`, once validated.
	pub(super) fn slot_size(&self) -> u32 {
		self.max_item_size as u32
	}
}
