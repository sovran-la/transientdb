//! Files with identical content are still distinct files: `remove` deletes exactly the
//! files a fetch returned.

use super::support::*;
use super::*;

/// A store that puts each item in its own file, holding `count` copies of `item`.
fn one_item_per_file(dir: &TempDir, item: &Value, count: usize) -> Result<DirectoryStore> {
	let mut store = DirectoryStore::new(item_limit_config(dir, Some(1)))?;
	for _ in 0..count {
		store.append(item.clone())?;
	}
	Ok(store)
}

fn finished_on_disk(store: &DirectoryStore) -> Result<usize> {
	Ok(store.sorted_files(false)?.len())
}

#[test]
fn test_removing_one_fetched_duplicate_keeps_the_rest() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let same = json!({"event": "click"});
	let mut store = one_item_per_file(&temp_dir, &same, 4)?;

	let result = store.fetch(Some(1), None)?.unwrap();
	let files = result.data.unwrap();
	assert_eq!(files.len(), 1);
	assert_eq!(read_finished(&files[0], KEY)?.batch(), &vec![same.clone()]);
	store.remove(&result.removable.unwrap())?;

	assert!(!files[0].exists());
	assert_eq!(finished_on_disk(&store)?, 3);
	let rest = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&rest, 1024 * 1024), vec![same; 3]);
	Ok(())
}

#[test]
fn test_walking_the_store_returns_every_duplicate() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let same = json!("dup");
	let mut store = one_item_per_file(&temp_dir, &same, 5)?;

	let mut seen = Vec::new();
	while let Some(result) = store.fetch(Some(2), None)? {
		for file in result.data.unwrap() {
			seen.extend(read_finished(&file, KEY)?.batch().iter().cloned());
		}
		store.remove(&result.removable.unwrap())?;
	}
	assert_eq!(seen, vec![same; 5]);
	assert!(!store.has_data());
	Ok(())
}

#[test]
fn test_removing_twice_is_a_noop() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let same = json!("dup");
	let mut store = one_item_per_file(&temp_dir, &same, 3)?;

	let result = store.fetch(Some(1), None)?.unwrap();
	let removable = result.removable.unwrap();
	store.remove(&removable)?;
	store.remove(&removable)?;
	assert_eq!(finished_on_disk(&store)?, 2);
	assert_eq!(files_on_disk(&temp_dir)?.len(), 2);
	Ok(())
}
