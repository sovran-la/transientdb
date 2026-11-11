//! What a FileStore is configured with.

use super::header::MAX_ID_LEN;
use std::path::PathBuf;

/// Configuration for a [`FileStore`](crate::FileStore).
///
/// The file is a 256 byte header and one `32 + max_item_size` byte slot per item at the
/// queue's deepest since the file was last cut back to its header, which removing the
/// last item does. So while `max_items` stays the same the file is never larger than
/// `256 + (32 + max_item_size) * max_items` bytes: with `max_item_size = 8192` and
/// `max_items = 2560` it may reach 21,053,696 bytes, but a queue never more than 50
/// items deep keeps it at 411,456. Lowering `max_items` drops items but does not shrink
/// the file, so until it is next emptied the bound is that of the largest `max_items` it
/// has been opened with since it was last emptied.
///
/// Put the file somewhere the OS keeps while the app is suspended: not in a temporary
/// or cache directory, preferably in the app's own container, and with a
/// data-protection class that allows access whenever the app can run. See
/// [`FileStore`](crate::FileStore#where-to-put-the-file).
///
/// # Examples
/// ```
/// use transientdb::{FileConfig, FileStore};
///
/// let dir = tempfile::tempdir().unwrap();
/// let store = FileStore::new(FileConfig {
///     path: dir.path().join("queue").join("events.tdb"), // `queue/` is created
///     id: "write-key".to_string(), // a file held for another id is discarded
///     max_item_size: 8 * 1024,     // no item over 8 KiB serialized
///     max_items: 1000,             // the 1001st append drops the oldest item
/// });
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileConfig {
	/// The store's file. Its parent directory is created if missing. Two stores in one
	/// process must not use one file, however its path is spelled: the second does
	/// nothing until the first is dropped.
	pub path: PathBuf,
	/// Whose data this is: a write key, for example. It is recorded in the file, and a
	/// file recorded under a different id is discarded at open. 1 to 224 bytes.
	pub id: String,
	/// The largest item the store accepts: its compact `serde_json` text, in bytes.
	/// `append` rejects a larger item with [`Error::ItemTooLarge`](crate::Error). This is
	/// the size of every slot in the file, so changing it discards the file. Must be 1 to
	/// `u32::MAX`.
	pub max_item_size: usize,
	/// The most items the store holds at once. Appending past it drops the oldest item,
	/// fetched or not. It can change between runs: lowered, the oldest items beyond it
	/// are dropped when the file is opened, and the file keeps its length until it is
	/// next emptied. Must not be 0.
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
