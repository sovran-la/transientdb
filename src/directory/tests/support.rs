//! Helpers shared by the byte-ceiling tests.

use super::*;
use crate::envelope::samples::items_of;
use std::path::PathBuf;

/// The value of the test envelope's `source` field.
pub(super) const KEY: &str = "test-key";
/// `{"items":[`, the opening of [`with_source`]`(KEY)`.
pub(super) const OPENING: usize = 10;
/// `],"source":"test-key","sentAt":"<24 byte timestamp>"}`, the closing of
/// [`with_source`]`(KEY)`. `test_envelope_lengths_match_constants` checks both against the
/// store.
pub(super) const CLOSING: usize = 58;
pub(super) const ENVELOPE: usize = OPENING + CLOSING;
/// The item that, after a 60 byte one and its comma, fills a 200 byte file exactly.
pub(super) const FILLER: usize = 200 - ENVELOPE - 60 - 1;

/// A test envelope with a field and a timestamp:
/// `{"items":[...],"source":<source>,"sentAt":"..."}`.
pub(super) fn with_source(source: &str) -> Envelope {
	Envelope::new("items")
		.field("source", source)
		.timestamp("sentAt")
}

pub(super) fn config(dir: &TempDir, max_file_size: usize, envelope: Envelope) -> DirectoryConfig {
	DirectoryConfig {
		envelope,
		storage_location: dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size,
		max_file_items: None,
	}
}

pub(super) fn store(dir: &TempDir, max_file_size: usize) -> Result<DirectoryStore> {
	DirectoryStore::new(config(dir, max_file_size, with_source(KEY)))
}

/// A JSON string item whose compact serialization is exactly `len` bytes (`len >= 2`).
pub(super) fn item_of_len(len: usize) -> Value {
	assert!(len >= 2, "an item of {len} bytes cannot be a JSON string");
	Value::String("x".repeat(len - 2))
}

pub(super) fn item_len(item: &Value) -> usize {
	serde_json::to_vec(item).unwrap().len()
}

/// A finished file: its length on disk and its parsed JSON.
pub(super) struct Finished {
	pub len: u64,
	pub json: Value,
	/// The envelope's items key.
	pub items_key: String,
}

impl Finished {
	pub fn batch(&self) -> &Vec<Value> {
		self.json[&self.items_key].as_array().expect("items array")
	}
}

/// Reads one finished file and checks it is exactly what `envelope` describes: it starts
/// with the opening, has the items key, every field with its value, the timestamp if
/// configured, and no other key.
pub(super) fn read_finished(path: &std::path::Path, envelope: &Envelope) -> Result<Finished> {
	let len = fs::metadata(path)?.len();
	let content = fs::read_to_string(path)?;
	let opening = format!("{{{}:[", serde_json::to_string(&envelope.items_key)?);
	assert!(
		content.starts_with(&opening),
		"Bad envelope start: {content}"
	);
	let json: Value = serde_json::from_str(&content)?;
	items_of(&json, envelope);
	let finished = Finished {
		len,
		json,
		items_key: envelope.items_key.clone(),
	};
	assert!(
		!finished.batch().is_empty(),
		"Empty or envelope-only file: {content}"
	);
	Ok(finished)
}

/// Fetches every finished file, in file order.
pub(super) fn finished_files(store: &mut DirectoryStore) -> Result<Vec<Finished>> {
	let Some(result) = store.fetch(None, None)? else {
		return Ok(Vec::new());
	};
	let envelope = store.config.envelope.clone();
	result
		.data
		.unwrap()
		.iter()
		.map(|f| read_finished(f, &envelope))
		.collect()
}

/// Asserts that no file is over `max` bytes and returns every item in file order.
pub(super) fn assert_ceiling(files: &[Finished], max: usize) -> Vec<Value> {
	let mut items = Vec::new();
	for (i, file) in files.iter().enumerate() {
		assert!(
			file.len <= max as u64,
			"file {i} is {} bytes, over max_file_size {max}",
			file.len
		);
		items.extend(file.batch().iter().cloned());
	}
	items
}

pub(super) fn batch_counts(files: &[Finished]) -> Vec<usize> {
	files.iter().map(|f| f.batch().len()).collect()
}

/// Every file in the store's directory, finished or not.
pub(super) fn files_on_disk(dir: &TempDir) -> Result<Vec<PathBuf>> {
	let mut files: Vec<PathBuf> = fs::read_dir(dir.path())?
		.map(|e| e.map(|e| e.path()))
		.collect::<Result<_>>()?;
	files.sort();
	Ok(files)
}

pub(super) fn assert_invalid_input(err: &io::Error) {
	assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{err}");
}

/// Every file under `root`, recursively, with its bytes, keyed by path relative to it.
pub(super) fn snapshot(
	root: &std::path::Path,
) -> Result<std::collections::BTreeMap<PathBuf, Vec<u8>>> {
	let mut files = std::collections::BTreeMap::new();
	let mut dirs = vec![root.to_path_buf()];
	while let Some(dir) = dirs.pop() {
		for entry in fs::read_dir(&dir)? {
			let path = entry?.path();
			if path.is_dir() {
				dirs.push(path);
			} else {
				let relative = path.strip_prefix(root).unwrap().to_path_buf();
				files.insert(relative, fs::read(&path)?);
			}
		}
	}
	Ok(files)
}

/// The bytes a store with `envelope` writes for `items` in an open file: the opening,
/// then the items separated by commas, without a closing.
pub(super) fn unclosed_with(envelope: &Envelope, items: &[Value]) -> Vec<u8> {
	let mut bytes = envelope.opening();
	for (i, item) in items.iter().enumerate() {
		if i > 0 {
			bytes.push(b',');
		}
		bytes.extend(serde_json::to_vec(item).unwrap());
	}
	bytes
}

/// A fixed finishing time, so files built by the tests are reproducible.
pub(super) fn fixed_time() -> chrono::DateTime<chrono::Utc> {
	chrono::DateTime::parse_from_rfc3339("2026-11-09T12:34:56.789Z")
		.unwrap()
		.to_utc()
}

/// `unclosed_with(envelope, items)` followed by its closing at [`fixed_time`], as a file
/// finished but not yet renamed.
pub(super) fn closed_with(envelope: &Envelope, items: &[Value]) -> Vec<u8> {
	let mut bytes = unclosed_with(envelope, items);
	bytes.extend(envelope.closing(fixed_time()));
	bytes
}

/// [`unclosed_with`] under [`with_source`]`(KEY)`, the envelope of [`store`].
pub(super) fn unclosed(items: &[Value]) -> Vec<u8> {
	unclosed_with(&with_source(KEY), items)
}

/// [`closed_with`] under [`with_source`]`(KEY)`, the envelope of [`store`].
pub(super) fn closed(items: &[Value]) -> Vec<u8> {
	closed_with(&with_source(KEY), items)
}

/// Writes `bytes` as the "events" store's unfinished file `index`, as an earlier session
/// would have left it. Returns its path.
pub(super) fn write_leftover(dir: &TempDir, index: u32, bytes: &[u8]) -> Result<PathBuf> {
	let path = dir.path().join(format!("{index}-events"));
	fs::write(&path, bytes)?;
	Ok(path)
}

/// The path `path` is renamed to once finished.
pub(super) fn finished(path: &std::path::Path) -> PathBuf {
	DirectoryStore::finished_path(path)
}
