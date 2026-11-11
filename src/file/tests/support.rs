//! Helpers shared by the FileStore tests.

use crate::file::config::FileConfig;
use crate::file::core::FileCore;
use crate::file::faults::{Faults, Hook, Op};
use crate::file::layout::{Layout, HEADER_LEN};
use std::path::{Component, Path, PathBuf};
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

/// Writes `payload` into slot `slot` of the file at `path`, whose slots are `slot_size`
/// bytes, under a live entry for item `sequence` whose CRC matches it: an item the store
/// will read back as written, whatever its bytes.
pub(crate) fn write_item(path: &Path, slot_size: usize, slot: u64, sequence: u64, payload: &[u8]) {
	let layout = Layout {
		slot_size: slot_size as u32,
	};
	poke(path, layout.payload_offset(slot), payload);
	poke(
		path,
		layout.entry_offset(slot),
		&crate::file::entry::encode_live(sequence, payload),
	);
}

/// The bytes of the file at `path`.
pub(crate) fn bytes_of(path: &Path) -> Vec<u8> {
	std::fs::read(path).unwrap()
}

/// The variable naming the one test a child process started by [`isolated`] runs.
const ISOLATED_TEST: &str = "TRANSIENTDB_ISOLATED_TEST";

/// Runs `body` for the test named `test` (its full path, from `module_path!()`) in a
/// child process running only that test, and fails if the child does not pass it. A
/// stack overflow aborts a whole process, so a test that would overflow on a regression
/// fails on its own instead of taking the rest of the test binary with it.
///
/// `test` is typed by hand, so it must be the calling test's own name. A name that
/// matches no test runs nothing in the child, and fails. But a test copied from another
/// that keeps the original's name runs the original in the child, which passes, so the
/// copy passes without its own body ever running.
pub(crate) fn isolated(test: &str, body: impl FnOnce()) {
	let name = test.split_once("::").map_or(test, |(_, name)| name);
	if std::env::var(ISOLATED_TEST).as_deref() == Ok(name) {
		body();
		return;
	}
	let output = std::process::Command::new(std::env::current_exe().unwrap())
		.args([name, "--exact", "--test-threads=1", "--nocapture"])
		.env(ISOLATED_TEST, name)
		.output()
		.unwrap();
	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		output.status.success() && stdout.contains("test result: ok. 1 passed"),
		"{name} failed in its own process ({}):\n{stdout}\n{stderr}",
		output.status
	);
}

/// `path`, an absolute path, spelled relative to the current directory.
pub(crate) fn relative(path: &Path) -> PathBuf {
	let cwd = std::env::current_dir().unwrap();
	let ups = cwd
		.components()
		.filter(|c| matches!(c, Component::Normal(_)))
		.count();
	let mut relative = PathBuf::new();
	for _ in 0..ups {
		relative.push("..");
	}
	relative.join(path.strip_prefix("/").unwrap())
}
