use super::*;
use crate::contract;
use serde_json::json;
use support::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

mod capacity;
mod depth;
mod exact_items;
mod foreign;
mod hydrate;
mod isolation;
mod persistence;
mod reopen;
mod schema;
mod support;
mod upgrade;
mod write_failure;

macro_rules! web_check {
	($name:ident, $max_item_size:expr, $max_items:expr) => {
		#[wasm_bindgen_test]
		async fn $name() {
			let db_name = concat!("test-contract-", stringify!($name));
			let mut store = fresh_store(db_name, $max_item_size, $max_items).await;
			contract::$name(&mut store);
			// What IndexedDB holds is exactly what the store still holds.
			if store.is_persisted() {
				assert_eq!(flushed(&store).await, stored(&store));
			}
			store.reset();
			settle().await;
		}
	};
}

mod contract_checks {
	use super::*;
	contract::for_each_check!(web_check);
}

#[wasm_bindgen_test]
async fn test_basic_operations() {
	let mut store = fresh_store("test-basic-ops", 1024, 1000).await;
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
	settle().await;
}

#[wasm_bindgen_test]
async fn test_persistence_state() {
	let store = WebStore::new(config("test-persistence-state", 1024, 1000)).await;
	let state = store.persistence_state();
	// In headless Chrome IndexedDB is available.
	assert_eq!(state, PersistenceState::Persisted);
	assert_eq!(store.is_persisted(), state == PersistenceState::Persisted);
}

#[wasm_bindgen_test]
#[should_panic(expected = "max_item_size = 0? Even `0` takes a byte.")]
async fn test_rejects_zero_max_item_size() {
	let _store = WebStore::new(config("test-panic", 0, 1000)).await;
}

#[wasm_bindgen_test]
#[should_panic(expected = "max_items = 0?")]
async fn test_rejects_zero_max_items() {
	let _store = WebStore::new(config("test-panic", 1024, 0)).await;
}
