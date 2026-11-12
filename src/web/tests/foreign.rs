//! `remove` acts only on an `Items` this store instance issued. Another instance's is
//! ignored, in memory and in IndexedDB, even one opened on the same database, which is
//! not an error, and goes back to the store that issued it.

use super::*;
use contract::{append_all, tagged};

async fn store_holding(db_name: &str, n: usize) -> WebStore {
	let mut store = fresh_store(db_name, 4096, 1000).await;
	append_all(&mut store, &tagged(0..n));
	store
}

#[wasm_bindgen_test]
async fn test_another_stores_items_remove_nothing() {
	let (name_a, name_b) = ("test-foreign-a", "test-foreign-b");
	let mut a = store_holding(name_a, 5).await;
	let mut b = store_holding(name_b, 5).await;
	let from_a = a.fetch(3, 4096).unwrap().unwrap();
	let from_b = b.fetch(5, 4096).unwrap().unwrap();

	b.remove(from_a).unwrap();
	a.remove(from_b).unwrap();
	assert_eq!(queued(&a), tagged(0..5));
	assert_eq!(queued(&b), tagged(0..5));
	// Each went back to its own store, released.
	assert_eq!(available(&mut a), tagged(0..5));
	assert_eq!(available(&mut b), tagged(0..5));
	if a.is_persisted() {
		assert_eq!(hydrated(name_a).await, tagged(0..5));
		assert_eq!(hydrated(name_b).await, tagged(0..5));
	}

	// Each store's own Items still removes exactly what it fetched.
	let from_a = a.fetch(3, 4096).unwrap().unwrap();
	a.remove(from_a).unwrap();
	assert_eq!(queued(&a), tagged(3..5));
	assert_eq!(queued(&b), tagged(0..5));
	if a.is_persisted() {
		assert_eq!(hydrated(name_a).await, tagged(3..5));
		assert_eq!(hydrated(name_b).await, tagged(0..5));
	}
	a.reset();
	b.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_a_second_instance_on_the_same_database_ignores_the_firsts_items() {
	let db_name = "test-foreign-same-db";
	let mut first = store_holding(db_name, 5).await;
	if !first.is_persisted() {
		web_sys::console::log_1(&"Skipping same-database test - no persistence".into());
		return;
	}
	settle().await;
	let mut second = WebStore::new(config(db_name, 4096, 1000)).await;
	// The same items under the same sequence numbers, but a different instance.
	assert_eq!(stored(&second), stored(&first));
	assert!(!first.queue.ledger.same(&second.queue.ledger));

	let items = first.fetch(3, 4096).unwrap().unwrap();
	second.remove(items).unwrap();
	assert_eq!(queued(&second), tagged(0..5));
	assert_eq!(hydrated(db_name).await, tagged(0..5));
	// Released back to the first instance by being ignored.
	let items = first.fetch(3, 4096).unwrap().unwrap();
	assert_eq!(items.values(), tagged(0..3));

	// Removed through the instance that fetched them, they are gone after a reopen.
	first.remove(items).unwrap();
	assert_eq!(queued(&first), tagged(3..5));
	assert_eq!(hydrated(db_name).await, tagged(3..5));
	first.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_every_instance_has_its_own_ledger() {
	let mut stores = Vec::new();
	for i in 0..5 {
		stores.push(fresh_store(&format!("test-foreign-id-{i}"), 64, 8).await);
	}
	stores.push(fresh_store("test-foreign-id-0", 64, 8).await);
	for (i, a) in stores.iter().enumerate() {
		for (j, b) in stores.iter().enumerate() {
			assert_eq!(a.queue.ledger.same(&b.queue.ledger), i == j);
		}
	}
}

#[wasm_bindgen_test]
async fn test_items_outliving_their_store_release_harmlessly() {
	let mut store = store_holding("test-foreign-outlive", 3).await;
	let items = store.fetch(2, 4096).unwrap().unwrap();
	drop(store);
	assert_eq!(items.values(), tagged(0..2));
	items.release();
	settle().await;
}
