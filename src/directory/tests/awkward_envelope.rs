//! Every envelope, awkward keys and values included, produces valid files of exactly the
//! documented bytes, and the byte ceiling counts the escaped bytes.

use super::support::*;
use super::*;
use crate::envelope::samples::{
	expected_closing, expected_len, expected_opening, sample_envelopes,
};

#[test]
fn test_awkward_envelopes_produce_valid_files_with_that_envelope() -> Result<()> {
	for envelope in sample_envelopes() {
		let temp_dir = TempDir::new()?;
		let max = expected_len(&envelope) + 120;
		let mut store = DirectoryStore::new(config(&temp_dir, max, envelope.clone()))?;
		let appended: Vec<Value> = (0..40).map(|i| json!({"i": i})).collect();
		for item in &appended {
			store.append(item.clone())?;
		}

		// finished_files checks each file has exactly the envelope's keys and values.
		let files = finished_files(&mut store)?;
		assert!(files.len() > 1, "{envelope:?} should rotate");
		assert_eq!(assert_ceiling(&files, max), appended, "{envelope:?}");
	}
	Ok(())
}

#[test]
fn test_opening_and_closing_lengths_count_escaped_bytes() -> Result<()> {
	for envelope in sample_envelopes() {
		let temp_dir = TempDir::new()?;
		let store = DirectoryStore::new(config(&temp_dir, 1024, envelope.clone()))?;
		assert_eq!(store.opening(), expected_opening(&envelope).as_bytes());
		assert_eq!(
			store.closing_len,
			expected_closing(&envelope, "2026-11-09T12:34:56.789Z").len(),
			"{envelope:?}"
		);
	}
	Ok(())
}

#[test]
fn test_awkward_envelopes_exact_fit_lands_on_the_limit() -> Result<()> {
	for envelope in sample_envelopes() {
		let temp_dir = TempDir::new()?;
		// Two items and their separator fill the file exactly.
		let max = (expected_len(&envelope) + 2 * 40 + 1).max(101);
		let first = item_of_len(40);
		let second = item_of_len(max - expected_len(&envelope) - 40 - 1);
		let mut store = DirectoryStore::new(config(&temp_dir, max, envelope.clone()))?;
		for item in [&first, &second, &first, &second, &first] {
			store.append(item.clone())?;
		}
		let files = finished_files(&mut store)?;
		assert_eq!(batch_counts(&files), vec![2, 2, 1], "{envelope:?}");
		assert_eq!(files[0].len, max as u64, "{envelope:?}");
		assert_eq!(files[1].len, max as u64, "{envelope:?}");
		assert_ceiling(&files, max);

		// One byte less and only one item fits per file.
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(config(&temp_dir, max - 1, envelope.clone()))?;
		store.append(first.clone())?;
		store.append(second.clone())?;
		let files = finished_files(&mut store)?;
		assert_eq!(batch_counts(&files), vec![1, 1], "{envelope:?}");
		assert_ceiling(&files, max - 1);
	}
	Ok(())
}

#[test]
fn test_largest_item_under_awkward_envelope_fills_file_exactly() -> Result<()> {
	for envelope in sample_envelopes() {
		let temp_dir = TempDir::new()?;
		let max = expected_len(&envelope) + 150;
		let mut store = DirectoryStore::new(config(&temp_dir, max, envelope.clone()))?;
		let largest = store.max_item_size();
		assert_eq!(largest, 150, "{envelope:?}");
		assert_invalid_input(&store.append(item_of_len(largest + 1)).unwrap_err());
		store.append(item_of_len(largest))?;
		let files = finished_files(&mut store)?;
		assert_eq!(files.len(), 1);
		assert_eq!(files[0].len, max as u64, "{envelope:?}");
	}
	Ok(())
}
