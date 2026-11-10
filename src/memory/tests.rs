use crate::memory::{MemoryConfig, MemoryStore};
use crate::{contract, DataStore};
use serde_json::{json, Value};

mod capacity;
mod foreign;
mod panic;
mod sequence;

fn store(max_item_size: usize, max_items: usize) -> MemoryStore {
	MemoryStore::new(MemoryConfig {
		max_item_size,
		max_items,
	})
}

/// The values of the queued items, fetched or not, in order.
fn queued(store: &MemoryStore) -> Vec<Value> {
	let texts = store.queue.items.iter().map(|item| item.text.as_str());
	texts
		.map(|text| serde_json::from_str(text).unwrap())
		.collect()
}

macro_rules! memory_check {
	($name:ident, $max_item_size:expr, $max_items:expr) => {
		#[test]
		fn $name() {
			contract::$name(&mut store($max_item_size, $max_items));
		}
	};
}

mod contract_checks {
	use super::*;
	contract::for_each_check!(memory_check);
}

#[test]
fn test_basic_operations() {
	let mut store = store(1024, 1000);
	assert!(!store.has_data());

	store
		.append(json!({"event": "test", "value": 123}))
		.unwrap();
	assert!(store.has_data());

	let items = store.fetch(10, 1024).unwrap().unwrap();
	assert_eq!(items.values(), vec![json!({"event": "test", "value": 123})]);
	// Still stored, but held back from other fetches.
	assert_eq!(store.queue.items.len(), 1);
	assert!(!store.has_data());

	store.remove(items).unwrap();
	assert!(!store.has_data());
	assert!(store.queue.items.is_empty());
}

#[test]
fn test_every_json_type_is_stored_and_returned() {
	let mut store = store(1024, 100);
	let items = vec![
		json!(null),
		json!(true),
		json!(42),
		json!(42.5),
		json!("string"),
		json!([1, 2, 3]),
		json!({"key": "value"}),
	];
	contract::append_all(&mut store, &items);
	assert_eq!(store.fetch(100, 1024).unwrap().unwrap().values(), items);
}

#[test]
fn test_items_are_held_as_compact_text() {
	let mut store = store(1024, 100);
	store
		.append(json!({ "a" : [1, 2], "b": { "c": null } }))
		.unwrap();
	assert_eq!(store.queue.items[0].text, r#"{"a":[1,2],"b":{"c":null}}"#);
}

#[test]
fn test_config_limits_of_one_work() {
	let mut store = store(1, 1);
	store.append(json!(1)).unwrap();
	store.append(json!(2)).unwrap();
	assert!(store.append(json!(10)).is_err());
	assert_eq!(contract::drain(&mut store, 10, 10).concat(), vec![json!(2)]);
}

#[test]
#[should_panic(expected = "max_item_size = 0? Even `0` takes a byte. There is no item that small.")]
fn test_rejects_zero_max_item_size() {
	let _store = store(0, 1000);
}

#[test]
#[should_panic(
	expected = "max_items = 0? So... you want a store that stores nothing? That's what /dev/null is for."
)]
fn test_rejects_zero_max_items() {
	let _store = store(1024, 0);
}
