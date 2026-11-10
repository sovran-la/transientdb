//! DirectoryStore has no item cap: it never drops an item because of how many it holds.

use super::support::*;
use super::*;

#[test]
fn test_never_drops_for_capacity() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut config = config(&temp_dir, 64 * 1024, with_source(KEY));
	config.max_file_items = Some(50);
	let mut store = DirectoryStore::new(config)?;
	let appended: Vec<Value> = (0..3000).map(|i| json!({"i": i})).collect();
	for item in &appended {
		store.append(item.clone())?;
	}

	// Walk the store a few files at a time: every item comes back once, in order.
	let mut seen = Vec::new();
	let mut file_count = 0;
	while let Some(result) = store.fetch(Some(7), None)? {
		let files = result.data.unwrap();
		assert!(!files.is_empty() && files.len() <= 7);
		file_count += files.len();
		for file in &files {
			seen.extend(
				read_finished(file, &with_source(KEY))?
					.batch()
					.iter()
					.cloned(),
			);
		}
		store.remove(&result.removable.unwrap())?;
	}
	assert_eq!(file_count, 60);
	assert_eq!(seen, appended);
	assert!(!store.has_data());
	assert!(files_on_disk(&temp_dir)?.is_empty());
	Ok(())
}
