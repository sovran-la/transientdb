//! A write that fails partway leaves the open file a valid prefix: the opening and the
//! whole items before it, with the accounting unchanged, so the next append continues
//! it. If the file cannot even be rolled back, it is abandoned and salvaged later.

use super::support::*;
use super::*;
use crate::directory::data_file::Faults;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn faults(store: &DirectoryStore, set: impl FnOnce(&mut Faults)) {
	set(&mut store.faults.lock().unwrap());
}

fn heal(store: &DirectoryStore) {
	faults(store, |f| *f = Faults::default());
}

fn fail_after(store: &DirectoryStore, bytes: usize) {
	faults(store, |f| f.write_budget = Some(bytes));
}

/// The items in the open file, asserting it is exactly the opening and whole items
/// separated by commas, `current_size` bytes long.
fn open_items(store: &DirectoryStore) -> Vec<Value> {
	let path = store.current_path.as_ref().expect("open file");
	let bytes = fs::read(path).unwrap();
	assert_eq!(bytes.len(), store.current_size, "on disk vs accounted");
	let json: Value = serde_json::from_slice(&[&bytes[..], b"]}"].concat()).unwrap_or_else(|e| {
		panic!(
			"not a valid prefix ({e}): {}",
			String::from_utf8_lossy(&bytes)
		)
	});
	let items = json["items"].as_array().unwrap().clone();
	assert_eq!(items.len(), store.current_items);
	assert_eq!(bytes, unclosed(&items));
	items
}

fn long_item(tag: usize) -> Value {
	json!({"tag": tag, "text": "a long item, with } and ] and \" inside 🦀".repeat(3)})
}

/// Every finished file is valid, within `max`, and together they hold `expected`.
fn assert_fetches(store: &mut DirectoryStore, expected: &[Value], max: usize) -> Result<()> {
	let files = finished_files(store)?;
	assert_eq!(assert_ceiling(&files, max), expected);
	Ok(())
}

/// One failing append at `budget` bytes into the write (or a failing flush when
/// `budget` is `None`), after `a` and `b`; then `d` is appended successfully.
fn fails_then_recovers(budget: Option<usize>) -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 4096)?;
	let (a, b, c, d) = (long_item(0), long_item(1), long_item(2), long_item(3));
	store.append(a.clone())?;
	store.append(b.clone())?;
	let before = (
		store.current_path.clone(),
		store.current_size,
		store.current_items,
	);

	match budget {
		Some(bytes) => fail_after(&store, bytes),
		None => faults(&store, |f| f.fail_flush = true),
	}
	let err = store.append(c).unwrap_err();
	assert!(err.to_string().starts_with("injected"), "{err}");
	assert_eq!(
		(
			store.current_path.clone(),
			store.current_size,
			store.current_items
		),
		before
	);
	assert_eq!(open_items(&store), vec![a.clone(), b.clone()]);

	heal(&store);
	store.append(d.clone())?;
	assert_eq!(open_items(&store), vec![a.clone(), b.clone(), d.clone()]);
	assert_fetches(&mut store, &[a, b, d], 4096)?;
	assert_eq!(files_on_disk(&temp_dir)?.len(), 1);
	Ok(())
}

#[test]
fn test_failure_partway_through_an_item() -> Result<()> {
	let len = item_len(&long_item(2));
	// The comma plus part of the item, up to all of it but its last byte.
	for budget in [2, 3, len / 2, len - 1, len] {
		fails_then_recovers(Some(budget))?;
	}
	Ok(())
}

#[test]
fn test_failure_on_the_separator() -> Result<()> {
	// Nothing written, and only the comma written.
	fails_then_recovers(Some(0))?;
	fails_then_recovers(Some(1))
}

#[test]
fn test_failure_on_flush() -> Result<()> {
	fails_then_recovers(None)
}

#[test]
fn test_failure_on_the_first_item_of_a_file() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 4096)?;
	store.append(json!("seed"))?;
	finished_files(&mut store)?;
	// The next append opens a file; its opening is written, then the item fails.
	fail_after(&store, OPENING + 5);
	assert!(store.append(long_item(0)).is_err());
	assert_eq!(open_items(&store), Vec::<Value>::new());
	heal(&store);
	store.append(long_item(1))?;
	assert_fetches(&mut store, &[json!("seed"), long_item(1)], 4096)
}

