//! The crate's error type, shared by every store.

use std::fmt;
use std::io;

/// What went wrong in a store operation.
///
/// Each variant says what a caller can do about it. [`ItemTooLarge`](Self::ItemTooLarge)
/// and [`ItemTooDeep`](Self::ItemTooDeep) mean the item will never be stored: drop it or
/// report it. [`InUse`](Self::InUse) means two stores were made for one file, a mistake
/// in the caller's setup. [`Io`](Self::Io) is the only one worth retrying.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
	/// `append`: the serialized item is larger than the store's `max_item_size`.
	ItemTooLarge {
		/// The item's length serialized compactly, in bytes.
		size: usize,
		/// The store's `max_item_size`.
		max: usize,
	},
	/// `append`: the item is nested too deeply in arrays and objects to be read back.
	ItemTooDeep,
	/// `append`, `fetch`, `remove`: another store in this process is using this file, so
	/// this one did nothing.
	InUse,
	/// Anything the underlying storage reported.
	Io(io::Error),
}

/// The result of a store operation.
pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::ItemTooLarge { size, max } => write!(
				f,
				"item is {size} bytes serialized, larger than max_item_size = {max} bytes"
			),
			Self::ItemTooDeep => write!(
				f,
				"item is nested more than {} levels deep in arrays and objects",
				crate::serialize::MAX_ITEM_DEPTH
			),
			Self::InUse => write!(f, "another store in this process is using this file"),
			Self::Io(e) => write!(f, "I/O error: {e}"),
		}
	}
}

impl std::error::Error for Error {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Self::Io(e) => Some(e),
			_ => None,
		}
	}
}

impl From<io::Error> for Error {
	fn from(e: io::Error) -> Self {
		Self::Io(e)
	}
}

#[cfg(test)]
mod tests;
