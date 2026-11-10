//! The process-wide set of store files in use, so two stores never share one.
//!
//! A path is recorded in its canonical form, so different spellings of one file (relative
//! and absolute, through `..` or a symbolic link) are the same entry. No OS lock is taken
//! and nothing is written to the file.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

static IN_USE: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// The registry. A panic while it was held cannot have left it half changed: every
/// change is one insert or remove.
fn in_use() -> MutexGuard<'static, BTreeSet<PathBuf>> {
	IN_USE
		.lock()
		.unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The canonical form of a store's `path`, creating its parent directory if it is
/// missing.
///
/// The file itself need not exist. If it does, the whole path is canonicalized. If not,
/// the parent is, and the file name appended, so the result is the path the file will
/// have once created.
pub(super) fn canonical(path: &Path) -> io::Result<PathBuf> {
	let name = path.file_name().ok_or_else(|| {
		io::Error::new(
			io::ErrorKind::InvalidInput,
			format!("{} does not name a file", path.display()),
		)
	})?;
	match path.canonicalize() {
		Ok(canonical) => return Ok(canonical),
		Err(e) if e.kind() == io::ErrorKind::NotFound => {}
		Err(e) => return Err(e),
	}
	let parent = match path.parent() {
		Some(parent) if !parent.as_os_str().is_empty() => parent,
		_ => Path::new("."),
	};
	std::fs::create_dir_all(parent)?;
	Ok(parent.canonicalize()?.join(name))
}

/// A store's claim on its file, held until it is dropped.
#[derive(Debug)]
pub(super) struct Lease {
	path: PathBuf,
}

impl Lease {
	/// Claims the file at the canonical path `path` for one store, or returns `None` if
	/// another store in this process holds it.
	pub(super) fn acquire(path: PathBuf) -> Option<Lease> {
		// The lease is made only once the path is taken: dropping one releases its path,
		// which would deadlock on the guard here and free another store's claim.
		let taken = in_use().insert(path.clone());
		if taken {
			Some(Lease { path })
		} else {
			None
		}
	}

	pub(super) fn path(&self) -> &Path {
		&self.path
	}
}

impl Drop for Lease {
	fn drop(&mut self) {
		in_use().remove(&self.path);
	}
}