#[test]
fn test_failure_writing_the_opening_of_a_new_file() -> Result<()> {
	for budget in [0, 1, OPENING - 1] {
		let temp_dir = TempDir::new()?;
		let mut store = store(&temp_dir, 4096)?;
		fail_after(&store, budget);
		assert!(store.append(json!("lost")).is_err());
		// No usable open file is left behind, and no file at all.
		assert!(store.writer.is_none());
		assert!(store.current_path.is_none());
		assert!(files_on_disk(&temp_dir)?.is_empty());
		assert!(!store.has_data());

		heal(&store);
		store.append(json!("kept"))?;
		assert_eq!(open_items(&store), vec![json!("kept")]);
		assert_fetches(&mut store, &[json!("kept")], 4096)?;
	}

	// The same on rotation: the finished file is unharmed. The closing of the finished
	// file goes through the same writer, so the budget lets all of it through.
	let temp_dir = TempDir::new()?;
	let mut config = config(&temp_dir, 4096, with_source(KEY));
	config.max_file_items = Some(1);
	let mut store = DirectoryStore::new(config)?;
	store.append(json!("a"))?;
	let first = store.current_path.clone().unwrap();
	fail_after(&store, CLOSING + 3);
	assert!(store.append(json!("lost")).is_err());
	assert!(store.writer.is_none());
	assert_eq!(store.sorted_files(true)?, vec![finished(&first)]);
	assert!(store.leftovers.is_empty());
	heal(&store);
	store.append(json!("c"))?;
	let files = finished_files(&mut store)?;
	assert_eq!(batch_counts(&files), vec![1, 1]);
	assert_eq!(assert_ceiling(&files, 4096), vec![json!("a"), json!("c")]);
	Ok(())
}

#[test]
fn test_failures_interleaved_with_successes() -> Result<()> {
	let max = 400;
	let temp_dir = TempDir::new()?;
	let mut config = config(&temp_dir, max, with_source(KEY));
	config.max_file_items = Some(4);
	let mut store = DirectoryStore::new(config)?;
	let validated = Arc::new(Mutex::new(Vec::<PathBuf>::new()));
	let log = validated.clone();
	store.set_file_validator(move |path| {
		serde_json::from_slice::<Value>(&fs::read(path)?)?;
		log.lock().unwrap().push(path.to_path_buf());
		Ok(())
	});

	let mut accepted = Vec::new();
	for i in 0..120 {
		let item = json!({"i": i, "pad": "p".repeat(i * 7 % 50), "s": "}],\""});
		let len = item_len(&item);
		// A flush failure is armed only for an append that writes into the open file;
		// finishing a file on rotation is not part of the write path under test.
		let rotates = store.writer.is_none() || !store.fits_in_open_file(len);
		let flush_fails = i % 5 == 3 && !rotates;
		match i % 5 {
			1 => fail_after(&store, i % (len + 2)),
			3 if flush_fails => faults(&store, |f| f.fail_flush = true),
			_ => {}
		}
		let result = store.append(item.clone());
		heal(&store);
		match result {
			Ok(()) => {
				assert!(!flush_fails, "a flush failure must fail the append");
				accepted.push(item);
			}
			Err(e) => assert!(i % 5 == 1 || flush_fails, "unexpected failure at {i}: {e}"),
		}
		if store.writer.is_some() {
			open_items(&store);
		}
	}
	assert!(
		accepted.len() < 120 - 10,
		"too few failures: {}",
		accepted.len()
	);
	assert!(
		accepted.len() > 120 / 2,
		"too few successes: {}",
		accepted.len()
	);
	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, max), accepted);
	assert!(batch_counts(&files).iter().all(|&n| (1..=4).contains(&n)));
	assert_eq!(validated.lock().unwrap().len(), files.len());
	Ok(())
}

