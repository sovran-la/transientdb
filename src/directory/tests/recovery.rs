//! Recovery finishes every file an earlier session left without the `.temp` extension,
//! and never closes a file twice: a file the validator rejected is already closed.

use super::support::*;
use super::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

const CLOSING_START: &str = "],\"sentAt\":";

fn closings(path: &Path) -> Result<usize> {
	Ok(fs::read_to_string(path)?.matches(CLOSING_START).count())
}

fn reject_all(store: &mut DirectoryStore) {
	store.set_file_validator(|_| Err(io::Error::other("rejected")));
}

/// The one file on disk, asserting there is exactly one.
fn only_file(dir: &TempDir) -> Result<PathBuf> {
	let files = files_on_disk(dir)?;
	assert_eq!(files.len(), 1, "{files:?}");
	Ok(files[0].clone())
}

fn is_finished(path: &Path) -> bool {
	path.extension().and_then(|e| e.to_str()) == Some(DirectoryStore::TEMP_EXTENSION)
}

/// Writes `items` into one file whose validator rejects it, leaving it closed but not
/// renamed. Returns the file.
fn rejected_file(dir: &TempDir, max_file_size: usize, items: &[Value]) -> Result<PathBuf> {
	let mut store = store(dir, max_file_size)?;
	reject_all(&mut store);
	for item in items {
		store.append(item.clone())?;
	}
	let err = store.fetch(None, None).unwrap_err();
	assert_eq!(err.to_string(), "rejected");
	let path = only_file(dir)?;
	assert!(!is_finished(&path));
	assert_eq!(closings(&path)?, 1);
	Ok(path)
}

fn tagged(range: std::ops::Range<usize>) -> Vec<Value> {
	range.map(|i| json!({"i": i, "brace": "}"})).collect()
}

#[test]
fn test_rejected_file_is_finalized_once_on_restart() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let items = tagged(0..3);
	rejected_file(&temp_dir, 1024, &items)?;

	let mut store = store(&temp_dir, 1024)?;
	let path = only_file(&temp_dir)?;
	assert!(is_finished(&path));
	assert_eq!(closings(&path)?, 1);
	let files = finished_files(&mut store)?;
	assert_eq!(files.len(), 1);
	assert_eq!(assert_ceiling(&files, 1024), items);
	Ok(())
}

#[test]
fn test_repeated_restarts_never_add_a_second_closing() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let items = tagged(0..2);
	rejected_file(&temp_dir, 1024, &items)?;

	for _ in 0..4 {
		let store = store(&temp_dir, 1024)?;
		drop(store);
		let path = only_file(&temp_dir)?;
		assert!(is_finished(&path));
		assert_eq!(closings(&path)?, 1);
	}
	let mut store = store(&temp_dir, 1024)?;
	assert_eq!(assert_ceiling(&finished_files(&mut store)?, 1024), items);
	Ok(())
}

#[test]
fn test_recovery_with_rejecting_validator_never_adds_a_second_closing() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let items = tagged(0..2);
	let path = rejected_file(&temp_dir, 1024, &items)?;

	// `new` recovers before a validator can be set, so run recovery directly with one,
	// as a startup that validates would.
	let mut store = store(&temp_dir, 1024)?;
	// `new` already recovered the file; put it back to its rejected state.
	fs::rename(only_file(&temp_dir)?, &path)?;
	let validated = Arc::new(AtomicUsize::new(0));
	let accept = Arc::new(AtomicBool::new(false));
	let (count, flag) = (validated.clone(), accept.clone());
	store.set_file_validator(move |path| {
		count.fetch_add(1, Ordering::SeqCst);
		serde_json::from_str::<Value>(&fs::read_to_string(path)?)?;
		if flag.load(Ordering::SeqCst) {
			Ok(())
		} else {
			Err(io::Error::other("rejected"))
		}
	});

	for attempt in 1..=3 {
		store.initialize_directory()?;
		assert_eq!(validated.load(Ordering::SeqCst), attempt);
		assert!(!is_finished(&only_file(&temp_dir)?));
		assert_eq!(closings(&path)?, 1);
	}

	accept.store(true, Ordering::SeqCst);
	store.initialize_directory()?;
	assert_eq!(validated.load(Ordering::SeqCst), 4);
	let finished = only_file(&temp_dir)?;
	assert!(is_finished(&finished));
	assert_eq!(closings(&finished)?, 1);
	assert_eq!(assert_ceiling(&finished_files(&mut store)?, 1024), items);
	Ok(())
}

#[test]
fn test_unfinished_file_still_gets_its_closing() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let items = tagged(0..3);
	{
		let mut store = store(&temp_dir, 1024)?;
		for item in &items {
			store.append(item.clone())?;
		}
		// Dropped with the file open and unclosed.
	}
	let path = only_file(&temp_dir)?;
	assert_eq!(closings(&path)?, 0);
	// Its last byte is `}`, the same as a closed file's.
	assert!(fs::read_to_string(&path)?.ends_with('}'));

	let mut store = store(&temp_dir, 1024)?;
	let finished = only_file(&temp_dir)?;
	assert!(is_finished(&finished));
	assert_eq!(closings(&finished)?, 1);
	assert_eq!(assert_ceiling(&finished_files(&mut store)?, 1024), items);
	Ok(())
}

#[test]
fn test_recovered_rejected_file_respects_max_file_size() -> Result<()> {
	let temp_dir = TempDir::new()?;
	// Two items that fill a 200 byte file exactly once closed.
	let items = vec![item_of_len(60), item_of_len(67)];
	let path = rejected_file(&temp_dir, 200, &items)?;
	assert_eq!(fs::metadata(&path)?.len(), 200);

	let mut store = store(&temp_dir, 200)?;
	let files = finished_files(&mut store)?;
	assert_eq!(files.len(), 1);
	assert_eq!(files[0].len, 200);
	assert_eq!(assert_ceiling(&files, 200), items);
	Ok(())
}

#[test]
fn test_appends_and_fetches_after_a_rejection_behave_normally() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 1024)?;
	let reject = Arc::new(AtomicBool::new(true));
	let flag = reject.clone();
	store.set_file_validator(move |_| {
		if flag.swap(false, Ordering::SeqCst) {
			return Err(io::Error::other("rejected once"));
		}
		Ok(())
	});

	let first = tagged(0..2);
	for item in &first {
		store.append(item.clone())?;
	}
	assert_eq!(
		store.fetch(None, None).unwrap_err().to_string(),
		"rejected once"
	);
	// The rejected file is not handed out, but its data still counts.
	assert!(store.fetch(None, None)?.is_none());
	assert!(store.has_data());

	let second = tagged(2..5);
	for item in &second {
		store.append(item.clone())?;
	}
	let files = finished_files(&mut store)?;
	assert_eq!(files.len(), 1);
	assert_eq!(assert_ceiling(&files, 1024), second);
	assert_eq!(files_on_disk(&temp_dir)?.len(), 2);
	drop(store);

	// The next session recovers the rejected file, once, ahead of the newer one.
	let mut store = DirectoryStore::new(config(&temp_dir, 1024, KEY))?;
	let files = finished_files(&mut store)?;
	assert_eq!(files.len(), 2);
	assert_eq!(assert_ceiling(&files, 1024), [first, second].concat());
	for path in files_on_disk(&temp_dir)? {
		assert_eq!(closings(&path)?, 1);
	}
	Ok(())
}
