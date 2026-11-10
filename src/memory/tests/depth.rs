//! `append` rejects an item nested too deeply for `serde_json` to read back from a
//! fetched batch.

use super::support::*;
use super::*;
use crate::batch::{nested, Shape, MAX_ITEM_DEPTH, SHAPES};

fn deep(depth: usize, shape: Shape) -> Value {
	nested(depth, shape, json!(0))
}

fn assert_too_deep(err: &std::io::Error) {
	assert_invalid_input(err);
	assert!(
		err.to_string()
			.contains(&format!("{MAX_ITEM_DEPTH} levels")),
		"{err}"
	);
}

/// The items of one fetched batch, read back from its serialized bytes.
fn read_back(store: &mut MemoryStore) -> Result<Vec<Value>> {
	let result = store.fetch(None, None)?.unwrap();
	let bytes = serde_json::to_vec(result.data.as_ref().unwrap())?;
	let batch: Value = serde_json::from_slice(&bytes)?;
	Ok(batch["items"].as_array().unwrap().clone())
}

#[test]
fn test_deepest_accepted_item_reads_back_and_one_deeper_is_rejected() -> Result<()> {
	for shape in SHAPES {
		let mut store = store(1024 * 1024);
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
		assert_eq!(read_back(&mut store)?, accepted, "{shape:?}");
	}
	Ok(())
}

#[test]
fn test_boundary_items_of_every_shape_are_fetchable() -> Result<()> {
	let mut store = store(1024 * 1024);
	let mut items = Vec::new();
	for shape in SHAPES {
		for inner in [json!(0), json!([]), json!({})] {
			items.push(nested(MAX_ITEM_DEPTH, shape, inner));
		}
	}
	for item in &items {
		store.append(item.clone())?;
	}
	assert_eq!(read_back(&mut store)?, items);
	Ok(())
}

#[test]
fn test_rejection_leaves_the_store_unchanged() -> Result<()> {
	let mut store = store(1024);
	assert_too_deep(
		&store
			.append(deep(MAX_ITEM_DEPTH + 1, Shape::Objects))
			.unwrap_err(),
	);
	assert!(!store.has_data());

	store.append(json!("a"))?;
	let (before, next_seq) = (store.items.clone(), store.next_seq);
	for shape in SHAPES {
		assert_too_deep(&store.append(deep(MAX_ITEM_DEPTH + 1, shape)).unwrap_err());
	}
	assert_eq!(store.items, before);
	assert_eq!(store.next_seq, next_seq);

	store.append(json!("b"))?;
	assert_eq!(
		drain(&mut store, None, None)?.concat(),
		vec![json!("a"), json!("b")]
	);
	Ok(())
}

#[test]
fn test_very_deep_item_is_rejected_without_overflowing_the_stack() -> Result<()> {
	let mut store = store(1024);
	for shape in SHAPES {
		assert_too_deep(&store.append(deep(100_000, shape)).unwrap_err());
	}
	assert!(!store.has_data());
	store.append(json!("after"))?;
	assert_eq!(read_back(&mut store)?, vec![json!("after")]);
	Ok(())
}
