//! Helpers shared by the salvage tests.

use super::support::*;
use super::*;
use crate::envelope::samples::{every_json_type, expected_closing, items_of, SAMPLE_TIMESTAMP};
use std::path::PathBuf;

/// The opening of the default envelope and of `with_source(KEY)`.
pub(super) const OPENING_BYTES: &[u8] = b"{\"items\":[";

/// The envelopes the every-offset test runs under: the default, the one the other tests
/// use (a field and a timestamp), and one with a multi-byte items key and fields of
/// every JSON type.
pub(super) fn salvage_envelopes() -> Vec<Envelope> {
	vec![
		Envelope::default(),
		with_source(KEY),
		Envelope {
			items_key: "項目🦀".into(),
			fields: every_json_type(),
			timestamp_key: Some("at".into()),
		},
	]
}

/// Items a naive scanner for `,`, `}`, `]` or `"` would split in the wrong place.
pub(super) fn awkward_items() -> Vec<Value> {
	vec![
		json!({"a": {"b": [1, 2, {"c": "x,}]\"y"}]}, "n": -12.5e3}),
		json!(["arr", ",", "}", "]", "\"", "\\", "🦀", "},{", "],["]),
		json!("plain, with } and ] and \" and \\ and \n and \t and 🦀 and 漢字 and é"),
		json!(12345),
		json!({"deep": [[[[]]], {}], "t": true, "f": false, "z": null, "e": 1e-7}),
		json!("\u{1}\u{1f}\\u0041 not an escape once encoded"),
		json!(-0.5),
		json!({"日本語": "テキスト", "esc": "\"quoted\"", "empty": ""}),
		json!([true, null, false, 0]),
		json!({"last": "item, ends in a brace }"}),
	]
}

/// The end offset in `unclosed_with(envelope, items)` of each item.
pub(super) fn item_ends(envelope: &Envelope, items: &[Value]) -> Vec<usize> {
	let mut end = envelope.opening().len();
	items
		.iter()
		.enumerate()
		.map(|(i, item)| {
			end += usize::from(i > 0) + item_len(item);
			end
		})
		.collect()
}

/// Recovers `bytes` as the only leftover in a fresh directory, under `with_source(KEY)`;
/// returns the fetched files (finished, with their bytes) and what is left on disk.
pub(super) fn recover(
	bytes: &[u8],
	max_file_size: usize,
) -> Result<(Vec<Vec<u8>>, Vec<PathBuf>, TempDir)> {
	recover_with(&with_source(KEY), bytes, max_file_size)
}

/// [`recover`] under `envelope`.
pub(super) fn recover_with(
	envelope: &Envelope,
	bytes: &[u8],
	max_file_size: usize,
) -> Result<(Vec<Vec<u8>>, Vec<PathBuf>, TempDir)> {
	let temp_dir = TempDir::new()?;
	write_leftover(&temp_dir, 1, bytes)?;
	let mut store = DirectoryStore::new(config(&temp_dir, max_file_size, envelope.clone()))?;
	let files: Vec<PathBuf> = store
		.fetch(None, None)?
		.map(|r| r.data.unwrap())
		.unwrap_or_default();
	let contents = files.iter().map(fs::read).collect::<Result<_>>()?;
	let on_disk = files_on_disk(&temp_dir)?;
	Ok((contents, on_disk, temp_dir))
}

/// The items of a finished file, checking it is exactly what `envelope` describes.
pub(super) fn batch_of_with(envelope: &Envelope, bytes: &[u8]) -> Vec<Value> {
	let json: Value = serde_json::from_slice(bytes).expect("valid JSON");
	items_of(&json, envelope)
}

pub(super) fn batch_of(bytes: &[u8]) -> Vec<Value> {
	batch_of_with(&with_source(KEY), bytes)
}

/// Asserts the leftover `bytes` recovers, under `with_source(KEY)`, to exactly `expected`
/// items, with their original bytes, or is deleted when `expected` is empty.
pub(super) fn assert_recovers_to(bytes: &[u8], expected: &[Value], max: usize) -> Result<()> {
	assert_recovers_to_with(&with_source(KEY), bytes, expected, max)
}

/// [`assert_recovers_to`] under `envelope`: the kept items are closed with its closing.
pub(super) fn assert_recovers_to_with(
	envelope: &Envelope,
	bytes: &[u8],
	expected: &[Value],
	max: usize,
) -> Result<()> {
	let (files, on_disk, _dir) = recover_with(envelope, bytes, max)?;
	if expected.is_empty() {
		assert!(files.is_empty(), "{:?}", String::from_utf8_lossy(bytes));
		assert!(on_disk.is_empty(), "{:?}", String::from_utf8_lossy(bytes));
		return Ok(());
	}
	assert_eq!(files.len(), 1, "{:?}", String::from_utf8_lossy(bytes));
	let file = &files[0];
	assert!(file.len() <= max, "{} bytes over {max}", file.len());
	assert_eq!(
		&batch_of_with(envelope, file),
		expected,
		"{:?}",
		String::from_utf8_lossy(bytes)
	);
	let kept = unclosed_with(envelope, expected);
	assert!(file.starts_with(&kept), "original bytes not kept");
	let closing = expected_closing(envelope, SAMPLE_TIMESTAMP);
	assert_eq!(file.len(), kept.len() + closing.len());
	// The closing is the documented one, apart from the timestamp's value.
	let tail = String::from_utf8(file[kept.len()..].to_vec()).unwrap();
	if envelope.timestamp_key.is_some() {
		let at = tail.len() - 1 - 25;
		assert_eq!(tail[..at], closing[..at]);
		assert_eq!(tail[tail.len() - 2..], closing[closing.len() - 2..]);
	} else {
		assert_eq!(tail, closing);
	}
	Ok(())
}
