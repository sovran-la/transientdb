//! `end_batch` together with the other limits on a file: no file crosses a boundary
//! or exceeds a limit, and a long span is split without crossing into the next.

use super::end_batch::*;
use super::support::*;
use super::*;
use crate::batch::script::{self, Step};

#[test]
fn test_boundary_with_the_item_limit() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut config = config(&temp_dir, 4096, with_source(KEY));
	config.max_file_items = Some(2);
	let mut store = DirectoryStore::new(config)?;
	append_tags(&mut store, 0..5)?;
	store.end_batch()?;
	append_tags(&mut store, 5..8)?;
	store.end_batch()?;
	append_tags(&mut store, 8..9)?;
	let files = finished_files(&mut store)?;
	assert_eq!(
		tags_of(&files),
		vec![
			vec![0, 1],
			vec![2, 3],
			vec![4],
			vec![5, 6],
			vec![7],
			vec![8]
		]
	);
	Ok(())
}

#[test]
fn test_boundary_with_the_byte_ceiling() -> Result<()> {
	let max = 200;
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, max)?;
	// Two 60 byte items fit per file (10 + 60 + 1 + 60 + 58 = 189), three do not.
	let items: Vec<Value> = (0..7)
		.map(|n| json!(format!("{n}{}", "x".repeat(57))))
		.collect();
	assert!(items.iter().all(|i| item_len(i) == 60));
	for item in &items[..5] {
		store.append(item.clone())?;
	}
	store.end_batch()?;
	for item in &items[5..] {
		store.append(item.clone())?;
	}
	let files = finished_files(&mut store)?;
	assert_eq!(batch_counts(&files), vec![2, 2, 1, 2]);
	assert_eq!(assert_ceiling(&files, max), items);
	Ok(())
}

#[test]
fn test_scripted_boundaries_match_the_model() -> Result<()> {
	for (seed, per_file) in [(1u32, 3usize), (42, 1), (0xdead_beef, 5), (99, 1000)] {
		let temp_dir = TempDir::new()?;
		let mut config = config(&temp_dir, 64 * 1024, with_source(KEY));
		config.max_file_items = Some(per_file);
		let mut store = DirectoryStore::new(config)?;
		let steps = script::steps(seed, 300);
		for step in &steps {
			match step {
				Step::Append(n) => store.append(tag(*n))?,
				Step::End => store.end_batch()?,
			}
		}
		// Walk one file at a time with fetch and remove.
		let mut batches = Vec::new();
		while let Some(result) = store.fetch(Some(1), None)? {
			let files = result.data.unwrap();
			assert_eq!(files.len(), 1);
			let file = read_finished(&files[0], &with_source(KEY))?;
			batches.extend(tags_of(&[file]));
			store.remove(&result.removable.unwrap())?;
		}
		assert_eq!(batches, script::expected(&steps, per_file), "seed {seed}");
		assert!(!store.has_data());
		assert!(files_on_disk(&temp_dir)?.is_empty());
	}
	Ok(())
}
