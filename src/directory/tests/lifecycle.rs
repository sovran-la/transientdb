//! Byte ceiling interplay with the item limit, fetch, recovery, validators and config.

use super::support::*;
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn limited_store(dir: &TempDir, max_file_size: usize, items: usize) -> Result<DirectoryStore> {
	let mut config = config(dir, max_file_size, KEY);
	config.max_file_items = Some(items);
	DirectoryStore::new(config)
}

fn append_all(store: &mut DirectoryStore, items: &[Value]) -> Result<()> {
	items.iter().try_for_each(|item| store.append(item.clone()))
}

#[test]
fn test_item_limit_trips_before_byte_ceiling() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = limited_store(&temp_dir, 1000, 3)?;
	let items: Vec<Value> = (0..10).map(|_| item_of_len(20)).collect();
	append_all(&mut store, &items)?;

	let files = finished_files(&mut store)?;
	assert_eq!(batch_counts(&files), vec![3, 3, 3, 1]);
	assert_eq!(assert_ceiling(&files, 1000), items);
	Ok(())
}

#[test]
fn test_byte_ceiling_trips_before_item_limit() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = limited_store(&temp_dir, 200, 5)?;
	// 72 + 41n - 1 <= 200 allows 3 items of 40 bytes, below the item limit of 5.
	let items: Vec<Value> = (0..10).map(|_| item_of_len(40)).collect();
	append_all(&mut store, &items)?;

	let files = finished_files(&mut store)?;
	assert_eq!(batch_counts(&files), vec![3, 3, 3, 1]);
	assert_eq!(assert_ceiling(&files, 200), items);
	Ok(())
}

#[test]
fn test_item_limit_and_byte_ceiling_trip_together() -> Result<()> {
	let temp_dir = TempDir::new()?;
	// Three 40 byte items and two commas fill a 194 byte file exactly.
	let max = ENVELOPE + 3 * 40 + 2;
	let mut store = limited_store(&temp_dir, max, 3)?;
	let items: Vec<Value> = (0..10).map(|_| item_of_len(40)).collect();
	append_all(&mut store, &items)?;

	let files = finished_files(&mut store)?;
	assert_eq!(batch_counts(&files), vec![3, 3, 3, 1]);
	for file in &files[..3] {
		assert_eq!(file.len, max as u64);
	}
	assert_eq!(assert_ceiling(&files, max), items);
	Ok(())
}

#[test]
fn test_no_empty_or_envelope_only_files() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = limited_store(&temp_dir, 150, 4)?;
	let mut accepted = Vec::new();
	for i in 0..40 {
		if i % 4 == 0 {
			assert_invalid_input(&store.append(item_of_len(79)).unwrap_err());
		}
		let item = item_of_len(2 + (i * 13) % 60);
		store.append(item.clone())?;
		accepted.push(item);
	}
	assert_invalid_input(&store.append(item_of_len(79)).unwrap_err());

	let on_disk = store.sorted_files(true)?;
	// finished_files asserts every batch is non-empty.
	let files = finished_files(&mut store)?;
	assert_eq!(files.len(), on_disk.len());
	assert_eq!(files_on_disk(&temp_dir)?.len(), files.len());
	assert!(batch_counts(&files).iter().all(|&c| c <= 4));
	assert_eq!(assert_ceiling(&files, 150), accepted);
	Ok(())
}

#[test]
fn test_fetch_orders_files_by_numeric_index() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = limited_store(&temp_dir, 1024, 1)?;
	// Twelve files: "10-events" must come after "9-events", not after "1-events".
	let items: Vec<Value> = (0..12).map(|i| json!({"i": i})).collect();
	append_all(&mut store, &items)?;

	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 1024), items);

	// A count limit takes the oldest files, not the lexically smallest.
	let oldest = store.fetch(Some(3), None)?.unwrap().data.unwrap();
	let indexes: Vec<Value> = oldest
		.iter()
		.map(|f| read_finished(f, KEY).map(|f| f.batch()[0]["i"].clone()))
		.collect::<Result<_>>()?;
	assert_eq!(indexes, vec![json!(0), json!(1), json!(2)]);
	Ok(())
}

#[test]
fn test_fetch_closes_partial_file_within_ceiling() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 500)?;
	store.append(item_of_len(30))?;
	store.append(item_of_len(30))?;

	let files = finished_files(&mut store)?;
	assert_eq!(files.len(), 1);
	assert_eq!(files[0].len, (ENVELOPE + 61) as u64);
	assert_ceiling(&files, 500);
	Ok(())
}

