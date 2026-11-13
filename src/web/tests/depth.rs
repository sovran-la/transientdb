//! Every item `append` accepts at the depth limit survives a reopen.

use super::*;
use crate::serialize::{nested, MAX_ITEM_DEPTH, SHAPES};

#[wasm_bindgen_test]
async fn test_deepest_items_survive_a_reopen() {
	let db_name = "test-depth-reopen";
	let Some(mut store) = persisted(db_name, 1 << 20, 1000).await else {
		return;
	};
	let items: Vec<Value> = SHAPES
		.iter()
		.flat_map(|&shape| {
			[json!(0), json!([]), json!({})].map(|inner| nested(MAX_ITEM_DEPTH, shape, inner))
		})
		.collect();
	contract::append_all(&mut store, &items);
	drop(store);

	let mut reopened = WebStore::new(config(db_name, 1 << 20, 1000)).await;
	let fetched = reopened.fetch(100, usize::MAX).unwrap().unwrap();
	assert_eq!(fetched.values(), items);
	drop(fetched);
	reopened.reset();
	settle().await;
}
