//! The store's open file: positional reads and writes, with nothing buffered.
//!
//! Every write goes straight to the OS, so when a call returns the bytes are with the
//! kernel and outlive the process. Every operation first asks the [`Hook`] whether to
//! fail, or to cut a write short; outside tests it never does.

use super::faults::{check, killed, record, spend, Hook, Op};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};

pub(super) struct SlotFile {
	file: File,
	hook: Hook,
}

impl SlotFile {
	pub(super) fn new(file: File, hook: Hook) -> Self {
		Self { file, hook }
	}

	/// The file's length in bytes.
	pub(super) fn len(&self) -> io::Result<u64> {
		check(&self.hook, Op::Length)?;
		Ok(self.file.metadata()?.len())
	}

	/// Reads exactly `buf.len()` bytes from `offset`. Fails with `UnexpectedEof` if the
	/// file ends first.
	pub(super) fn read_at(&mut self, offset: u64, buf: &mut [u8], op: Op) -> io::Result<()> {
		check(&self.hook, op)?;
		self.file.seek(SeekFrom::Start(offset))?;
		self.file.read_exact(buf)
	}

	/// Writes all of `bytes` at `offset`, extending the file if it ends before them.
	pub(super) fn write_at(&mut self, offset: u64, bytes: &[u8], op: Op) -> io::Result<()> {
		check(&self.hook, op)?;
		self.file.seek(SeekFrom::Start(offset))?;
		match spend(&self.hook, bytes.len()) {
			Ok(_) => {
				self.file.write_all(bytes)?;
				record(&self.hook, op);
				Ok(())
			}
			Err(allowed) => {
				self.file.write_all(&bytes[..allowed])?;
				Err(killed())
			}
		}
	}

	/// Cuts the file to `len` bytes.
	pub(super) fn set_len(&mut self, len: u64) -> io::Result<()> {
		check(&self.hook, Op::Cut)?;
		self.file.set_len(len)?;
		record(&self.hook, Op::Cut);
		Ok(())
	}
}