#[test]
fn test_recovered_file_respects_ceiling() -> Result<()> {
	let temp_dir = TempDir::new()?;
	{
		let mut store = store(&temp_dir, 200)?;
		// Fills the open file to exactly 200 bytes once closed; dropped unfinished.
		store.append(item_of_len(60))?;
		store.append(item_of_len(67))?;
	}

	let mut store = store(&temp_dir, 200)?;
	let files = finished_files(&mut store)?;
	assert_eq!(batch_counts(&files), vec![2]);
	assert_eq!(files[0].len, 200);
	Ok(())
}

#[test]
fn test_validator_runs_on_byte_ceiling_rotation() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 200)?;
	let validated = Arc::new(AtomicUsize::new(0));
	let counter = validated.clone();
	store.set_file_validator(move |path| {
		assert!(fs::metadata(path)?.len() <= 200);
		serde_json::from_str::<Value>(&fs::read_to_string(path)?)?;
		counter.fetch_add(1, Ordering::SeqCst);
		Ok(())
	});

	// Three 40 byte items per file: 7 items close two files by rotation.
	for _ in 0..7 {
		store.append(item_of_len(40))?;
	}
	assert_eq!(validated.load(Ordering::SeqCst), 2);
	assert_eq!(batch_counts(&finished_files(&mut store)?), vec![3, 3, 1]);
	assert_eq!(validated.load(Ordering::SeqCst), 3);
	Ok(())
}

#[test]
fn test_validator_failure_on_byte_ceiling_rotation_propagates() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 200)?;
	store.set_file_validator(|_| Err(io::Error::other("rejected")));

	for _ in 0..3 {
		store.append(item_of_len(40))?;
	}
	let err = store.append(item_of_len(40)).unwrap_err();
	assert_eq!(err.to_string(), "rejected");
	Ok(())
}

#[test]
#[should_panic(
	expected = "max_file_size is all envelope and no room for an item? That's packaging, not storage."
)]
fn test_rejects_envelope_that_leaves_no_room() {
	let temp_dir = TempDir::new().unwrap();
	// 12 + (52 + 36) = 100: the envelope alone fills the file.
	let _ = DirectoryStore::new(config(&temp_dir, 100, &"k".repeat(36)));
}

#[test]
fn test_envelope_leaving_one_byte_is_accepted() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let key = "k".repeat(35);
	let mut store = DirectoryStore::new(config(&temp_dir, 100, &key))?;
	assert_eq!(store.max_item_size(), 1);
	assert_invalid_input(&store.append(json!(10)).unwrap_err());
	store.append(json!(7))?;

	let files = finished_files(&mut store)?;
	assert_eq!(files.len(), 1);
	assert_eq!(files[0].len, 100);
	Ok(())
}

#[test]
fn test_rejected_envelope_config_creates_no_directory() {
	let temp_dir = TempDir::new().unwrap();
	let location = temp_dir.path().join("never-created");
	let mut config = config(&temp_dir, 100, &"k".repeat(36));
	config.storage_location = location.clone();
	let result = std::panic::catch_unwind(|| DirectoryStore::new(config));
	assert!(result.is_err());
	assert!(!location.exists());
}

#[test]
fn test_validator_failure_does_not_corrupt_next_file() -> Result<()> {
	use std::sync::atomic::AtomicBool;

	let temp_dir = TempDir::new()?;
	let mut store = DirectoryStore::new(item_limit_config(&temp_dir, Some(1)))?;
	let reject = Arc::new(AtomicBool::new(true));
	let flag = reject.clone();
	store.set_file_validator(move |_| {
		if flag.swap(false, Ordering::SeqCst) {
			return Err(io::Error::other("rejected once"));
		}
		Ok(())
	});

	append_indexed(&mut store, 0..1)?;
	// Rotation finishes file 0; its validator rejects it, so the append fails.
	let err = store.append(json!({"index": 1})).unwrap_err();
	assert_eq!(err.to_string(), "rejected once");

	// The store must start a clean file, not continue the abandoned one's accounting.
	append_indexed(&mut store, 2..4)?;
	let files = store.fetch(None, None)?.unwrap().data.unwrap();
	let mut indexes = Vec::new();
	for file in &files {
		for item in read_batch(file)? {
			indexes.push(item["index"].as_u64().unwrap());
		}
	}
	assert_eq!(indexes, vec![2, 3]);
	Ok(())
}
