//! What a fetch hands out after a reload is the text that was stored. An item's JSON is
//! checked when it is appended, not when the store opens, so a record holding text
//! `append` would not have written (put there by something else: another script, devtools)
//! is kept and handed out as it is, and can be released and removed like any other. A
//! record larger than `max_item_size` is still discarded.

use super::schema::add_raw;
use super::*;
use crate::serialize::{nested, Shape, MAX_ITEM_DEPTH};
use contract::tagged;

/// Opens a fresh, persisted store on `db_name`, adds a record for each of `texts` under
/// sequence numbers 1 up, and opens the store again over them. `None`, after logging,
/// when IndexedDB is unavailable.
async fn reopened_over(db_name: &str, max_item_size: usize, texts: &[&str]) -> Option<WebStore> {
	let store = persisted(db_name, 1 << 20, 1000).await?;
	settle().await;
	let records: Vec<Value> = texts
		.iter()
		.enumerate()
		.map(|(i, text)| idb::record(i as u64 + 1, text))
		.collect();
	add_raw(store.db.as_ref().unwrap(), &records).await;
	drop(store);
	let reopened = WebStore::new(config(db_name, max_item_size, 1000)).await;
	assert!(reopened.is_persisted());
	Some(reopened)
}

/// Removes everything left, in memory and in IndexedDB.
async fn cleared(mut store: WebStore) {
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_a_record_that_is_not_json_is_handed_out_as_it_was_stored() {
	let db_name = "test-stored-text-not-json";
	let good = tagged(0..2);
	let (first, second) = (good[0].to_string(), good[1].to_string());
	let texts = [first.as_str(), "not json", "[1,2,", second.as_str()];
	let Some(mut store) = reopened_over(db_name, 4096, &texts).await else {
		return;
	};
	assert!(store.has_data());
	let items = store.fetch(10, 4096).unwrap().unwrap();
	assert_eq!(items.iter().collect::<Vec<_>>(), texts);
	assert_eq!(items.json(), format!("[{}]", texts.join(",")));
	assert_eq!(
		items.byte_len(),
		texts.iter().map(|t| t.len()).sum::<usize>()
	);
	assert_eq!(
		items.values(),
		vec![
			good[0].clone(),
			json!("not json"),
			json!("[1,2,"),
			good[1].clone()
		]
	);
	// Released, they are handed out again, in order.
	items.release();
	let head = store.fetch(2, 4096).unwrap().unwrap();
	assert_eq!(head.iter().collect::<Vec<_>>(), texts[..2]);
	// Removed, the bad one is gone from IndexedDB as well; the rest are not.
	store.remove(head).unwrap();
	let rest = store.fetch(10, 4096).unwrap().unwrap();
	assert_eq!(rest.iter().collect::<Vec<_>>(), texts[2..]);
	drop(rest);
	assert_eq!(
		flushed(&store).await,
		vec![(3, "[1,2,".to_string()), (4, second.clone())]
	);
	cleared(store).await;
}

#[wasm_bindgen_test]
async fn test_a_record_nested_deeper_than_append_allows_is_handed_out_as_it_was_stored() {
	let db_name = "test-stored-text-too-deep";
	// One level too deep for `append`, which `values()` still parses, and far too deep
	// for `serde_json`, which `values()` gives as the text, without overflowing.
	let deep = nested(MAX_ITEM_DEPTH + 1, Shape::Objects, json!(0));
	let deep_text = deep.to_string();
	let deepest = "[".repeat(100_000) + &"]".repeat(100_000);
	let texts = [deep_text.as_str(), deepest.as_str(), "1"];
	let Some(mut store) = reopened_over(db_name, 1 << 20, &texts).await else {
		return;
	};
	let items = store.fetch(10, usize::MAX).unwrap().unwrap();
	assert_eq!(items.iter().collect::<Vec<_>>(), texts);
	assert_eq!(items.json(), format!("[{}]", texts.join(",")));
	let values = items.values();
	assert_eq!(values[0], deep);
	assert_eq!(values[1], Value::String(deepest.clone()));
	assert_eq!(values[2], json!(1));
	drop(values);
	store.remove(items).unwrap();
	assert!(!store.has_data());
	assert!(flushed(&store).await.is_empty());
	cleared(store).await;
}

#[wasm_bindgen_test]
async fn test_a_record_larger_than_max_item_size_is_still_discarded() {
	let db_name = "test-stored-text-oversized";
	let large = "x".repeat(65);
	let texts = ["\"ok\"", large.as_str(), "not json", "\"also ok\""];
	let Some(store) = reopened_over(db_name, 64, &texts).await else {
		return;
	};
	assert_eq!(
		stored(&store),
		vec![
			(1, "\"ok\"".to_string()),
			(3, "not json".to_string()),
			(4, "\"also ok\"".to_string())
		]
	);
	// The discarded record was deleted from IndexedDB.
	assert_eq!(flushed(&store).await, stored(&store));
	cleared(store).await;
}
