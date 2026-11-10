//! A leftover that is not a complete envelope is salvaged: cut back to its last complete
//! item and closed, so every delivered file is valid JSON holding only whole original
//! items. One of the store's files that does not start like a data file is deleted.

use super::salvage_support::*;
use super::support::*;
use super::*;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[test]
fn test_cut_at_every_offset_keeps_exactly_the_complete_items() -> Result<()> {
	for envelope in salvage_envelopes() {
		let items = awkward_items();
		let full = unclosed_with(&envelope, &items);
		let ends = item_ends(&envelope, &items);
		assert_eq!(*ends.last().unwrap(), full.len());
		let max = 4096;
		assert!(full.len() + envelope.closing(fixed_time()).len() <= max);

		for cut in envelope.opening().len()..=full.len() {
			// Items ending at or before the cut are complete, except a number ending
			// exactly at it, which might have been longer.
			let complete = ends
				.iter()
				.zip(&items)
				.take_while(|(end, item)| **end < cut || (**end == cut && !item.is_number()))
				.count();
			assert_recovers_to_with(&envelope, &full[..cut], &items[..complete], max)
				.map_err(|e| io::Error::other(format!("{envelope:?} cut {cut}: {e}")))?;
		}
		// Cut inside the opening: unreadable, deleted.
		for cut in 0..envelope.opening().len() {
			let (files, on_disk, _dir) = recover_with(&envelope, &full[..cut], max)?;
			assert!(
				files.is_empty() && on_disk.is_empty(),
				"{envelope:?} cut {cut}"
			);
		}
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
	partial_closing.extend_from_slice(b"],\"source\":\"test-");
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
		&b"{\"items\":["[..],
		b"{\"items\":[\0\0\0",
		b"{\"items\":[,",
		b"{\"items\":[{\"a\":",
		b"{\"items\":[\"cut",
		b"{\"items\":[garbage",
		b"{\"items\":[]",
		b"{\"items\":[],\"source\":\"test-key\",\"sentAt\":\"s\"}",
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
		b"{\"items\"".to_vec(),
		b"{\"items\":".to_vec(),
		b"{\"events\":[1,2".to_vec(),
		b"{ \"items\": [1".to_vec(),
		b"{\"items\" :[1".to_vec(),
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
fn test_salvaged_file_stays_within_max_file_size() -> Result<()> {
	// A leftover written under a larger max_file_size: only the leading items that fit
	// under the current one, closing included, are kept.
	let items: Vec<Value> = (0..6).map(|_| item_of_len(40)).collect();
	let mut bytes = unclosed(&items);
	bytes.extend_from_slice(b",{\"cut");
	// 10 + 3 * 40 + 2 + 58 = 190 fits in 200; a fourth item does not.
	let three = ENVELOPE + 3 * 40 + 2;
	assert_eq!(three, 190);
	assert_recovers_to(&bytes, &items[..3], 200)?;
	assert_recovers_to(&bytes, &items[..3], three)?;
	assert_recovers_to(&bytes, &items[..2], three - 1)?;
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
			.push((path.to_path_buf(), json["items"].clone()));
		if json["items"][0] == "reject" {
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
