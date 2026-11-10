//! A leftover that is already a complete envelope, but larger than `max_file_size`
//! because an earlier session wrote it under a larger limit, is cut down to the leading
//! items that fit, like a salvaged file. One at or under the limit is renamed untouched.

use super::support::*;
use super::*;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Six 40 byte items: 12 + 6 * 40 + 5 + 60 = 317 bytes once closed.
fn six_items() -> Vec<Value> {
	(0..6)
		.map(|i| Value::String(format!("{i}{}", "x".repeat(37))))
		.collect()
}

/// Fetches everything from a store over `dir` with `max_file_size`; returns the bytes
/// of each fetched file.
fn fetch_bytes(dir: &TempDir, max_file_size: usize) -> Result<Vec<Vec<u8>>> {
	let mut store = store(dir, max_file_size)?;
	let files: Vec<PathBuf> = store
		.fetch(None, None)?
		.map(|r| r.data.unwrap())
		.unwrap_or_default();
	files.iter().map(fs::read).collect()
}

/// Asserts `bytes` is a finished file of at most `max` bytes holding exactly `items`,
/// with their original bytes.
fn assert_cut_to(bytes: &[u8], items: &[Value], max: usize) {
	assert!(bytes.len() <= max, "{} bytes, over {max}", bytes.len());
	let json: Value = serde_json::from_slice(bytes).expect("valid JSON");
	assert_eq!(json["writeKey"], KEY);
	assert!(json["sentAt"].is_string());
	assert_eq!(json["batch"].as_array().unwrap(), items);
	let kept = unclosed(items);
	assert!(bytes.starts_with(&kept), "original bytes not kept");
	assert_eq!(bytes.len(), kept.len() + CLOSING);
}

#[test]
fn test_complete_leftover_over_the_ceiling_keeps_the_leading_items_that_fit() -> Result<()> {
	let items = six_items();
	let bytes = closed(&items);
	assert_eq!(bytes.len(), 317);
	// 12 + 3 * 40 + 2 + 60 = 194 fits in 200 and 193; a fourth item does not.
	for (max, kept) in [(200, 3), (194, 3), (193, 2), (316, 5), (153, 2), (152, 1)] {
		let temp_dir = TempDir::new()?;
		let path = write_leftover(&temp_dir, 1, &bytes)?;
		let files = fetch_bytes(&temp_dir, max)?;
		assert_eq!(files.len(), 1, "max={max}");
		assert_cut_to(&files[0], &items[..kept], max);
		assert_eq!(
			files_on_disk(&temp_dir)?,
			vec![finished(&path)],
			"max={max}"
		);
	}
	Ok(())
}

#[test]
fn test_complete_leftover_whose_first_item_does_not_fit_is_deleted() -> Result<()> {
	// 12 + 150 + 60 = 222 bytes: no 150 byte item fits in a 200 byte file.
	let bytes = closed(&[item_of_len(150), json!("small")]);
	let temp_dir = TempDir::new()?;
	let path = write_leftover(&temp_dir, 1, &bytes)?;
	let mut store = store(&temp_dir, 200)?;
	let validated = Arc::new(Mutex::new(0));
	let count = validated.clone();
	store.set_file_validator(move |_| {
		*count.lock().unwrap() += 1;
		Ok(())
	});
	assert!(store.fetch(None, None)?.is_none());
	assert_eq!(*validated.lock().unwrap(), 0);
	assert!(!path.exists() && !finished(&path).exists());
	assert!(files_on_disk(&temp_dir)?.is_empty());
	assert!(!store.has_data());
	Ok(())
}

#[test]
fn test_complete_leftover_at_or_under_the_ceiling_is_renamed_untouched() -> Result<()> {
	let bytes = closed(&six_items());
	for max in [bytes.len(), bytes.len() + 1] {
		let temp_dir = TempDir::new()?;
		let path = write_leftover(&temp_dir, 1, &bytes)?;
		let files = fetch_bytes(&temp_dir, max)?;
		assert_eq!(files, vec![bytes.clone()], "max={max}");
		assert_eq!(files_on_disk(&temp_dir)?, vec![finished(&path)]);
	}
	// One byte over is cut.
	let temp_dir = TempDir::new()?;
	write_leftover(&temp_dir, 1, &bytes)?;
	let files = fetch_bytes(&temp_dir, bytes.len() - 1)?;
	assert_cut_to(&files[0], &six_items()[..5], bytes.len() - 1);
	Ok(())
}

#[test]
fn test_validator_sees_the_cut_down_file() -> Result<()> {
	let items = six_items();
	let temp_dir = TempDir::new()?;
	let path = write_leftover(&temp_dir, 1, &closed(&items))?;
	let mut store = store(&temp_dir, 200)?;
	let seen = Arc::new(Mutex::new(Vec::new()));
	let log = seen.clone();
	store.set_file_validator(move |path: &Path| {
		log.lock()
			.unwrap()
			.push((path.to_path_buf(), fs::read(path)?));
		Ok(())
	});
	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 200), items[..3].to_vec());

	let seen = seen.lock().unwrap();
	assert_eq!(seen.len(), 1);
	assert_eq!(seen[0].0, path);
	assert_cut_to(&seen[0].1, &items[..3], 200);
	Ok(())
}

#[test]
fn test_validator_rejecting_the_cut_down_file_deletes_it() -> Result<()> {
	let temp_dir = TempDir::new()?;
	write_leftover(&temp_dir, 1, &closed(&six_items()))?;
	let mut store = store(&temp_dir, 200)?;
	store.set_file_validator(|_| Err(io::Error::other("rejected")));
	assert!(store.fetch(None, None)?.is_none());
	assert!(files_on_disk(&temp_dir)?.is_empty());
	Ok(())
}

#[test]
fn test_reopening_with_a_smaller_limit_respects_it_for_real_leftovers() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let first: Vec<Value> = (0..10)
		.map(|i| json!({"first": i, "pad": "p".repeat(20)}))
		.collect();
	let second: Vec<Value> = (0..10)
		.map(|i| json!({"second": i, "pad": "q".repeat(20)}))
		.collect();
	{
		let mut large = store(&temp_dir, 1024)?;
		for item in &first {
			large.append(item.clone())?;
		}
		// A directory at the finished path makes the rename fail: the file is closed
		// but keeps its unfinished name, as when a session ends between the two.
		let closed_path = large.current_path.clone().unwrap();
		fs::create_dir(finished(&closed_path))?;
		assert!(large.fetch(None, None).is_err());
		fs::remove_dir(finished(&closed_path))?;
		let closed_bytes = fs::read(&closed_path)?;
		assert!(serde_json::from_slice::<Value>(&closed_bytes).is_ok());
		assert!(closed_bytes.len() > 200);

		// And a file left open, never closed.
		for item in &second {
			large.append(item.clone())?;
		}
		assert!(fs::metadata(large.current_path.as_ref().unwrap())?.len() > 200);
	}
	assert_eq!(files_on_disk(&temp_dir)?.len(), 2);

	let mut small = store(&temp_dir, 200)?;
	let files = finished_files(&mut small)?;
	assert_eq!(files.len(), 2);
	assert_ceiling(&files, 200);
	// Each keeps a non-empty prefix of its items, in order.
	for (file, items) in files.iter().zip([&first, &second]) {
		let batch = file.batch();
		assert!(!batch.is_empty() && batch.len() < items.len());
		assert_eq!(batch[..], items[..batch.len()]);
	}
	Ok(())
}
