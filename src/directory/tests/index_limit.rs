//! File indexes never wrap. Once `u32::MAX` is taken, an append that needs a new file
//! fails cleanly; no existing file is ever reused, overwritten or appended to, and the
//! files already there can still be fetched.

use super::support::*;
use super::*;
use std::path::PathBuf;

const NO_INDEX: &str = "No file index left";

fn path(dir: &TempDir, index: u32, finished: bool) -> PathBuf {
	let name = format!("{index}-events{}", if finished { ".temp" } else { "" });
	dir.path().join(name)
}

fn one_per_file(dir: &TempDir) -> Result<DirectoryStore> {
	let mut config = config(dir, 1024, KEY);
	config.max_file_items = Some(1);
	DirectoryStore::new(config)
}

fn assert_no_index(err: io::Error) {
	assert!(err.to_string().starts_with(NO_INDEX), "{err}");
}

#[test]
fn test_a_finished_file_at_u32_max_stops_new_files_and_stays_intact() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let last = path(&temp_dir, u32::MAX, true);
	fs::write(&last, closed(&[json!("last")]))?;
	let before = snapshot(temp_dir.path())?;

	let mut store = store(&temp_dir, 1024)?;
	assert_eq!(store.next_index, None);
	for _ in 0..3 {
		assert_no_index(store.append(json!("new")).unwrap_err());
		assert!(store.writer.is_none());
		assert_eq!(snapshot(temp_dir.path())?, before);
	}

	// What is there is still delivered.
	assert!(store.has_data());
	let result = store.fetch(None, None)?.unwrap();
	assert_eq!(result.data.as_ref().unwrap(), &vec![last.clone()]);
	assert_eq!(fs::read(&last)?, closed(&[json!("last")]));
	store.remove(&result.removable.unwrap())?;
	assert!(!store.has_data());
	// Still none in this session: the index is only rescanned by `new`.
	assert_no_index(store.append(json!("new")).unwrap_err());
	assert!(files_on_disk(&temp_dir)?.is_empty());
	drop(store);

	// Once delivered and removed, a new session starts over.
	let mut store = super::support::store(&temp_dir, 1024)?;
	store.append(json!("again"))?;
	assert_eq!(store.current_path, Some(path(&temp_dir, 1, false)));
	Ok(())
}

#[test]
fn test_an_unfinished_file_at_u32_max_is_never_appended_to() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let last = path(&temp_dir, u32::MAX, false);
	let bytes = unclosed(&[json!("left over")]);
	fs::write(&last, &bytes)?;

	let mut store = store(&temp_dir, 1024)?;
	assert_no_index(store.append(json!("new")).unwrap_err());
	assert_eq!(fs::read(&last)?, bytes);
	assert_eq!(files_on_disk(&temp_dir)?, vec![last.clone()]);

	// Recovery still finishes it, with only its own item.
	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 1024), vec![json!("left over")]);
	assert_eq!(files_on_disk(&temp_dir)?, vec![finished(&last)]);
	Ok(())
}

#[test]
fn test_u32_max_minus_one_allows_exactly_one_more_file() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let below = path(&temp_dir, u32::MAX - 1, true);
	fs::write(&below, closed(&[json!("below")]))?;

	let mut store = one_per_file(&temp_dir)?;
	assert_eq!(store.next_index, Some(u32::MAX));
	store.append(json!("a"))?;
	let last = path(&temp_dir, u32::MAX, false);
	assert_eq!(store.current_path, Some(last.clone()));
	assert_eq!(store.next_index, None);

	// The next append rotates: the last file is finished, then no new one can start.
	assert_no_index(store.append(json!("b")).unwrap_err());
	assert!(store.writer.is_none());
	assert!(store.leftovers.is_empty());
	let mut expected = vec![below.clone(), finished(&last)];
	expected.sort();
	assert_eq!(files_on_disk(&temp_dir)?, expected);
	assert_eq!(fs::read(&below)?, closed(&[json!("below")]));
	let before = snapshot(temp_dir.path())?;
	assert_no_index(store.append(json!("c")).unwrap_err());
	assert_eq!(snapshot(temp_dir.path())?, before);

	let files = finished_files(&mut store)?;
	assert_eq!(
		assert_ceiling(&files, 1024),
		vec![json!("below"), json!("a")]
	);
	Ok(())
}

#[test]
fn test_taken_indexes_up_to_u32_max_fail_without_touching_them() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 1024)?;
	store.append(json!("seed"))?;
	assert_eq!(finished_files(&mut store)?.len(), 1);
	// Another writer takes the indexes from here on, up to the last.
	store.next_index = Some(u32::MAX - 2);
	for index in [u32::MAX - 2, u32::MAX - 1, u32::MAX] {
		fs::write(path(&temp_dir, index, false), format!("taken {index}"))?;
	}
	let before = snapshot(temp_dir.path())?;
	assert_no_index(store.append(json!("new")).unwrap_err());
	assert_eq!(snapshot(temp_dir.path())?, before);
	assert_eq!(store.next_index, None);
	Ok(())
}

#[test]
fn test_take_index_counts_up_and_stops_at_u32_max() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 1024)?;
	assert_eq!(store.take_index()?, 1);
	assert_eq!(store.take_index()?, 2);
	store.next_index = Some(u32::MAX - 1);
	assert_eq!(store.take_index()?, u32::MAX - 1);
	assert_eq!(store.take_index()?, u32::MAX);
	for _ in 0..3 {
		assert_no_index(store.take_index().unwrap_err());
	}
	Ok(())
}

#[test]
fn test_index_progression_across_sessions_is_unchanged() -> Result<()> {
	let temp_dir = TempDir::new()?;
	// No files: the first index is 1.
	let mut store = one_per_file(&temp_dir)?;
	assert_eq!(store.next_index, Some(1));
	for item in ["a", "b", "c"] {
		store.append(json!(item))?;
	}
	assert_eq!(store.current_path, Some(path(&temp_dir, 3, false)));
	drop(store);

	// Highest index plus one, counting finished and unfinished files.
	let mut store = one_per_file(&temp_dir)?;
	assert_eq!(store.next_index, Some(4));
	store.append(json!("d"))?;
	assert_eq!(store.current_path, Some(path(&temp_dir, 4, false)));
	let files = finished_files(&mut store)?;
	assert_eq!(
		assert_ceiling(&files, 1024),
		vec![json!("a"), json!("b"), json!("c"), json!("d")]
	);
	drop(store);

	let mut store = one_per_file(&temp_dir)?;
	assert_eq!(store.next_index, Some(5));
	store.append(json!("e"))?;
	assert_eq!(store.current_path, Some(path(&temp_dir, 5, false)));

	// A file named index 0 is this store's too, and does not change the next index.
	let temp_dir = TempDir::new()?;
	fs::write(path(&temp_dir, 0, true), closed(&[json!("zero")]))?;
	let store = one_per_file(&temp_dir)?;
	assert_eq!(store.next_index, Some(1));
	Ok(())
}
