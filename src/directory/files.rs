//! Which files in the storage directory belong to a store.
//!
//! A store owns exactly the files it names: `{index}-{base_filename}` while open or
//! unfinished, and that whole name with `.temp` appended once finished
//! (`5-events.json` becomes `5-events.json.temp`). `index` is a `u32` in canonical
//! decimal, as the store writes it. Names are compared as whole strings, and only
//! regular files count. Every listing of the directory goes through
//! [`DirectoryStore::owned_files`], so a store never reads, closes, renames, counts or
//! deletes anything else in the directory, including another store's files.
//!
//! Indexes only grow. A new file takes the next index above every one in use, and
//! once `u32::MAX` is taken there is no next index: starting a file fails rather than
//! wrapping around to an index that would be fetched out of order.

use super::DirectoryStore;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Result};
use std::path::{Path, PathBuf};

/// One of a store's files on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OwnedFile {
	pub(super) path: PathBuf,
	pub(super) index: u32,
	/// Whether the file has been renamed with `.temp`, ready to be fetched.
	pub(super) finished: bool,
}

impl DirectoryStore {
	/// Takes the next index for a new file, or fails if `u32::MAX` is already taken.
	pub(super) fn take_index(&mut self) -> Result<u32> {
		let index = self.next_index.ok_or_else(|| {
			io::Error::other(format!(
				"No file index left: index {} is taken and indexes never wrap. Fetch and remove this store's files, then open it again.",
				u32::MAX
			))
		})?;
		self.next_index = index.checked_add(1);
		Ok(index)
	}

	/// Panics unless `base_filename` names files inside the storage directory that the
	/// ownership rule can tell apart: not empty, not `.` or `..`, no `/`, `\` or NUL, and
	/// not ending in `.temp`. Both separators are refused on every platform, so the same
	/// configuration behaves the same everywhere.
	pub(super) fn validate_base_filename(base_filename: &str) {
		if base_filename.is_empty() {
			panic!("base_filename is empty? Every file would be named \"{{index}}-\", which is a number and a dash, not a name.");
		}
		if base_filename.contains(['/', '\\']) {
			panic!("base_filename contains a path separator? Files go in storage_location, not wherever the name wanders off to.");
		}
		if base_filename == "." || base_filename == ".." {
			panic!("base_filename is \".\" or \"..\"? Those name directories, not files.");
		}
		if base_filename.contains('\0') {
			panic!("base_filename contains a NUL byte? The filesystem stops reading there, and so would ownership.");
		}
		if base_filename.ends_with(&format!(".{}", Self::TEMP_EXTENSION)) {
			panic!("base_filename ends in \".temp\"? That's the mark of a finished file; a name that looks done before it starts can't be told apart from one that is.");
		}
	}

	/// The name of this store's file `index` while it is open or unfinished.
	pub(super) fn unfinished_name(&self, index: u32) -> String {
		format!("{index}-{}", self.config.base_filename)
	}

	/// The path an unfinished file is renamed to once finished: `.temp` appended to its
	/// whole name, never replacing an extension the base filename already has.
	pub(super) fn finished_path(path: &Path) -> PathBuf {
		let mut name = OsString::from(path);
		name.push(".");
		name.push(Self::TEMP_EXTENSION);
		PathBuf::from(name)
	}

	/// The ownership rule: `Some((index, finished))` if `file_name` is exactly one of
	/// this store's names, `{index}-{base_filename}` (unfinished) or
	/// `{index}-{base_filename}.temp` (finished), and `None` for anything else.
	///
	/// The index must be written exactly as the store writes it, so `007-events` and
	/// `+7-events` are not ours. `new` rejects a base filename ending in `.temp`: base
	/// `x.temp`'s unfinished files would have base `x`'s finished names, so two stores
	/// could not tell their files apart. Every other pair of distinct bases has disjoint
	/// names.
	pub(super) fn owned_name(&self, file_name: &str) -> Option<(u32, bool)> {
		let (prefix, rest) = file_name.split_once('-')?;
		let index: u32 = prefix.parse().ok()?;
		if prefix != index.to_string() {
			return None;
		}
		let base = self.config.base_filename.as_str();
		if rest == base {
			return Some((index, false));
		}
		let extension = rest.strip_prefix(base)?.strip_prefix('.')?;
		(extension == Self::TEMP_EXTENSION).then_some((index, true))
	}

	/// Every file of this store in the storage directory, lowest index first.
	///
	/// Entries that vanish or cannot be inspected while the directory is listed are
	/// skipped, as are directories and other non-regular files, whatever their name.
	pub(super) fn owned_files(&self) -> Result<Vec<OwnedFile>> {
		let mut files = Vec::new();
		for entry in fs::read_dir(&self.config.storage_location)?.flatten() {
			let name = entry.file_name();
			let Some((index, finished)) = name.to_str().and_then(|n| self.owned_name(n)) else {
				continue;
			};
			if entry.file_type().is_ok_and(|t| t.is_file()) {
				files.push(OwnedFile {
					path: entry.path(),
					index,
					finished,
				});
			}
		}
		files.sort_by_key(|f| (f.index, f.finished));
		Ok(files)
	}
}
