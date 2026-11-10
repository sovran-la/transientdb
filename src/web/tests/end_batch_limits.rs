//! `end_batch` together with the other limits on a batch: no batch crosses a boundary
//! or exceeds a limit, and a long span is split without crossing into the next.

use super::end_batch::*;
use super::support::*;
use super::*;
use crate::batch::script::{self, Step};

#[wasm_bindgen_test]
async fn test_boundary_with_the_item_limit() {
	let mut store = store_with_limits("test-end-batch-item-limit", None, Some(2)).await;
	append_tags(&mut store, 0..5);
	store.end_batch().unwrap();
	append_tags(&mut store, 5..8);
	store.end_batch().unwrap();
	append_tags(&mut store, 8..9);
	let expected = vec![
		vec![0, 1],
		vec![2, 3],
		vec![4],
		vec![5, 6],
		vec![7],
		vec![8],
	];
	assert_eq!(drained(&mut store, None, None), expected);

	append_tags(&mut store, 0..5);
	store.end_batch().unwrap();
	append_tags(&mut store, 5..6);
	assert_eq!(
		drained(&mut store, Some(3), None),
		vec![vec![0, 1], vec![2, 3], vec![4], vec![5]]
	);
	settle().await;
}

#[wasm_bindgen_test]
async fn test_boundary_with_the_byte_ceiling() {
	let limit = envelope(KEY) + 40 * 2 + 1;
	let mut store = fresh_store("test-end-batch-bytes", limit).await;
	let items: Vec<Value> = (0..7).map(|n| item_of_len(40, n)).collect();
	for item in &items[..5] {
		store.append(item.clone()).unwrap();
	}
	store.end_batch().unwrap();
	for item in &items[5..] {
		store.append(item.clone()).unwrap();
	}
	let batches = drain(&mut store, None, None);
	let sizes: Vec<usize> = batches.iter().map(Vec::len).collect();
	assert_eq!(sizes, vec![2, 2, 1, 2]);
	assert_eq!(batches.concat(), items);

	for item in &items[..3] {
		store.append(item.clone()).unwrap();
	}
	store.end_batch().unwrap();
	store.append(items[3].clone()).unwrap();
	let sizes: Vec<usize> = drain(&mut store, None, Some(limit))
		.iter()
		.map(Vec::len)
		.collect();
	assert_eq!(sizes, vec![2, 1, 1]);
	settle().await;
}

#[wasm_bindgen_test]
async fn test_scripted_boundaries_match_the_model() {
	for (seed, per_batch) in [(1u32, 3usize), (42, 1), (0xdead_beef, 5), (99, 1000)] {
		let mut store = store_with_limits("test-end-batch-script", None, Some(per_batch)).await;
		let steps = script::steps(seed, 300);
		for step in &steps {
			match step {
				Step::Append(n) => store.append(tag(*n)).unwrap(),
				Step::End => store.end_batch().unwrap(),
			}
		}
		assert_eq!(
			drained(&mut store, None, None),
			script::expected(&steps, per_batch),
			"seed {seed}"
		);
		settle().await;
	}
}
