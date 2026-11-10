//! Recovery of the files an earlier session left unfinished.
//!
//! `new` only lists this store's unfinished files; nothing on disk changes until the
//! first `fetch`, which recovers each listed file once, after any validator has been
//! set. A file the validator rejects is deleted; one that fails to recover for any
//! other reason is logged and left for a later session.

use super::DirectoryStore;
use serde_json::Value;
use std::fs;
use std::io::Result;
use std::path::Path;

impl DirectoryStore {
	/// Recovers every file `new` listed as left over, lowest index first, then forgets
	/// the list. A failure recovering one file is logged and does not stop the others;
	/// a file the validator rejects is deleted, which is not a failure.
	pub(super) fn recover_leftovers(&mut self) {
		for path in std::mem::take(&mut self.leftovers) {
			if let Err(e) = self.recover_file(&path) {
				eprintln!("Failed to recover file {:?}: {}", path, e);
			}
		}
	}

	/// Finalizes a file an earlier session left without the `.temp` extension, running
	/// the validator on it as on any other finished file.
	///
	/// A file that is already a complete envelope, because its validator rejected it
	/// after the closing was written or the session ended before the rename, is not
	/// closed a second time. Any other file is closed as usual.
	pub(super) fn recover_file(&self, path: &Path) -> Result<()> {
		if Self::is_complete_envelope(&fs::read(path)?) {
			self.mark_finished(path)
		} else {
			self.finalize_file(path)
		}
	}

	/// Whether `content` already parses as a finished data file.
	fn is_complete_envelope(content: &[u8]) -> bool {
		serde_json::from_slice::<Value>(content).is_ok_and(|json| {
			json["batch"].is_array() && json["sentAt"].is_string() && json["writeKey"].is_string()
		})
	}
}
