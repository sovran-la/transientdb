//! Helpers shared by the FileStore tests.

use crate::file::config::FileConfig;
use crate::file::core::FileCore;
use crate::file::faults::{Faults, Hook, Op};
use crate::file::layout::{Layout, HEADER_LEN};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The id the tests' stores use.
pub(crate) const ID: &str = "write-key";
/// The slot size most tests use: small, so a test can step through every byte.
pub(crate) const SLOT: usize = 16;

pub(crate) fn config(path: &Path, max_item_size: usize, max_items: usize) -> FileConfig {
	FileConfig {
		path: path.to_path_buf(),
		id: ID.to_string(),
		max_item_size,
		max_items,
	}
}

/// A temporary directory and the path of a store file in it.
pub(crate) fn temp_path() -> (TempDir, PathBuf) {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("store.tdb");
	(dir, path)
}

/// Opens the store file at `path` with `SLOT` byte slots, returning the core and the
/// faults it reads.
pub(crate) fn open_with(path: &Path, max_items: usize) -> (FileCore, Hook) {
	open_config(&config(path, SLOT, max_items))
}

pub(crate) fn open_config(config: &FileConfig) -> (FileCore, Hook) {
	let hook = Faults::shared();
	let core = FileCore::open(&config.path, config, hook.clone()).unwrap();
	(core, hook)
}

/// Opens the store file at `path` with `SLOT` byte slots and room for 100 items.
pub(crate) fn open(path: &Path) -> FileCore {
	open_with(path, 100).0
}

pub(crate) fn layout() -> Layout {
	Layout {
		slot_size: SLOT as u32,
	}
}

/// The length of a file of `slots` whole `SLOT` byte slots.
pub(crate) fn len_of(slots: u64) -> u64 {
	HEADER_LEN + layout().slot_len() * slots
}

pub(crate) fn file_len(path: &Path) -> u64 {
	std::fs::metadata(path).unwrap().len()
}

/// A JSON string item, unique by `tag`, whose text is exactly `len` bytes.
pub(crate) fn item(len: usize, tag: usize) -> String {
	let width = len - 2;
	let text = format!("\"{tag:0>width$}\"");
	assert_eq!(text.len(), len, "tag {tag} too wide for {len} bytes");
	text
}

/// Every live item's text, oldest first, read without limits.
pub(crate) fn texts(core: &mut FileCore) -> Vec<String> {
	all(core).into_iter().map(|(_, text)| text).collect()
}

/// Every live item, oldest first, read without limits.
pub(crate) fn all(core: &mut FileCore) -> Vec<(u64, String)> {
	core.read_items(usize::MAX, usize::MAX, |_| false).unwrap()
}

/// The slot each live item is in, oldest item first.
pub(crate) fn slots_of(core: &FileCore) -> Vec<u64> {
	core.index.live().map(|(_, live)| live.slot).collect()
}

/// The ops that reached the file since the hook was made or last drained.
pub(crate) fn drain(hook: &Hook) -> Vec<Op> {
	std::mem::take(&mut hook.lock().unwrap().done)
}

/// Every permutation of `0..n`.
pub(crate) fn permutations(n: usize) -> Vec<Vec<usize>> {
	if n == 0 {
		return vec![Vec::new()];
	}
	let mut out = Vec::new();
	for rest in permutations(n - 1) {
		for at in 0..=rest.len() {
			let mut p = rest.clone();
			p.insert(at, n - 1);
			out.push(p);
		}
	}
	out
}

/// Writes `bytes` at `offset` into the file at `path`, as damage from outside.
pub(crate) fn poke(path: &Path, offset: u64, bytes: &[u8]) {
	use std::io::{Seek, SeekFrom, Write};
	let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
	file.seek(SeekFrom::Start(offset)).unwrap();
	file.write_all(bytes).unwrap();
}

/// The bytes of the file at `path`.
pub(crate) fn bytes_of(path: &Path) -> Vec<u8> {
	std::fs::read(path).unwrap()
}
