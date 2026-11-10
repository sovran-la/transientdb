//! Items too large for any file are rejected without disturbing the store.

use super::support::*;
use super::*;

#[test]
fn test_oversized_item_into_empty_store() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 100)?;
	let err = store.append(item_of_len(29)).unwrap_err();
	assert_invalid_input(&err);

	assert!(files_on_disk(&temp_dir)?.is_empty());
	assert!(!store.has_data());
	assert!(store.writer.is_none());
	assert!(store.fetch(None, None)?.is_none());
	assert!(files_on_disk(&temp_dir)?.is_empty());
	Ok(())
}

#[test]
fn test_oversized_item_leaves_open_file_untouched() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 200)?;
	let (a, b, c) = (json!({"n": "a"}), json!({"n": "b"}), json!({"n": "c"}));
	store.append(a.clone())?;
	store.append(b.clone())?;

	let path = store.current_path.clone().unwrap();
	let (size, items) = (store.current_size, store.current_items);
	let bytes = fs::read(&path)?;

	let err = store.append(item_of_len(129)).unwrap_err();
	assert_invalid_input(&err);
	assert_eq!(store.current_path.as_ref(), Some(&path));
	assert_eq!((store.current_size, store.current_items), (size, items));
	assert!(store.writer.is_some());
	assert_eq!(fs::read(&path)?, bytes);
	assert_eq!(files_on_disk(&temp_dir)?, vec![path.clone()]);

	store.append(c.clone())?;
	assert_eq!(store.current_path.as_ref(), Some(&path));

	let files = finished_files(&mut store)?;
	assert_eq!(files.len(), 1);
	assert_eq!(assert_ceiling(&files, 200), vec![a, b, c]);
	Ok(())
}

#[test]
fn test_largest_item_accepted_one_byte_more_rejected() -> Result<()> {
	for max in [100usize, 101, 512, 4096] {
		let temp_dir = TempDir::new()?;
		let mut store = store(&temp_dir, max)?;
		let largest = max - ENVELOPE;
		assert_eq!(store.max_item_size(), largest);

		assert_invalid_input(&store.append(item_of_len(largest + 1)).unwrap_err());
		store.append(item_of_len(largest))?;
		assert_invalid_input(&store.append(item_of_len(largest + 1)).unwrap_err());
		// A full-size item after a small one rotates into its own file.
		store.append(item_of_len(10))?;
		store.append(item_of_len(largest))?;

		let files = finished_files(&mut store)?;
		assert_eq!(batch_counts(&files), vec![1, 1, 1], "max={max}");
		assert_eq!(files[0].len, max as u64);
		assert_eq!(files[2].len, max as u64);
		assert_ceiling(&files, max);
	}
	Ok(())
}

#[test]
fn test_rejections_mixed_with_accepted_items() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 150)?;
	let mut accepted = Vec::new();
	let mut rejected = 0;
	for i in 0..60 {
		if i % 3 == 1 {
			let item = json!({"rejected": "r".repeat(70 + i)});
			assert_invalid_input(&store.append(item).unwrap_err());
			rejected += 1;
		} else {
			let item = json!({"i": i, "pad": "p".repeat(i % 30)});
			store.append(item.clone())?;
			accepted.push(item);
		}
	}
	assert_eq!(rejected, 20);

	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 150), accepted);
	for path in store.sorted_files(true)? {
		assert!(!fs::read_to_string(path)?.contains("rejected"));
	}
	Ok(())
}

#[test]
fn test_oversized_error_message_states_sizes() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 100)?;
	let err = store.append(item_of_len(1234)).unwrap_err();
	let message = err.to_string();
	assert!(message.contains("1234 bytes"), "{message}");
	assert!(message.contains("28 bytes"), "{message}");
	assert!(message.contains("100 bytes"), "{message}");
	Ok(())
}

#[test]
fn test_reset_and_fetch_after_rejection() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 100)?;
	store.append(json!("first"))?;
	assert_invalid_input(&store.append(item_of_len(500)).unwrap_err());

	store.reset();
	assert!(!store.has_data());
	assert!(files_on_disk(&temp_dir)?.is_empty());
	assert_invalid_input(&store.append(item_of_len(500)).unwrap_err());
	assert!(store.fetch(None, None)?.is_none());

	store.append(json!("second"))?;
	assert_invalid_input(&store.append(item_of_len(500)).unwrap_err());
	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 100), vec![json!("second")]);
	assert_eq!(files_on_disk(&temp_dir)?.len(), 1);
	Ok(())
}
