//! `append` rejects an item nested too deeply for `serde_json` to read back from a
//! finished file, and every item it accepts survives a finished file, recovery and
//! salvage.

use super::support::*;
use super::*;
use crate::batch::{nested, Shape, MAX_ITEM_DEPTH, SHAPES};

fn deep(depth: usize, shape: Shape) -> Value {
	nested(depth, shape, json!(0))
}

/// The items in the store's one finished file, parsed with `serde_json::from_slice`.
fn fetch_one_file(store: &mut DirectoryStore) -> Result<Vec<Value>> {
	let files = store.fetch(None, None)?.unwrap().data.unwrap();
	assert_eq!(files.len(), 1);
	let json: Value = serde_json::from_slice(&fs::read(&files[0])?)?;
	assert_eq!(json["source"], KEY);
	Ok(json["items"].as_array().unwrap().clone())
}

fn assert_too_deep(err: &io::Error) {
	assert_invalid_input(err);
	assert!(
		err.to_string()
			.contains(&format!("{MAX_ITEM_DEPTH} levels")),
		"{err}"
	);
}

#[test]
fn test_deepest_accepted_item_reads_back_and_one_deeper_is_rejected() -> Result<()> {
	for shape in SHAPES {
		let temp_dir = TempDir::new()?;
		let mut store = store(&temp_dir, 1024 * 1024)?;
		// Every depth until the first rejection is accepted.
		let mut accepted = Vec::new();
		let rejected_at = (1..1000)
			.find(|&depth| match store.append(deep(depth, shape)) {
				Ok(()) => {
					accepted.push(deep(depth, shape));
					false
				}
				Err(e) => {
					assert_too_deep(&e);
					true
				}
			})
			.unwrap();
		let deepest = rejected_at - 1;
		assert_eq!(deepest, MAX_ITEM_DEPTH, "{shape:?}");
		assert_too_deep(&store.append(deep(deepest + 1, shape)).unwrap_err());
		// Every accepted item, the deepest included, round-trips through the file.
		assert_eq!(fetch_one_file(&mut store)?, accepted, "{shape:?}");
	}
	Ok(())
}

#[test]
fn test_rejection_leaves_the_store_unchanged() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 4096)?;
	assert_too_deep(
		&store
			.append(deep(MAX_ITEM_DEPTH + 1, Shape::Arrays))
			.unwrap_err(),
	);
	assert!(!store.has_data());
	assert!(files_on_disk(&temp_dir)?.is_empty());

	store.append(json!("a"))?;
	let before = snapshot(temp_dir.path())?;
	let (size, items) = (store.current_size, store.current_items);
	for shape in SHAPES {
		assert_too_deep(&store.append(deep(MAX_ITEM_DEPTH + 1, shape)).unwrap_err());
	}
	assert_eq!(snapshot(temp_dir.path())?, before);
	assert_eq!((store.current_size, store.current_items), (size, items));

	store.append(json!("b"))?;
	assert_eq!(fetch_one_file(&mut store)?, vec![json!("a"), json!("b")]);
	Ok(())
}

#[test]
fn test_very_deep_item_is_rejected_without_overflowing_the_stack() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 1024)?;
	for shape in SHAPES {
		assert_too_deep(&store.append(deep(100_000, shape)).unwrap_err());
	}
	assert!(!store.has_data());
	store.append(json!("after"))?;
	assert_eq!(fetch_one_file(&mut store)?, vec![json!("after")]);
	Ok(())
}

fn boundary_items() -> Vec<Value> {
	SHAPES
		.iter()
		.flat_map(|&shape| {
			[
				deep(MAX_ITEM_DEPTH, shape),
				nested(MAX_ITEM_DEPTH, shape, json!([])),
				nested(MAX_ITEM_DEPTH, shape, json!({})),
			]
		})
		.collect()
}

#[test]
fn test_boundary_items_survive_recovery_of_an_unclosed_file() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let items = boundary_items();
	{
		let mut store = store(&temp_dir, 1024 * 1024)?;
		for item in &items {
			store.append(item.clone())?;
		}
		// Dropped with the file open and unclosed.
	}
	let mut store = store(&temp_dir, 1024 * 1024)?;
	assert_eq!(fetch_one_file(&mut store)?, items);
	Ok(())
}

#[test]
fn test_boundary_items_survive_recovery_of_a_closed_file() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let items = boundary_items();
	let bytes = closed(&items);
	write_leftover(&temp_dir, 1, &bytes)?;
	let mut store = store(&temp_dir, 1024 * 1024)?;
	let files = store.fetch(None, None)?.unwrap().data.unwrap();
	// A complete envelope: renamed untouched.
	assert_eq!(fs::read(&files[0])?, bytes);
	assert_eq!(fetch_one_file(&mut store)?, items);
	Ok(())
}

#[test]
fn test_boundary_items_survive_salvage() -> Result<()> {
	let items = boundary_items();
	let tails: [&[u8]; 4] = [b"", b",", b",[[[[[[", b",{\"k\":[{\"k\""];
	for tail in tails {
		let temp_dir = TempDir::new()?;
		write_leftover(&temp_dir, 1, &[unclosed(&items), tail.to_vec()].concat())?;
		let mut store = store(&temp_dir, 1024 * 1024)?;
		assert_eq!(fetch_one_file(&mut store)?, items, "{tail:?}");
	}
	// Cut down to a smaller ceiling, the leading boundary items are kept intact.
	let temp_dir = TempDir::new()?;
	write_leftover(&temp_dir, 1, &closed(&items))?;
	let first_two = unclosed(&items[..2]).len() + CLOSING;
	let mut store = store(&temp_dir, first_two)?;
	assert_eq!(fetch_one_file(&mut store)?, items[..2].to_vec());
	Ok(())
}
