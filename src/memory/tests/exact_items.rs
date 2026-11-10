//! Items come back exactly as they were appended: the store holds each `Value` as given,
//! so numbers a JavaScript number cannot hold, and floats such as `42.0`, are unchanged.

use super::support::*;
use super::*;
use crate::envelope::samples::items_of;
use crate::item_samples::{assert_exact, awkward_items};

#[test]
fn test_awkward_items_are_fetched_exactly() {
	let items = awkward_items();
	let mut store = store(4096);
	for item in &items {
		store.append(item.clone()).unwrap();
	}
	let result = store.fetch(None, None).unwrap().unwrap();
	let batch = result.data.as_ref().unwrap();
	assert_exact(&items_of(batch, &with_source(KEY)), &items);
	// The batch's text holds each item as it serializes on its own.
	let text = serde_json::to_string(batch).unwrap();
	for item in &items {
		let item_text = serde_json::to_string(item).unwrap();
		assert!(text.contains(&item_text), "{item_text} not in {text}");
	}
	store.remove(&result.removable.unwrap()).unwrap();
	assert!(!store.has_data());
}
