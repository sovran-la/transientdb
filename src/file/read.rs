//! Reading items back at fetch, checking each one as it is read.

use super::core::FileCore;
use super::entry::{self, Entry, LIVE};
use super::faults::Op;
use super::index::Live;
use std::io;

impl FileCore {
	/// Reads the oldest live items not `skipped`, in sequence order, as `(sequence, text)`:
	/// at most `count`, whose lengths total at most `max_bytes`. Stops at the first item
	/// that would pass `max_bytes`, so items are never returned out of order.
	///
	/// Each item is checked as it is read: its CRC, that it is UTF-8 and well-formed
	/// JSON. One that fails is freed, logged and skipped. One whose slot no longer holds
	/// it (only another process using the file can cause that) is forgotten, logged and
	/// skipped, and its slot left as it is.
	///
	/// On an I/O error nothing is returned and the index is as it was, apart from any
	/// damaged item already freed.
	pub(super) fn read_items(
		&mut self,
		count: usize,
		max_bytes: usize,
		mut skipped: impl FnMut(u64) -> bool,
	) -> io::Result<Vec<(u64, String)>> {
		let mut items = Vec::new();
		let mut bytes = 0usize;
		let mut cursor = None;
		while items.len() < count {
			let Some((sequence, live)) = self.index.live_after(cursor) else {
				break;
			};
			cursor = Some(sequence);
			if skipped(sequence) {
				continue;
			}
			let len = live.len as usize;
			if len > max_bytes - bytes {
				break;
			}
			if let Some(text) = self.read_item(sequence, live)? {
				bytes += len;
				items.push((sequence, text));
			}
		}
		Ok(items)
	}

	/// Whether any live item is not `skipped`.
	pub(super) fn has_unskipped(&self, mut skipped: impl FnMut(u64) -> bool) -> bool {
		self.index.live().any(|(sequence, _)| !skipped(sequence))
	}

	/// Reads item `sequence` from its slot and checks it. `None` if it failed a check and
	/// was dropped.
	fn read_item(&mut self, sequence: u64, live: Live) -> io::Result<Option<String>> {
		let mut bytes = vec![0u8; entry::LEN + live.len as usize];
		let offset = self.layout.entry_offset(live.slot);
		match self.file.read_at(offset, &mut bytes, Op::ReadItem) {
			Ok(()) => {}
			Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
				self.forget(sequence, "the file ends inside its slot");
				return Ok(None);
			}
			Err(e) => return Err(e),
		}
		let payload = bytes.split_off(entry::LEN);
		let entry = Entry::decode(bytes[..].try_into().unwrap());
		if entry.state != LIVE || entry.sequence != sequence || entry.length != live.len {
			self.forget(sequence, "its slot holds another entry");
			return Ok(None);
		}
		if !entry.matches(&payload) {
			self.discard(sequence, "it fails its CRC");
			return Ok(None);
		}
		let Ok(text) = String::from_utf8(payload) else {
			self.discard(sequence, "it is not UTF-8");
			return Ok(None);
		};
		if serde_json::from_str::<serde::de::IgnoredAny>(&text).is_err() {
			self.discard(sequence, "it is not well-formed JSON");
			return Ok(None);
		}
		Ok(Some(text))
	}

	/// Forgets item `sequence` without writing anything: its slot holds something else,
	/// which is not this store's to free.
	fn forget(&mut self, sequence: u64, why: &str) {
		if let Some(slot) = self.index.remove(sequence) {
			self.log(format_args!(
				"lost item {sequence} in slot {slot}: {why}; is another process using this file?"
			));
			self.shrink_if_empty();
		}
	}
}
