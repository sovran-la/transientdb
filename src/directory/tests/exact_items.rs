//! Items come back exactly as they were appended: the file holds each item's serialized
//! bytes, and recovery keeps the bytes a leftover holds, so numbers a JavaScript number
//! cannot hold, and floats such as `42.0`, survive both a fetch and a restart.

use super::support::*;
use super::*;
use crate::item_samples::{assert_exact, awkward_items};

/// The bytes of every fetched file.
fn fetch_bytes(store: &mut DirectoryStore) -> Result<Vec<Vec<u8>>> {
	let files = store
		.fetch(None, None)?
		.map(|r| r.data.unwrap())
		.unwrap_or_default();
	files.iter().map(fs::read).collect()
}

/// Asserts that `file` is one finished file under [`with_source`]`(KEY)` holding exactly
/// `items`: it starts with the opening and each item's `serde_json::to_vec` bytes, comma
/// separated, and its items parse equal to them and serialize the same.
fn assert_holds_exactly(file: &[u8], items: &[Value]) -> Result<()> {
	let expected = unclosed(items);
	assert!(
		file.starts_with(&expected),
		"{}",
		String::from_utf8_lossy(file)
	);
	let json: Value = serde_json::from_slice(file)?;
	let parsed = json["items"].as_array().expect("items array");
	assert_exact(parsed, items);
	Ok(())
}

#[test]
fn test_awkward_items_are_fetched_exactly() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let items = awkward_items();
	let mut store = store(&temp_dir, 4096)?;
	for item in &items {
		store.append(item.clone())?;
	}
	let files = fetch_bytes(&mut store)?;
	assert_eq!(files.len(), 1);
	assert_holds_exactly(&files[0], &items)?;
	// Each item is written as its own bytes: nothing reformats a number.
	for item in &items {
		let bytes = serde_json::to_vec(item)?;
		assert!(
			files[0].windows(bytes.len()).any(|w| w == bytes.as_slice()),
			"{item} not found as written"
		);
	}
	Ok(())
}

#[test]
fn test_awkward_items_in_an_unfinished_leftover_are_recovered_exactly() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let items = awkward_items();
	let path = write_leftover(&temp_dir, 1, &unclosed(&items))?;

	let mut store = store(&temp_dir, 4096)?;
	let files = fetch_bytes(&mut store)?;
	assert_eq!(files.len(), 1);
	assert_holds_exactly(&files[0], &items)?;
	assert!(finished(&path).exists());
	Ok(())
}

#[test]
fn test_awkward_items_in_a_cut_off_leftover_are_salvaged_exactly() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let items = awkward_items();
	// The last item was cut off mid-number as it was written.
	let mut cut_off = unclosed(&items);
	cut_off.extend_from_slice(b",[18446744073709551");
	write_leftover(&temp_dir, 1, &cut_off)?;

	let mut store = store(&temp_dir, 4096)?;
	let files = fetch_bytes(&mut store)?;
	assert_eq!(files.len(), 1);
	assert_holds_exactly(&files[0], &items)?;
	Ok(())
}

#[test]
fn test_awkward_items_survive_a_restart_exactly() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let items = awkward_items();
	{
		let mut store = store(&temp_dir, 4096)?;
		for item in &items {
			store.append(item.clone())?;
		}
	}
	let mut store = store(&temp_dir, 4096)?;
	let files = fetch_bytes(&mut store)?;
	assert_eq!(files.len(), 1);
	assert_holds_exactly(&files[0], &items)?;
	Ok(())
}
