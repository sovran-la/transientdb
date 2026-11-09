//! Helpers shared by the byte-ceiling tests.

use super::*;
use std::path::PathBuf;

pub(super) const KEY: &str = "test-key";
/// `{ "batch": [` with `write_key` "test-key".
pub(super) const OPENING: usize = 12;
/// `],"sentAt":"<24 byte timestamp>","writeKey":"test-key"}`.
pub(super) const CLOSING: usize = 60;
pub(super) const ENVELOPE: usize = OPENING + CLOSING;

pub(super) fn config(dir: &TempDir, max_file_size: usize, write_key: &str) -> DirectoryConfig {
	DirectoryConfig {
		write_key: write_key.to_string(),
		storage_location: dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size,
		max_file_items: None,
	}
}

pub(super) fn store(dir: &TempDir, max_file_size: usize) -> Result<DirectoryStore> {
	DirectoryStore::new(config(dir, max_file_size, KEY))
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
}

impl Finished {
	pub fn batch(&self) -> &Vec<Value> {
		self.json["batch"].as_array().expect("batch array")
	}
}

/// Reads one finished file and checks its envelope.
pub(super) fn read_finished(path: &std::path::Path, write_key: &str) -> Result<Finished> {
	let len = fs::metadata(path)?.len();
	let content = fs::read_to_string(path)?;
	assert!(
		content.starts_with("{ \"batch\": ["),
		"Bad envelope start: {content}"
	);
	let json: Value = serde_json::from_str(&content)?;
	assert!(json["sentAt"].is_string(), "Missing sentAt: {content}");
	assert_eq!(json["writeKey"], write_key);
	let finished = Finished { len, json };
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
	let key = store.config.write_key.clone();
	result
		.data
		.unwrap()
		.iter()
		.map(|f| read_finished(f, &key))
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
