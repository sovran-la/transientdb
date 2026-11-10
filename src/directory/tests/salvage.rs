//! A leftover that is not a complete envelope is salvaged: cut back to its last complete
//! item and closed, so every delivered file is valid JSON holding only whole original
//! items. One of the store's files that does not start like a data file is deleted.

use super::support::*;
use super::*;
use crate::directory::recovery::{salvage, Salvage};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const OPENING_BYTES: &[u8] = b"{ \"batch\": [";

/// Items a naive scanner for `,`, `}`, `]` or `"` would split in the wrong place.
fn awkward_items() -> Vec<Value> {
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

/// The end offset in `unclosed(items)` of each item.
fn item_ends(items: &[Value]) -> Vec<usize> {
	let mut end = OPENING_BYTES.len();
	items
		.iter()
		.enumerate()
		.map(|(i, item)| {
			end += usize::from(i > 0) + item_len(item);
			end
		})
		.collect()
}

/// Recovers `bytes` as the only leftover in a fresh directory; returns the fetched
/// files (finished, with their bytes) and what is left on disk.
fn recover(bytes: &[u8], max_file_size: usize) -> Result<(Vec<Vec<u8>>, Vec<PathBuf>, TempDir)> {
	let temp_dir = TempDir::new()?;
	write_leftover(&temp_dir, 1, bytes)?;
	let mut store = store(&temp_dir, max_file_size)?;
	let files: Vec<PathBuf> = store
		.fetch(None, None)?
		.map(|r| r.data.unwrap())
		.unwrap_or_default();
	let contents = files.iter().map(fs::read).collect::<Result<_>>()?;
	let on_disk = files_on_disk(&temp_dir)?;
	Ok((contents, on_disk, temp_dir))
}

fn batch_of(bytes: &[u8]) -> Vec<Value> {
	let json: Value = serde_json::from_slice(bytes).expect("valid JSON");
	assert_eq!(json["writeKey"], KEY);
	assert!(json["sentAt"].is_string());
	json["batch"].as_array().unwrap().clone()
}

/// Asserts the leftover `bytes` recovers to exactly `expected` items, with their
/// original bytes, or is deleted when `expected` is empty.
fn assert_recovers_to(bytes: &[u8], expected: &[Value], max: usize) -> Result<()> {
	let (files, on_disk, _dir) = recover(bytes, max)?;
	if expected.is_empty() {
		assert!(files.is_empty(), "{:?}", String::from_utf8_lossy(bytes));
		assert!(on_disk.is_empty(), "{:?}", String::from_utf8_lossy(bytes));
		return Ok(());
	}
	assert_eq!(files.len(), 1, "{:?}", String::from_utf8_lossy(bytes));
	let file = &files[0];
	assert!(file.len() <= max, "{} bytes over {max}", file.len());
	assert_eq!(
		&batch_of(file),
		expected,
		"{:?}",
		String::from_utf8_lossy(bytes)
	);
	let kept = unclosed(expected);
	assert!(file.starts_with(&kept), "original bytes not kept");
	assert_eq!(file.len(), kept.len() + CLOSING);
	Ok(())
}

#[test]
fn test_cut_at_every_offset_keeps_exactly_the_complete_items() -> Result<()> {
	let items = awkward_items();
	let full = unclosed(&items);
	let ends = item_ends(&items);
	assert_eq!(*ends.last().unwrap(), full.len());
	let max = 4096;
	assert!(full.len() + CLOSING <= max);

	for cut in OPENING_BYTES.len()..=full.len() {
		// Items ending at or before the cut are complete, except a number ending
		// exactly at it, which might have been longer.
		let complete = ends
			.iter()
			.zip(&items)
			.take_while(|(end, item)| **end < cut || (**end == cut && !item.is_number()))
			.count();
		assert_recovers_to(&full[..cut], &items[..complete], max)
			.map_err(|e| io::Error::other(format!("cut {cut}: {e}")))?;
	}
	Ok(())
}

#[test]
fn test_cut_inside_multibyte_utf8_drops_the_item() -> Result<()> {
	let items = vec![json!("ascii"), json!("🦀🦀")];
	let full = unclosed(&items);
	let crab = full.len() - 1 - 4;
	for cut in crab - 3..crab + 4 {
		assert_recovers_to(&full[..cut], &items[..1], 1024)?;
	}
	Ok(())
}

#[test]
fn test_trailing_bytes_after_the_last_item_are_cut() -> Result<()> {
	let items = vec![json!({"a": 1}), json!([2, "two"]), json!("three")];
	let clean = unclosed(&items);
	let mut nul_after_comma = clean.clone();
	nul_after_comma.extend_from_slice(b",\0\0");
	let mut invalid_utf8 = clean.clone();
	invalid_utf8.extend_from_slice(&[0xff, 0xfe, b'"']);
	let mut partial_closing = clean.clone();
	partial_closing.extend_from_slice(b"],\"sentAt\":\"2026-11-");
	let mut closing_then_garbage = closed(&items);
	closing_then_garbage.extend_from_slice(b"garbage");
	let mut tails: Vec<Vec<u8>> = [
		&b","[..],
		b",,",
		b", ",
		b"\0",
		b"\0\0\0\0\0\0\0\0\0\0\0\0",
		b"garbage data!!!",
		b"}",
		b"]",
		b" \n",
		b",{\"partial\": [1, 2",
		b",\"unterminated",
		b",nul",
		b",1x",
	]
	.iter()
	.map(|tail| [&clean[..], tail].concat())
	.collect();
	tails.extend([
		nul_after_comma,
		invalid_utf8,
		partial_closing,
		closing_then_garbage,
	]);

	for bytes in tails {
		assert_recovers_to(&bytes, &items, 1024)?;
	}
	Ok(())
}

#[test]
fn test_no_complete_item_deletes_the_file() -> Result<()> {
	for bytes in [
		&b"{ \"batch\": ["[..],
		b"{ \"batch\": [\0\0\0",
		b"{ \"batch\": [,",
		b"{ \"batch\": [{\"a\":",
		b"{ \"batch\": [\"cut",
		b"{ \"batch\": [garbage",
		b"{ \"batch\": []",
		b"{ \"batch\": [],\"sentAt\":\"s\",\"writeKey\":\"k\"}",
	] {
		assert_recovers_to(bytes, &[], 1024)?;
	}
	// Never a file with an empty batch, even one already closed.
	assert_recovers_to(&closed(&[]), &[], 1024)?;
	Ok(())
}

#[test]
fn test_unreadable_opening_is_deleted() -> Result<()> {
	let cases: Vec<Vec<u8>> = vec![
		Vec::new(),
		b"{".to_vec(),
		b"{ \"batch\"".to_vec(),
		b"{ \"batch\": ".to_vec(),
		b"{\"batch\":[1,2".to_vec(),
		b"{ \"batch\" : [1".to_vec(),
		b"[1,2,3".to_vec(),
		b"garbage".to_vec(),
		vec![0xff, 0xff, 0xff, 0xff],
		b"\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0".to_vec(),
		b"{\"not_batch\": [], \"not_write_key\": \"\"}".to_vec(),
	];
	for bytes in cases {
		let temp_dir = TempDir::new()?;
		let path = write_leftover(&temp_dir, 1, &bytes)?;
		let case = String::from_utf8_lossy(&bytes).into_owned();
		for session in 0..2 {
			let mut store = store(&temp_dir, 1024)?;
			assert_eq!(store.has_data(), session == 0, "{case:?}");
			let validated = Arc::new(Mutex::new(0));
			let count = validated.clone();
			store.set_file_validator(move |_| {
				*count.lock().unwrap() += 1;
				Ok(())
			});
			assert!(store.fetch(None, None)?.is_none(), "{case:?}");
			// Deleted without reaching the validator, and never renamed.
			assert_eq!(*validated.lock().unwrap(), 0, "{case:?}");
			assert!(!path.exists() && !finished(&path).exists(), "{case:?}");
			assert!(files_on_disk(&temp_dir)?.is_empty(), "{case:?}");
			assert!(!store.has_data(), "{case:?}");
		}
	}
	Ok(())
}

#[test]
fn test_complete_envelope_is_renamed_and_not_closed_again() -> Result<()> {
	let items = awkward_items();
	let bytes = closed(&items);
	let (files, _, _dir) = recover(&bytes, 4096)?;
	assert_eq!(files, vec![bytes]);
	Ok(())
}

#[test]
fn test_clean_unfinished_file_is_closed_normally() -> Result<()> {
	let items = awkward_items();
	let bytes = unclosed(&items);
	let (files, _, _dir) = recover(&bytes, 4096)?;
	assert_eq!(files.len(), 1);
	assert!(files[0].starts_with(&bytes));
	assert_eq!(files[0].len(), bytes.len() + CLOSING);
	assert_eq!(batch_of(&files[0]), items);
	Ok(())
}

#[test]
fn test_top_level_scalars() -> Result<()> {
	let cases: Vec<(&str, Vec<Value>)> = vec![
		// A number at the very end may have been cut short: dropped.
		("1,22,333", vec![json!(1), json!(22)]),
		("42", vec![]),
		("1,-0.5e10", vec![json!(1)]),
		// A comma, or any delimiter, after it shows it was complete.
		("1,22,333,", vec![json!(1), json!(22), json!(333)]),
		("1,22,333 ", vec![json!(1), json!(22), json!(333)]),
		("1,22,333]", vec![json!(1), json!(22), json!(333)]),
		// A number followed by anything that cannot end a number is not one.
		("1,22x", vec![json!(1)]),
		("1,22\0", vec![json!(1)]),
		// Incomplete numbers.
		("1,-", vec![json!(1)]),
		("1,2.", vec![json!(1)]),
		("1,2e", vec![json!(1)]),
		("1,2e+", vec![json!(1)]),
		// Strings and literals show their own end.
		("\"a\",\"b\"", vec![json!("a"), json!("b")]),
		(
			"true,false,null",
			vec![json!(true), json!(false), json!(null)],
		),
		("true,nul", vec![json!(true)]),
		("\"a\",\"b", vec![json!("a")]),
		("7,\"seven\"", vec![json!(7), json!("seven")]),
	];
	for (items, expected) in cases {
		let bytes = [OPENING_BYTES, items.as_bytes()].concat();
		assert_recovers_to(&bytes, &expected, 1024)
			.map_err(|e| io::Error::other(format!("{items:?}: {e}")))?;
	}
	Ok(())
}

#[test]
fn test_salvage_rule_directly() {
	let max = usize::MAX;
	assert_eq!(
		salvage(OPENING_BYTES, b"{ \"batch", max),
		Salvage::Unreadable
	);
	assert_eq!(salvage(OPENING_BYTES, b"", max), Salvage::Unreadable);
	assert_eq!(
		salvage(OPENING_BYTES, b"{\"batch\":[1,", max),
		Salvage::Unreadable
	);
	assert_eq!(
		salvage(OPENING_BYTES, b"{ \"batch\": [", max),
		Salvage::Empty
	);
	assert_eq!(
		salvage(OPENING_BYTES, b"{ \"batch\": [1", max),
		Salvage::Empty
	);
	assert_eq!(
		salvage(OPENING_BYTES, b"{ \"batch\": [1,", max),
		Salvage::Keep(13)
	);
	assert_eq!(
		salvage(OPENING_BYTES, b"{ \"batch\": [ 1 , \"x\" ,", max),
		Salvage::Keep(20)
	);
	assert_eq!(
		salvage(OPENING_BYTES, b"{ \"batch\": [\"a\",\"bc\"", max),
		Salvage::Keep(20)
	);
	// max_len bounds the end of the last kept item.
	let bytes = b"{ \"batch\": [\"a\",\"bc\"";
	assert_eq!(salvage(OPENING_BYTES, bytes, 19), Salvage::Keep(15));
	assert_eq!(salvage(OPENING_BYTES, bytes, 15), Salvage::Keep(15));
	assert_eq!(salvage(OPENING_BYTES, bytes, 14), Salvage::Empty);
}

#[test]
fn test_salvaged_file_stays_within_max_file_size() -> Result<()> {
	// A leftover written under a larger max_file_size: only the leading items that fit
	// under the current one, closing included, are kept.
	let items: Vec<Value> = (0..6).map(|_| item_of_len(40)).collect();
	let mut bytes = unclosed(&items);
	bytes.extend_from_slice(b",{\"cut");
	// 12 + 3 * 40 + 2 + 60 = 194 fits in 200; a fourth item does not.
	assert_recovers_to(&bytes, &items[..3], 200)?;
	assert_recovers_to(&bytes, &items[..2], 193)?;
	Ok(())
}

#[test]
fn test_salvaged_files_go_through_the_validator() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let kept = vec![json!({"keep": 1}), json!({"keep": 2})];
	let cut_kept = [unclosed(&kept), b",{\"keep\": 3, \"cut".to_vec()].concat();
	let cut_rejected = [unclosed(&[json!("reject")]), b",\"cut".to_vec()].concat();
	let first = write_leftover(&temp_dir, 1, &cut_kept)?;
	let second = write_leftover(&temp_dir, 2, &cut_rejected)?;

	let mut store = store(&temp_dir, 1024)?;
	let seen = Arc::new(Mutex::new(Vec::new()));
	let log = seen.clone();
	store.set_file_validator(move |path: &Path| {
		// The validator sees the salvaged file: valid JSON with only whole items.
		let content = fs::read(path)?;
		let json: Value = serde_json::from_slice(&content)?;
		log.lock()
			.unwrap()
			.push((path.to_path_buf(), json["batch"].clone()));
		if json["batch"][0] == "reject" {
			return Err(io::Error::other("rejected"));
		}
		Ok(())
	});

	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 1024), kept);
	let seen = seen.lock().unwrap().clone();
	assert_eq!(
		seen,
		vec![
			(first.clone(), json!(kept)),
			(second.clone(), json!(["reject"]))
		]
	);
	// The rejected salvage is deleted, like any rejected file.
	assert!(!second.exists() && !finished(&second).exists());
	assert_eq!(files_on_disk(&temp_dir)?, vec![finished(&first)]);
	Ok(())
}
