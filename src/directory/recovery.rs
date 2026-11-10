//! Recovery of the files an earlier session left unfinished, and of those this session
//! could not finish.
//!
//! `new` only lists this store's unfinished files; nothing on disk changes until the
//! first `fetch`, which recovers each listed file once, after any validator has been
//! set. A file this session abandons after a failed rollback, or fails to finish, is
//! added to the same list, so the next `fetch` recovers it. Every fetch recovers what
//! is listed and clears the list. A file the validator rejects is deleted; one that
//! fails to recover for any other reason is logged and left for a later session.
//!
//! A file that is not already a complete envelope is salvaged: it is cut back to the
//! end of its last complete item, then closed as usual. Nothing is fsynced, so a crash
//! or power loss can leave a file ending partway through an item, or followed by zero
//! bytes or garbage.
//!
//! A file that is a complete envelope is renamed as is, unless it is larger than
//! `max_file_size` (it was written under an earlier, larger limit). Then it is cut down
//! like a salvaged file: its leading complete items that fit once closed are kept, the
//! rest are dropped, and it is closed again. If not even its first item fits, it is
//! deleted. Only unfinished files are recovered; files already finished (`.temp`) by an
//! earlier session are fetched as they are.
//!
//! Both follow the configured envelope. A complete envelope is a JSON object whose
//! configured items key holds a non-empty array, whatever its other keys; it is renamed
//! byte for byte. Salvage looks for the configured opening and closes with the
//! configured closing. A file written under another items key does not start with the
//! opening, so it is deleted, as below. A changed envelope gets no other handling.
//!
//! A listed file that does not start with the store's opening (empty, cut inside the
//! opening, or overwritten) is deleted. Its name makes it this store's file (see
//! `files.rs`), the store never writes one that way, and it can never be read, so
//! keeping it would only leave it undeliverable forever.

use super::DirectoryStore;
use serde::de::IgnoredAny;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::{Result, Write};
use std::path::{Path, PathBuf};

/// What can be kept of an unfinished file that is not a complete envelope.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Salvage {
	/// It does not start with the store's opening, so nothing in it can be read.
	Unreadable,
	/// It starts with the opening but holds no complete item.
	Empty,
	/// Its first `len` bytes are the opening and every complete item that fits.
	Keep(usize),
}

impl DirectoryStore {
	/// Lists `path` for the next `fetch` to recover, unless it is listed already.
	pub(super) fn list_leftover(&mut self, path: PathBuf) {
		if !self.leftovers.contains(&path) {
			self.leftovers.push(path);
		}
	}

	/// Recovers every listed file, in the order listed, and clears the list, so each
	/// listing is recovered once. A failure recovering one file is logged and does not
	/// stop the others, and the file is not listed again: a later session finds it. A
	/// file the validator rejects is deleted, which is not a failure.
	pub(super) fn recover_leftovers(&mut self) {
		for path in std::mem::take(&mut self.leftovers) {
			if let Err(e) = self.recover_file(&path) {
				eprintln!("Failed to recover file {:?}: {}", path, e);
			}
		}
	}

	/// Finalizes a listed file, one an earlier session left without the `.temp`
	/// extension or one this session could not finish, running the validator on it as
	/// on any other finished file.
	///
	/// A file that is already a complete envelope, because the session ended after
	/// closing it and before renaming it, is not closed a second time: at or under
	/// `max_file_size` it is renamed as is. One over it, written under an earlier, larger
	/// `max_file_size`, is cut down by the same rule as any other file. Any other file is
	/// salvaged (see [`salvage`]): cut back to its last complete item that fits under
	/// `max_file_size` once closed, and closed. For a complete envelope, the closing
	/// already on it is just what follows its last complete item, so it is cut too.
	///
	/// One that does not start with the store's opening, or that has no complete item
	/// that fits, is deleted without reaching the validator. A failed delete is returned
	/// as an error, so it is logged and the file is left, neither renamed nor fetched.
	pub(super) fn recover_file(&self, path: &Path) -> Result<()> {
		// Reads the whole file, which is bounded by the max_file_size of the session
		// that wrote it: the store never writes a file larger than its own limit.
		let content = fs::read(path)?;
		let complete = self.is_complete_envelope(&content);
		if complete && content.len() <= self.config.max_file_size {
			return self.mark_finished(path);
		}
		let max_len = self.config.max_file_size - self.closing_len;
		match salvage(self.opening(), &content, max_len) {
			Salvage::Unreadable => {
				eprintln!(
					"Deleting {:?}: it does not start like a data file, so it cannot be read",
					path
				);
				self.remove_leftover(path)
			}
			Salvage::Empty if complete => {
				eprintln!(
					"Deleting {:?}: it is {} bytes, over max_file_size = {} bytes, and not even its first item fits",
					path,
					content.len(),
					self.config.max_file_size
				);
				self.remove_leftover(path)
			}
			Salvage::Empty => {
				eprintln!("Deleting {:?}: it holds no complete item", path);
				self.remove_leftover(path)
			}
			Salvage::Keep(len) => {
				// A complete envelope always loses at least its closing here.
				if len < content.len() {
					if complete {
						eprintln!(
							"Cutting {:?} down to max_file_size = {} bytes: keeping its first {} of {} bytes, the leading items that fit",
							path,
							self.config.max_file_size,
							len,
							content.len()
						);
					} else {
						eprintln!(
							"Salvaging {:?}: keeping its first {} of {} bytes, up to its last complete item",
							path,
							len,
							content.len()
						);
					}
					OpenOptions::new()
						.write(true)
						.open(path)?
						.set_len(len as u64)?;
				}
				self.finalize_file(path)
			}
		}
	}

