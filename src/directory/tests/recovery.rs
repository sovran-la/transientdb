//! Recovery finishes every file an earlier session left without the `.temp` extension,
//! and never closes a file twice: a session can end after closing a file and before
//! renaming it.

use super::support::*;
use super::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

const CLOSING_START: &str = "],\"sentAt\":";

fn closings(path: &Path) -> Result<usize> {
	Ok(fs::read_to_string(path)?.matches(CLOSING_START).count())
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

/// Writes `items` as file `index`, closed but not renamed, as a session that ended
/// between the two leaves it. Returns the file.
fn closed_file(dir: &TempDir, index: u32, items: &[Value]) -> Result<PathBuf> {
	let path = write_leftover(dir, index, &closed(items))?;
	assert!(!is_finished(&path));
	assert_eq!(closings(&path)?, 1);
	Ok(path)
}

fn tagged(range: std::ops::Range<usize>) -> Vec<Value> {
	range.map(|i| json!({"i": i, "brace": "}"})).collect()
}

#[test]
fn test_closed_file_is_finalized_once_on_restart() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let items = tagged(0..3);
	closed_file(&temp_dir, 1, &items)?;

	let mut store = store(&temp_dir, 1024)?;
	let files = finished_files(&mut store)?;
	let path = only_file(&temp_dir)?;
	assert!(is_finished(&path));
	assert_eq!(closings(&path)?, 1);
	assert_eq!(files.len(), 1);
	assert_eq!(assert_ceiling(&files, 1024), items);
	Ok(())
}

#[test]
fn test_repeated_restarts_never_add_a_second_closing() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let items = tagged(0..2);
	closed_file(&temp_dir, 1, &items)?;

	// Sessions that never fetch leave the file as it was.
	for _ in 0..4 {
		drop(store(&temp_dir, 1024)?);
		let path = only_file(&temp_dir)?;
		assert!(!is_finished(&path));
		assert_eq!(closings(&path)?, 1);
	}
	// Sessions that fetch recover it once and never close it again.
	for _ in 0..4 {
		let mut store = store(&temp_dir, 1024)?;
		assert_eq!(assert_ceiling(&finished_files(&mut store)?, 1024), items);
		let path = only_file(&temp_dir)?;
		assert!(is_finished(&path));
		assert_eq!(closings(&path)?, 1);
	}
	Ok(())
}

#[test]
fn test_recovery_with_a_validator_never_adds_a_second_closing() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let (kept, dropped) = (tagged(0..2), tagged(2..4));
	let accepted = closed_file(&temp_dir, 1, &kept)?;
	let rejected = closed_file(&temp_dir, 2, &dropped)?;

	let validated = Arc::new(AtomicUsize::new(0));
	let session = |validated: &Arc<AtomicUsize>| {
		let mut store = store(&temp_dir, 1024)?;
		let (count, reject) = (validated.clone(), rejected.clone());
		// Set after `new`, as a caller would; recovery waits for the first fetch.
		store.set_file_validator(move |path| {
			count.fetch_add(1, Ordering::SeqCst);
			// The validator sees the file with exactly its one closing.
			assert_eq!(closings(path)?, 1);
			serde_json::from_str::<Value>(&fs::read_to_string(path)?)?;
			if path == reject {
				Err(io::Error::other("rejected"))
			} else {
				Ok(())
			}
		});
		Ok::<_, io::Error>(store)
	};

	let mut store = session(&validated)?;
	assert_eq!(validated.load(Ordering::SeqCst), 0);
	let files = finished_files(&mut store)?;
	assert_eq!(validated.load(Ordering::SeqCst), 2);
	assert_eq!(assert_ceiling(&files, 1024), kept);
	// The rejected file is deleted; the accepted one is finished with one closing.
	let finished = only_file(&temp_dir)?;
	assert_eq!(finished, DirectoryStore::finished_path(&accepted));
	assert_eq!(closings(&finished)?, 1);
	drop(store);

	// Later sessions have nothing to recover and never close the file again.
	for _ in 0..3 {
		let mut store = session(&validated)?;
		assert_eq!(assert_ceiling(&finished_files(&mut store)?, 1024), kept);
		assert_eq!(validated.load(Ordering::SeqCst), 2);
		assert_eq!(closings(&only_file(&temp_dir)?)?, 1);
	}
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
	let files = finished_files(&mut store)?;
	let finished = only_file(&temp_dir)?;
	assert!(is_finished(&finished));
	assert_eq!(closings(&finished)?, 1);
	assert_eq!(assert_ceiling(&files, 1024), items);
	Ok(())
}

#[test]
fn test_recovered_closed_file_respects_max_file_size() -> Result<()> {
	let temp_dir = TempDir::new()?;
	// Two items that fill a 200 byte file exactly once closed.
	let items = vec![item_of_len(60), item_of_len(67)];
	let path = closed_file(&temp_dir, 1, &items)?;
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
	// The rejection is not an error of the fetch: the file and its items are gone.
	assert!(store.fetch(None, None)?.is_none());
	assert!(files_on_disk(&temp_dir)?.is_empty());
	assert!(!store.has_data());
	assert!(store.fetch(None, None)?.is_none());

	let second = tagged(2..5);
	for item in &second {
		store.append(item.clone())?;
	}
	let files = finished_files(&mut store)?;
	assert_eq!(files.len(), 1);
	assert_eq!(assert_ceiling(&files, 1024), second);
	assert_eq!(files_on_disk(&temp_dir)?.len(), 1);
	drop(store);

	// The next session has nothing to recover; the accepted file is closed once.
	let mut store = DirectoryStore::new(config(&temp_dir, 1024, KEY))?;
	assert!(store.leftovers.is_empty());
	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 1024), second);
	for path in files_on_disk(&temp_dir)? {
		assert_eq!(closings(&path)?, 1);
	}
	Ok(())
}
