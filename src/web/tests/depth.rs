//! `append` rejects an item nested too deeply for `serde_json` to read back from a
//! fetched batch or from IndexedDB, and every item it accepts survives a reopen.

use super::support::*;
use super::*;
use crate::batch::{nested, Shape, MAX_ITEM_DEPTH, SHAPES};

fn deep(depth: usize, shape: Shape) -> Value {
	nested(depth, shape, json!(0))
}

fn assert_too_deep(err: &Error) {
	assert_invalid_input(err);
	assert!(
		err.to_string()
			.contains(&format!("{MAX_ITEM_DEPTH} levels")),
		"{err}"
	);
}

/// The items of one fetched batch, read back from its serialized bytes.
fn read_back(store: &mut WebStore) -> Vec<Value> {
	let result = store.fetch(None, None).unwrap().unwrap();
	let bytes = serde_json::to_vec(result.data.as_ref().unwrap()).unwrap();
	let batch: Value = serde_json::from_slice(&bytes).unwrap();
	batch["items"].as_array().unwrap().clone()
}

fn boundary_items() -> Vec<Value> {
	SHAPES
		.iter()
		.flat_map(|&shape| {
			[json!(0), json!([]), json!({})].map(|inner| nested(MAX_ITEM_DEPTH, shape, inner))
		})
		.collect()
}

#[wasm_bindgen_test]
async fn test_deepest_accepted_item_reads_back_and_one_deeper_is_rejected() {
	for shape in SHAPES {
		let mut store = fresh_store(&format!("test-depth-boundary-{shape:?}"), 1024 * 1024).await;
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
		assert_eq!(read_back(&mut store), accepted, "{shape:?}");
		store.reset();
		settle().await;
	}
}

#[wasm_bindgen_test]
async fn test_boundary_items_of_every_shape_are_fetchable() {
	let mut store = fresh_store("test-depth-shapes", 1024 * 1024).await;
	let items = boundary_items();
	for item in &items {
		store.append(item.clone()).unwrap();
	}
	assert_eq!(read_back(&mut store), items);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_rejection_leaves_the_store_and_indexeddb_unchanged() {
	let db_name = "test-depth-unchanged";
	let mut store = fresh_store(db_name, 1024).await;
	assert_too_deep(
		&store
			.append(deep(MAX_ITEM_DEPTH + 1, Shape::Mixed))
			.unwrap_err(),
	);
	assert!(!store.has_data());

	store.append(json!("a")).unwrap();
	let next_key = store.next_key;
	for shape in SHAPES {
		assert_too_deep(&store.append(deep(MAX_ITEM_DEPTH + 1, shape)).unwrap_err());
	}
	assert_eq!(queued(&store), vec![json!("a")]);
	assert_eq!(store.next_key, next_key);
	if store.is_persisted() {
		settle().await;
		assert_eq!(hydrated(db_name, 1024).await, vec![json!("a")]);
	}

	store.append(json!("b")).unwrap();
	assert_eq!(
		drain(&mut store, None, None).concat(),
		vec![json!("a"), json!("b")]
	);
	settle().await;
}

#[wasm_bindgen_test]
async fn test_very_deep_item_is_rejected_without_overflowing_the_stack() {
	let mut store = fresh_store("test-depth-very-deep", 1024).await;
	for shape in SHAPES {
		assert_too_deep(&store.append(deep(100_000, shape)).unwrap_err());
	}
	assert!(!store.has_data());
	store.append(json!("after")).unwrap();
	assert_eq!(read_back(&mut store), vec![json!("after")]);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_boundary_items_survive_a_reopen() {
	let db_name = "test-depth-reopen";
	let mut store = fresh_store(db_name, 1024 * 1024).await;
	if !store.is_persisted() {
		web_sys::console::log_1(&"Skipping depth reopen test - no persistence".into());
		return;
	}
	let items = boundary_items();
	for item in &items {
		store.append(item.clone()).unwrap();
	}
	settle().await;

	let mut reopened = WebStore::new(config(db_name, 1024 * 1024, with_source(KEY))).await;
	assert_eq!(queued(&reopened), items);
	assert_eq!(read_back(&mut reopened), items);
	reopened.reset();
	settle().await;
}