	/// Appends a closing to a salvaged file and marks it finished.
	fn finalize_file(&self, path: &Path) -> Result<()> {
		{
			let mut file = OpenOptions::new().append(true).open(path)?;
			let closing = self.closing();
			debug_assert_eq!(closing.len(), self.closing_len);
			file.write_all(&closing)?;
			file.flush()?;
		}
		self.mark_finished(path)
	}

	/// Deletes a leftover that holds nothing deliverable.
	fn remove_leftover(&self, path: &Path) -> Result<()> {
		#[cfg(test)]
		if self.faults.lock().unwrap().fail_remove {
			return Err(std::io::Error::other("injected remove failure"));
		}
		fs::remove_file(path)
	}

	/// Whether `content` already parses as a finished data file with at least one item:
	/// a JSON object whose configured items key holds a non-empty array. Its other keys
	/// are not examined. An unfinished file never parses, so this tells the two apart. An
	/// envelope with an empty items array is not delivered; salvage deletes it.
	fn is_complete_envelope(&self, content: &[u8]) -> bool {
		serde_json::from_slice::<Value>(content).is_ok_and(|json| {
			json[&self.config.envelope.items_key]
				.as_array()
				.is_some_and(|items| !items.is_empty())
		})
	}
}

/// How much of an unclosed data file to keep: `opening`, then a complete JSON value,
/// then optionally a comma and another complete value, and so on, through the end of
/// the last complete item whose end is at most `max_len` bytes into the file. Whatever
/// follows it (a trailing comma, a partial item, zero bytes, garbage, a partial
/// closing) is cut. The kept bytes are the original bytes; nothing is re-serialized.
///
/// An item that ends exactly at the end of the file is complete unless it is a number:
/// `12` there may be what is left of `1234`, so a top-level number is kept only when
/// some byte (a comma, normally) follows it. Strings, arrays, objects, `true`, `false`
/// and `null` show their own end.
pub(super) fn salvage(opening: &[u8], content: &[u8], max_len: usize) -> Salvage {
	if !content.starts_with(opening) {
		return Salvage::Unreadable;
	}
	let mut kept = None;
	let mut pos = opening.len();
	loop {
		if kept.is_some() {
			// Every item after the first follows a comma.
			let rest = &content[pos..];
			let blank = rest.iter().take_while(|b| is_json_whitespace(**b)).count();
			if rest.get(blank) != Some(&b',') {
				break;
			}
			pos += blank + 1;
		}
		let mut items =
			serde_json::Deserializer::from_slice(&content[pos..]).into_iter::<IgnoredAny>();
		if !matches!(items.next(), Some(Ok(_))) {
			break;
		}
		let end = pos + items.byte_offset();
		let maybe_truncated_number = end == content.len() && content[end - 1].is_ascii_digit();
		// A complete value can still hold invalid UTF-8 in a string, which serde_json
		// skips over unchecked here; such bytes would make the file invalid JSON.
		if maybe_truncated_number
			|| end > max_len
			|| std::str::from_utf8(&content[pos..end]).is_err()
		{
			break;
		}
		kept = Some(end);
		pos = end;
	}
	kept.map_or(Salvage::Empty, Salvage::Keep)
}

fn is_json_whitespace(byte: u8) -> bool {
	matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}
