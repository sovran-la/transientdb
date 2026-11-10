//! The open data file, and rolling it back after a failed write.
//!
//! A thin wrapper around [`File`] that can roll the file back to a known length. Under
//! `cfg(test)` it also carries [`Faults`], shared with the store, so tests can make a
//! write fail after a given number of bytes, a flush fail, or a rollback fail. Recovery
//! reads the same `Faults` to make deleting a leftover fail.

use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};

#[cfg(test)]
use std::sync::{Arc, Mutex};

/// Failures to inject into every `DataFile` a store opens, for tests.
#[cfg(test)]
#[derive(Debug, Default)]
pub(super) struct Faults {
	/// Bytes that may still reach the file before writes fail; `None` never fails.
	pub(super) write_budget: Option<usize>,
	/// Whether flushing the file fails.
	pub(super) fail_flush: bool,
	/// Whether rolling the file back fails.
	pub(super) fail_truncate: bool,
	/// Whether recovery fails to delete a leftover it found nothing deliverable in.
	pub(super) fail_remove: bool,
}

#[cfg(test)]
fn injected(what: &str) -> io::Error {
	io::Error::other(format!("injected {what} failure"))
}

pub(super) struct DataFile {
	file: File,
	#[cfg(test)]
	faults: Arc<Mutex<Faults>>,
}

impl DataFile {
	pub(super) fn new(file: File, #[cfg(test)] faults: Arc<Mutex<Faults>>) -> Self {
		Self {
			file,
			#[cfg(test)]
			faults,
		}
	}

	/// Cuts the file back to `len` bytes and positions it there, so the next write
	/// continues from `len`. Fails, changing nothing, if the file is shorter than `len`:
	/// growing it would fill the gap with zero bytes.
	pub(super) fn truncate_to(&mut self, len: u64) -> io::Result<()> {
		#[cfg(test)]
		if self.faults.lock().unwrap().fail_truncate {
			return Err(injected("truncate"));
		}
		let actual = self.file.metadata()?.len();
		if actual < len {
			return Err(io::Error::other(format!(
				"file is {actual} bytes, shorter than the {len} it should keep"
			)));
		}
		self.file.set_len(len)?;
		self.file.seek(SeekFrom::Start(len))?;
		Ok(())
	}
}

impl Write for DataFile {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		#[cfg(test)]
		{
			let mut faults = self.faults.lock().unwrap();
			if let Some(budget) = faults.write_budget {
				if budget == 0 {
					return Err(injected("write"));
				}
				let written = self.file.write(&buf[..budget.min(buf.len())])?;
				faults.write_budget = Some(budget - written);
				return Ok(written);
			}
		}
		self.file.write(buf)
	}

	fn flush(&mut self) -> io::Result<()> {
		#[cfg(test)]
		if self.faults.lock().unwrap().fail_flush {
			return Err(injected("flush"));
		}
		self.file.flush()
	}
}

impl super::DirectoryStore {
	/// Writes `bytes` to the open file and flushes them.
	///
	/// If the write or the flush fails, the file is rolled back to `current_size`, the
	/// opening plus the complete items and separators before this write, and the error
	/// is returned; the accounting is left as it was. Bytes still buffered are dropped
	/// without being flushed. If the rollback fails too, the file is abandoned: the store
	/// forgets it, so the next append starts a fresh file, and lists it for the next
	/// fetch to salvage. The original error is returned either way.
	pub(super) fn write_to_open_file(&mut self, bytes: &[u8]) -> io::Result<()> {
		let writer = self
			.writer
			.as_mut()
			.ok_or_else(|| io::Error::other("No active writer"))?;
		let Err(e) = writer.write_all(bytes).and_then(|()| writer.flush()) else {
			return Ok(());
		};
		let Some(writer) = self.writer.take() else {
			return Err(e);
		};
		let (mut file, _unflushed) = writer.into_parts();
		match file.truncate_to(self.current_size as u64) {
			Ok(()) => self.writer = Some(BufWriter::new(file)),
			Err(rollback) => {
				let path = self.current_path.take();
				eprintln!(
					"Failed to roll back {:?} after a failed write ({}), abandoning it: {}",
					path, e, rollback
				);
				self.current_size = 0;
				self.current_items = 0;
				if let Some(path) = path {
					self.list_leftover(path);
				}
			}
		}
		Err(e)
	}
}
