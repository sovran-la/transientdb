//! A store's open file and what it knows about it: creating and opening the file.
//!
//! [`FileCore`] is everything a FileStore does to its file. It does not know about the
//! registry, pending items or the public API; the store wraps it with those.

use super::config::FileConfig;
use super::faults::{check, record, Hook, Op};
use super::handle::SlotFile;
use super::header::{self, Mismatch};
use super::index::SlotIndex;
use super::layout::{Layout, HEADER_LEN};
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

pub(super) struct FileCore {
	pub(super) file: SlotFile,
	/// The file's path, for logging.
	pub(super) path: PathBuf,
	pub(super) layout: Layout,
	pub(super) max_items: usize,
	pub(super) index: SlotIndex,
}

impl FileCore {
	/// Opens the store file at `path` for `config`, creating it if it does not exist and
	/// discarding it, with one log line, if it is not a file this build would write for
	/// `config`. Reads every slot's entry, but no payload.
	///
	/// An entry that is neither free nor validly live is freed. If more items are live
	/// than `max_items` allows, the oldest are dropped. If none is live, the file is cut
	/// back to its header. Those writes are logged if they fail, not returned: the slots
	/// they concern are free either way, and the next open sees the same.
	///
	/// Returns an error, having changed nothing that matters, if the file cannot be read,
	/// created or deleted.
	pub(super) fn open(path: &Path, config: &FileConfig, hook: Hook) -> io::Result<Self> {
		config.validate();
		let layout = Layout {
			slot_size: config.slot_size(),
		};
		check(&hook, Op::Open)?;
		match OpenOptions::new().read(true).write(true).open(path) {
			Ok(file) => {
				let mut file = SlotFile::new(file, hook.clone());
				let len = file.len()?;
				match read_header(&mut file, len, config)? {
					Ok(()) => return Self::scan(file, path, layout, len, config.max_items),
					Err(mismatch) => {
						warn(path, format_args!("discarding the file: {mismatch}"));
						drop(file);
						delete(path, &hook)?;
					}
				}
			}
			Err(e) if e.kind() == io::ErrorKind::NotFound => {}
			Err(e) => return Err(e),
		}
		Self::create(path, layout, config, hook)
	}

	/// Creates the file with its header and nothing else: no slots, nothing synced or
	/// renamed. A kill before the header is whole leaves a file that does not match, so
	/// the next open creates it again.
	fn create(path: &Path, layout: Layout, config: &FileConfig, hook: Hook) -> io::Result<Self> {
		check(&hook, Op::Create)?;
		let file = OpenOptions::new()
			.read(true)
			.write(true)
			.create_new(true)
			.open(path)?;
		record(&hook, Op::Create);
		let mut file = SlotFile::new(file, hook);
		file.write_at(0, &header::encode(layout.slot_size, &config.id), Op::Header)?;
		Ok(Self {
			file,
			path: path.to_path_buf(),
			layout,
			max_items: config.max_items,
			index: SlotIndex::new(0, 1),
		})
	}

	/// Logs a warning about this store's file: something the store handled itself.
	pub(super) fn warn(&self, message: std::fmt::Arguments) {
		warn(&self.path, message);
	}

	/// Logs an error about this store's file: a read or write the OS refused.
	pub(super) fn error(&self, message: std::fmt::Arguments) {
		error(&self.path, message);
	}
}

/// Logs a warning about the store file at `path`, through the `log` facade.
pub(super) fn warn(path: &Path, message: std::fmt::Arguments) {
	log::warn!(target: crate::LOG_TARGET, "FileStore {}: {message}", path.display());
}

/// Logs an error about the store file at `path`, through the `log` facade.
pub(super) fn error(path: &Path, message: std::fmt::Arguments) {
	log::error!(target: crate::LOG_TARGET, "FileStore {}: {message}", path.display());
}

/// Checks the header of a file of `len` bytes against `config`'s.
fn read_header(
	file: &mut SlotFile,
	len: u64,
	config: &FileConfig,
) -> io::Result<Result<(), Mismatch>> {
	let mut bytes = vec![0u8; len.min(HEADER_LEN) as usize];
	if !bytes.is_empty() {
		file.read_at(0, &mut bytes, Op::ReadHeader)?;
	}
	Ok(header::check(&bytes, config.slot_size(), &config.id))
}

/// Deletes a file that does not match. One already gone is as good as deleted.
fn delete(path: &Path, hook: &Hook) -> io::Result<()> {
	check(hook, Op::Delete)?;
	match fs::remove_file(path) {
		Ok(()) => {}
		Err(e) if e.kind() == io::ErrorKind::NotFound => {}
		Err(e) => return Err(e),
	}
	record(hook, Op::Delete);
	Ok(())
}
