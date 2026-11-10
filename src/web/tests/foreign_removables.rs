//! `remove` acts only on removables this store instance issued. Another instance's are
//! ignored, in memory and in IndexedDB, even one opened on the same database, and that
//! is not an error.

use super::support::*;
use super::*;

type Removables = Vec<Box<dyn Equivalent>>;

fn items() -> Vec<Value> {
	(0..5).map(|i| json!({"i": i})).collect()
}

async fn store_holding(db_name: &str, items: &[Value]) -> WebStore {
	let mut store = fresh_store(db_name, 4096).await;
	for item in items {
		store.append(item.clone()).unwrap();
	}
	store
}

fn fetch(store: &mut WebStore, count: usize) -> Removables {
	store
		.fetch(Some(count), None)
		.unwrap()
		.unwrap()
		.removable
		.unwrap()
}

#[wasm_bindgen_test]
async fn test_another_stores_removables_remove_nothing() {
	let (name_a, name_b) = ("test-foreign-a", "test-foreign-b");
	// Same config and same appends: the same keys in both.
	let mut a = store_holding(name_a, &items()).await;
	let mut b = store_holding(name_b, &items()).await;
	let a_removable = fetch(&mut a, 3);
	let b_removable = fetch(&mut b, 5);

	b.remove(&a_removable).unwrap();
	a.remove(&b_removable).unwrap();
	assert_eq!(queued(&a), items());
	assert_eq!(queued(&b), items());
	if b.is_persisted() {
		settle().await;
		assert_eq!(hydrated(name_a, 4096).await, items());
		assert_eq!(hydrated(name_b, 4096).await, items());
	}

	// Each store's own removables still remove exactly what it fetched.
	a.remove(&a_removable).unwrap();
	assert_eq!(queued(&a), items()[3..]);
	assert_eq!(queued(&b), items());
	if a.is_persisted() {
		settle().await;
		assert_eq!(hydrated(name_a, 4096).await, items()[3..]);
		assert_eq!(hydrated(name_b, 4096).await, items());
	}
	a.reset();
	b.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_a_mix_removes_only_the_stores_own_items() {
	let (name_a, name_b) = ("test-foreign-mix-a", "test-foreign-mix-b");
	let mut a = store_holding(name_a, &items()).await;
	let mut b = store_holding(name_b, &items()).await;
	let mut mixed = fetch(&mut a, 5);
	mixed.extend(fetch(&mut b, 2));
	b.remove(&mixed).unwrap();
	assert_eq!(queued(&b), items()[2..]);
	assert_eq!(queued(&a), items());
	if a.is_persisted() {
		settle().await;
		assert_eq!(hydrated(name_a, 4096).await, items());
		assert_eq!(hydrated(name_b, 4096).await, items()[2..]);
	}
	a.reset();
	b.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_a_second_instance_on_the_same_database_ignores_the_firsts_removables() {
	let db_name = "test-foreign-same-db";
	let mut first = store_holding(db_name, &items()).await;
	if !first.is_persisted() {
		web_sys::console::log_1(&"Skipping same-database test - no persistence".into());
		return;
	}
	settle().await;
	let mut second = WebStore::new(config(db_name, 4096, with_source(KEY))).await;
	// The same items under the same keys, but a different instance.
	assert_eq!(queued(&second), items());
	assert_eq!(
		second.items.iter().map(|e| e.key).collect::<Vec<_>>(),
		first.items.iter().map(|e| e.key).collect::<Vec<_>>()
	);
	assert_ne!(first.id, second.id);

	let removable = fetch(&mut first, 3);
	second.remove(&removable).unwrap();
	settle().await;
	assert_eq!(queued(&second), items());
	assert_eq!(hydrated(db_name, 4096).await, items());

	// Removed through the instance that fetched them, they are gone after a reopen.
	first.remove(&removable).unwrap();
	assert_eq!(queued(&first), items()[3..]);
	settle().await;
	assert_eq!(hydrated(db_name, 4096).await, items()[3..]);
	first.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_every_instance_has_its_own_identity() {
	let mut ids = Vec::new();
	for i in 0..5 {
		ids.push(fresh_store(&format!("test-foreign-id-{i}"), 4096).await.id);
	}
	ids.push(fresh_store("test-foreign-id-0", 4096).await.id);
	let count = ids.len();
	ids.sort();
	ids.dedup();
	assert_eq!(ids.len(), count);
}
