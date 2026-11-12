//! What a fetch hands out is the text that was stored. An item's JSON is checked when it
//! is appended, not when it is read back, so a slot whose CRC holds over text `append`
//! would not have written (not JSON, or nested too deep) is handed out as it is, and can
//! be released and removed like any other.

use super::*;
use crate::contract::{append_all, available, tagged};
use crate::file::layout::HEADER_LEN;
use crate::serialize::{nested, Shape, MAX_ITEM_DEPTH};
use crate::DataStore;
use serde_json::json;

/// Large enough for the deepest text a test writes.
const SIZE: usize = 200_000;

/// Text that does not parse as JSON.
const NOT_JSON: [&str; 6] = ["[1,2,", "not json", "{\"a\":1} trailing", "'s'", "1 2", " "];

/// A store on `path` holding items 0 to 3, with item 2's slot rewritten, between runs, to
/// hold `text` under a valid entry.
fn store_holding(path: &Path, text: &str) -> FileStore {
	let config = config(path, SIZE, 100);
	let mut first = FileStore::new(config.clone());
	append_all(&mut first, &tagged(0..4));
	drop(first);
	write_item(path, SIZE, 1, 2, text.as_bytes());
	FileStore::new(config)
}

/// The text of each of items 0 to 3, with item 2's being `text`.
fn texts_with(text: &str) -> Vec<String> {
	let mut texts: Vec<String> = tagged(0..4).iter().map(Value::to_string).collect();
	texts[1] = text.to_string();
	texts
}

/// Checks that `store`, made by [`store_holding`] with `text`, hands item 2 out as it was
/// stored, `values()` giving `value` for it, and releases and removes it like any other.
fn assert_handed_out_as_stored(mut store: FileStore, path: &Path, text: &str, value: Value) {
	assert!(store.has_data(), "{text:?}");
	let items = store.fetch(10, usize::MAX).unwrap().unwrap();
	let expected = texts_with(text);
	assert_eq!(items.iter().collect::<Vec<_>>(), expected, "{text:?}");
	assert_eq!(
		items.json(),
		format!("[{}]", expected.join(",")),
		"{text:?}"
	);
	assert_eq!(
		items.byte_len(),
		expected.iter().map(String::len).sum::<usize>()
	);
	let mut values = tagged(0..4);
	values[1] = value.clone();
	assert_eq!(items.values(), values, "{text:?}");
	// Released, it is handed out again, in its place.
	items.release();
	let first = store.fetch(1, usize::MAX).unwrap().unwrap();
	let second = store.fetch(1, usize::MAX).unwrap().unwrap();
	assert_eq!(second.iter().collect::<Vec<_>>(), [text], "{text:?}");
	assert_eq!(second.json(), format!("[{text}]"), "{text:?}");
	assert_eq!(second.values(), [value], "{text:?}");
	// Removed, it is gone from the file, and nothing else is.
	store.remove(second).unwrap();
	drop(first);
	assert_eq!(
		available(&mut store),
		[&tagged(0..1)[..], &tagged(2..4)].concat()
	);
	drop(store);
	let mut store = FileStore::new(config(path, SIZE, 100));
	assert_eq!(
		available(&mut store),
		[&tagged(0..1)[..], &tagged(2..4)].concat()
	);
	let all = store.fetch(10, usize::MAX).unwrap().unwrap();
	store.remove(all).unwrap();
	assert_eq!(file_len(path), HEADER_LEN, "{text:?}");
}

#[test]
fn test_an_item_that_is_not_json_is_handed_out_as_it_was_stored() {
	for text in NOT_JSON {
		assert!(serde_json::from_str::<Value>(text).is_err(), "{text:?}");
		let (_dir, path) = temp_path();
		let store = store_holding(&path, text);
		assert_handed_out_as_stored(store, &path, text, Value::String(text.to_string()));
	}
}

#[test]
fn test_an_item_nested_deeper_than_append_allows_is_handed_out_as_it_was_stored() {
	// One level too deep for `append`: `values()` still parses it.
	let deep = nested(MAX_ITEM_DEPTH + 1, Shape::Arrays, json!(0));
	let (_dir, path) = temp_path();
	let store = store_holding(&path, &deep.to_string());
	assert_handed_out_as_stored(store, &path, &deep.to_string(), deep);
	// Far too deep for `serde_json` to parse: `values()` gives the text, without
	// overflowing the stack.
	let deepest = "[".repeat(SIZE / 2) + &"]".repeat(SIZE / 2);
	let (_dir, path) = temp_path();
	let store = store_holding(&path, &deepest);
	assert_handed_out_as_stored(store, &path, &deepest, Value::String(deepest.clone()));
}

#[test]
fn test_an_item_that_is_not_json_does_not_disturb_the_fetch_limits() {
	let (_dir, path) = temp_path();
	let mut store = store_holding(&path, "not json");
	// Counted by its length, like any item: `{"i":0}` is 7 bytes, `not json` 8.
	assert_eq!(store.fetch(10, 14).unwrap().unwrap().len(), 1);
	let two = store.fetch(2, 15).unwrap().unwrap();
	assert_eq!(two.iter().collect::<Vec<_>>(), [r#"{"i":0}"#, "not json"]);
	assert_eq!(two.byte_len(), 15);
}
