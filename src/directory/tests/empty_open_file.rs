//! An open file can hold no item: a failed write of a file's first item is rolled back to
//! the bare opening, and the store keeps the file open. Finishing such a file, from
//! `end_batch`, `fetch` or rotation, discards it instead of producing an empty batch.

use super::support::*;
use super::*;
use crate::directory::data_file::Faults;

fn faults(store: &DirectoryStore, set: impl FnOnce(&mut Faults)) {
	set(&mut store.faults.lock().unwrap());
}

fn heal(store: &DirectoryStore) {
	faults(store, |f| *f = Faults::default());
}

/// A store whose open file holds the opening and no item, after a failed first write.
/// Returns the store and the path of that file.
fn with_empty_open_file(
	dir: &TempDir,
	config: DirectoryConfig,
) -> Result<(DirectoryStore, std::path::PathBuf)> {
	let mut store = DirectoryStore::new(config)?;
	// The opening reaches the file; the item does not.
	faults(&store, |f| f.write_budget = Some(OPENING + 3));
	assert!(store.append(json!("lost")).is_err());
	heal(&store);
	assert!(store.writer.is_some(), "the rolled back file stays open");
	assert_eq!(store.current_items, 0);
	let path = store.current_path.clone().unwrap();
	assert_eq!(fs::read(&path)?, unclosed(&[]));
	assert_eq!(files_on_disk(dir)?, vec![path.clone()]);
	Ok((store, path))
}

#[test]
fn test_end_batch_discards_an_open_file_with_no_item() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let (mut store, path) =
		with_empty_open_file(&temp_dir, config(&temp_dir, 1024, with_source(KEY)))?;
	store.end_batch()?;
	assert!(store.writer.is_none());
	assert!(store.current_path.is_none());
	assert_eq!((store.current_size, store.current_items), (0, 0));
	assert!(!path.exists() && !finished(&path).exists());
	assert!(files_on_disk(&temp_dir)?.is_empty());
	assert!(store.leftovers.is_empty());
	assert!(!store.has_data());
	// Nothing to fetch, and nothing appears later either.
	assert!(store.fetch(None, None)?.is_none());

	store.append(json!("kept"))?;
	let files = finished_files(&mut store)?;
	assert_eq!(batch_counts(&files), vec![1]);
	assert_eq!(assert_ceiling(&files, 1024), vec![json!("kept")]);
	Ok(())
}

#[test]
fn test_fetch_discards_an_open_file_with_no_item() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let (mut store, path) =
		with_empty_open_file(&temp_dir, config(&temp_dir, 1024, with_source(KEY)))?;
	assert!(store.fetch(None, None)?.is_none());
	assert!(!path.exists() && !finished(&path).exists());
	assert!(files_on_disk(&temp_dir)?.is_empty());
	assert!(!store.has_data());

	store.append(json!("a"))?;
	store.append(json!("b"))?;
	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 1024), vec![json!("a"), json!("b")]);
	Ok(())
}

#[test]
fn test_finishing_an_open_file_with_no_item_discards_it() -> Result<()> {
	// Rotation finishes through the same `finish_file`; call it directly on the empty
	// file, since no append rotates a file that holds no item (any item that passes
	// `append`'s size check fits after the bare opening).
	let temp_dir = TempDir::new()?;
	let (mut store, path) =
		with_empty_open_file(&temp_dir, config(&temp_dir, 1024, with_source(KEY)))?;
	store.finish_file()?;
	assert!(!path.exists() && !finished(&path).exists());
	assert!(files_on_disk(&temp_dir)?.is_empty());
	assert!(store.writer.is_none());
	Ok(())
}

#[test]
fn test_rotation_after_a_failed_first_write_makes_no_empty_batch() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut config = config(&temp_dir, 1024, with_source(KEY));
	config.max_file_items = Some(1);
	let (mut store, path) = with_empty_open_file(&temp_dir, config)?;
	// The empty file takes the next item; the one after rotates it.
	store.append(json!("a"))?;
	assert_eq!(store.current_path.as_ref(), Some(&path));
	store.append(json!("b"))?;
	store.end_batch()?;
	let files = finished_files(&mut store)?;
	assert_eq!(batch_counts(&files), vec![1, 1]);
	assert_eq!(assert_ceiling(&files, 1024), vec![json!("a"), json!("b")]);
	Ok(())
}

#[test]
fn test_failure_deleting_the_empty_file_leaves_it_for_recovery() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let (mut store, path) =
		with_empty_open_file(&temp_dir, config(&temp_dir, 1024, with_source(KEY)))?;
	faults(&store, |f| f.fail_remove = true);
	// Not an error: there was nothing in it to lose.
	store.end_batch()?;
	heal(&store);
	assert!(store.writer.is_none());
	assert!(path.exists() && !finished(&path).exists());
	assert_eq!(store.leftovers, vec![path.clone()]);

	// The next append starts a new file; the fetch deletes the empty one.
	store.append(json!("a"))?;
	assert_ne!(store.current_path.as_ref(), Some(&path));
	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 1024), vec![json!("a")]);
	assert!(!path.exists());
	assert!(store.leftovers.is_empty());
	Ok(())
}