#[test]
fn test_rollback_failure_abandons_the_file_for_salvage() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 4096)?;
	let (a, b) = (long_item(0), long_item(1));
	store.append(a.clone())?;
	store.append(b.clone())?;
	let abandoned = store.current_path.clone().unwrap();

	faults(&store, |f| {
		f.write_budget = Some(10);
		f.fail_truncate = true;
	});
	assert!(store.append(long_item(2)).is_err());
	// Stopped using it: no writer, no accounting, listed for the next fetch.
	assert!(store.writer.is_none());
	assert!(store.current_path.is_none());
	assert_eq!((store.current_size, store.current_items), (0, 0));
	assert_eq!(store.leftovers, vec![abandoned.clone()]);
	assert!(store.has_data());
	// The damaged file still has the partial bytes.
	assert!(fs::read(&abandoned)?.len() > unclosed(&[a.clone(), b.clone()]).len());

	heal(&store);
	let c = long_item(3);
	store.append(c.clone())?;
	assert_ne!(store.current_path.as_ref(), Some(&abandoned));
	assert_eq!(open_items(&store), vec![c.clone()]);

	// The fetch salvages the abandoned file's whole items, ahead of the new file.
	let files = finished_files(&mut store)?;
	assert_eq!(batch_counts(&files), vec![2, 1]);
	assert_eq!(assert_ceiling(&files, 4096), vec![a, b, c]);
	assert!(store.leftovers.is_empty());
	assert!(!abandoned.exists());
	Ok(())
}

#[test]
fn test_rollback_failure_on_a_files_first_item_deletes_it_at_fetch() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 4096)?;
	faults(&store, |f| {
		f.write_budget = Some(OPENING + 4);
		f.fail_truncate = true;
	});
	assert!(store.append(long_item(0)).is_err());
	assert_eq!(store.leftovers.len(), 1);
	heal(&store);
	// Nothing whole in it: salvage deletes it, and there is nothing to return.
	assert!(store.fetch(None, None)?.is_none());
	assert!(files_on_disk(&temp_dir)?.is_empty());
	assert!(!store.has_data());
	Ok(())
}

#[test]
fn test_failed_append_does_not_advance_the_item_limit() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut config = config(&temp_dir, 4096, with_source(KEY));
	config.max_file_items = Some(2);
	let mut store = DirectoryStore::new(config)?;
	store.append(json!("a"))?;
	for budget in [Some(0), Some(2), None] {
		match budget {
			Some(bytes) => fail_after(&store, bytes),
			None => faults(&store, |f| f.fail_flush = true),
		}
		assert!(store.append(json!("lost")).is_err());
		heal(&store);
		assert_eq!(store.current_items, 1);
	}
	let first = store.current_path.clone();
	// The failed appends did not count: "c" still fits in the first file.
	store.append(json!("c"))?;
	assert_eq!(store.current_path, first);
	store.append(json!("d"))?;
	assert_ne!(store.current_path, first);
	let files = finished_files(&mut store)?;
	assert_eq!(batch_counts(&files), vec![2, 1]);
	assert_eq!(
		assert_ceiling(&files, 4096),
		vec![json!("a"), json!("c"), json!("d")]
	);
	Ok(())
}

#[test]
fn test_failed_append_does_not_advance_the_byte_ceiling() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 200)?;
	store.append(item_of_len(60))?;
	// 10 + 60 + 1 + 71 + 58 = 200: the FILLER item fills the file exactly, so it only
	// fits in the same file if the failed attempts left current_size alone.
	// The write is FILLER + 1 bytes with its comma; every budget stops it short.
	for budget in [5, 40, FILLER] {
		fail_after(&store, budget);
		assert!(store.append(item_of_len(FILLER)).is_err());
		heal(&store);
		assert_eq!(store.current_size, OPENING + 60);
	}
	let first = store.current_path.clone();
	store.append(item_of_len(FILLER))?;
	assert_eq!(store.current_path, first);
	let files = finished_files(&mut store)?;
	assert_eq!(batch_counts(&files), vec![2]);
	assert_eq!(files[0].len, 200);
	Ok(())
}
