//! `write_key` is written as an escaped JSON string, and the byte ceiling counts the
//! escaped bytes.

use super::support::*;
use super::*;

use crate::batch::AWKWARD_WRITE_KEYS as AWKWARD_KEYS;

/// The closing this key must produce, built independently of the store.
fn expected_closing_len(write_key: &str) -> usize {
	let timestamp = "2026-11-09T12:34:56.789Z";
	let key = serde_json::to_string(write_key).unwrap();
	format!("],\"sentAt\":\"{timestamp}\",\"writeKey\":{key}}}").len()
}

#[test]
fn test_awkward_keys_produce_valid_files_with_that_key() -> Result<()> {
	for key in AWKWARD_KEYS {
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(config(&temp_dir, 300, key))?;
		let appended: Vec<Value> = (0..40).map(|i| json!({"i": i})).collect();
		for item in &appended {
			store.append(item.clone())?;
		}

		// finished_files parses each file and checks its writeKey equals the key.
		let files = finished_files(&mut store)?;
		assert!(files.len() > 1, "key {key:?} should rotate");
		assert_eq!(assert_ceiling(&files, 300), appended, "key {key:?}");
	}
	Ok(())
}

#[test]
fn test_closing_length_counts_escaped_bytes() -> Result<()> {
	for key in AWKWARD_KEYS {
		let temp_dir = TempDir::new()?;
		let store = DirectoryStore::new(config(&temp_dir, 300, key))?;
		assert_eq!(store.closing_len, expected_closing_len(key), "key {key:?}");
	}
	Ok(())
}

#[test]
fn test_awkward_keys_exact_fit_lands_on_the_limit() -> Result<()> {
	for key in AWKWARD_KEYS {
		let temp_dir = TempDir::new()?;
		let item = item_of_len(40);
		// Two items and their separator fill the file exactly.
		let max = OPENING + expected_closing_len(key) + 2 * 40 + 1;
		let mut store = DirectoryStore::new(config(&temp_dir, max, key))?;
		for _ in 0..5 {
			store.append(item.clone())?;
		}
		let files = finished_files(&mut store)?;
		assert_eq!(batch_counts(&files), vec![2, 2, 1], "key {key:?}");
		assert_eq!(files[0].len, max as u64, "key {key:?}");
		assert_eq!(files[1].len, max as u64, "key {key:?}");
		assert_ceiling(&files, max);

		// One byte less and only one item fits per file.
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(config(&temp_dir, max - 1, key))?;
		for _ in 0..3 {
			store.append(item.clone())?;
		}
		let files = finished_files(&mut store)?;
		assert_eq!(batch_counts(&files), vec![1, 1, 1], "key {key:?}");
		assert_ceiling(&files, max - 1);
	}
	Ok(())
}

#[test]
fn test_largest_item_with_awkward_key_fills_file_exactly() -> Result<()> {
	for key in AWKWARD_KEYS {
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(config(&temp_dir, 200, key))?;
		let largest = store.max_item_size();
		assert_eq!(largest, 200 - OPENING - expected_closing_len(key));
		assert_invalid_input(&store.append(item_of_len(largest + 1)).unwrap_err());
		store.append(item_of_len(largest))?;
		let files = finished_files(&mut store)?;
		assert_eq!(files.len(), 1);
		assert_eq!(files[0].len, 200, "key {key:?}");
	}
	Ok(())
}
