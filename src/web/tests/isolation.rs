//! Stores on different databases never see, count, fetch or clear each other's records,
//! in memory or in IndexedDB.

use super::support::*;
use super::*;

fn items(tag: &str, n: i64) -> Vec<Value> {
	(0..n).map(|i| json!({ "store": tag, "i": i })).collect()
}

fn fetched_items(store: &mut WebStore) -> Vec<Value> {
	match store.fetch(None, None).unwrap() {
		Some(result) => checked_batch(&result, store.config.max_fetch_size),
		None => Vec::new(),
	}
}

#[wasm_bindgen_test]
async fn test_stores_on_different_databases_are_isolated() {
	let (name_a, name_b) = ("test-isolation-a", "test-isolation-b");
	let mut a = fresh_store(name_a, 4096).await;
	let mut b = fresh_store(name_b, 4096).await;
	if !a.is_persisted() || !b.is_persisted() {
		web_sys::console::log_1(&"Skipping isolation test - no persistence".into());
		return;
	}
	let (a_items, b_items) = (items("a", 3), items("b", 4));
	for item in &a_items {
		a.append(item.clone()).unwrap();
	}
	for item in &b_items {
		b.append(item.clone()).unwrap();
	}
	settle().await;

	assert_eq!(fetched_items(&mut a), a_items);
	assert_eq!(fetched_items(&mut b), b_items);
	assert_eq!(hydrated(name_a, 4096).await, a_items);
	assert_eq!(hydrated(name_b, 4096).await, b_items);

	// Resetting one store clears only its own database.
	a.reset();
	settle().await;
	assert!(!a.has_data());
	assert!(a.fetch(None, None).unwrap().is_none());
	assert_eq!(fetched_items(&mut b), b_items);
	assert!(hydrated(name_a, 4096).await.is_empty());
	assert_eq!(hydrated(name_b, 4096).await, b_items);

	// Removing from one store's fetch never removes the other's records.
	let more_a = items("a2", 2);
	for item in &more_a {
		a.append(item.clone()).unwrap();
	}
	let result = b.fetch(None, None).unwrap().unwrap();
	b.remove(&result.removable.unwrap()).unwrap();
	settle().await;
	assert!(!b.has_data());
	assert_eq!(fetched_items(&mut a), more_a);
	assert_eq!(hydrated(name_a, 4096).await, more_a);
	assert!(hydrated(name_b, 4096).await.is_empty());
}

/// IndexedDB names are arbitrary strings, so unlike `DirectoryConfig::base_filename`
/// no `database_name` is refused: even an empty one, or one with what would be path
/// separators or a NUL byte in a file name, persists and is isolated from the others.
#[wasm_bindgen_test]
async fn test_any_database_name_persists_and_is_isolated() {
	let names = [
		"",
		" ",
		"../escape",
		"a\\b/c",
		"nul\0inside",
		".",
		"🦀 データ",
	];
	let mut stores = Vec::new();
	for name in names {
		let store = fresh_store(name, 4096).await;
		if !store.is_persisted() {
			web_sys::console::log_1(&"Skipping database name test - no persistence".into());
			return;
		}
		stores.push(store);
	}
	for (store, name) in stores.iter_mut().zip(names) {
		for item in items(name, 2) {
			store.append(item).unwrap();
		}
	}
	settle().await;

	for (store, name) in stores.iter_mut().zip(names) {
		assert_eq!(fetched_items(store), items(name, 2), "{name:?}");
		assert_eq!(hydrated(name, 4096).await, items(name, 2), "{name:?}");
	}
	for store in &mut stores {
		store.reset();
	}
	settle().await;
	for name in names {
		assert!(hydrated(name, 4096).await.is_empty(), "{name:?}");
	}
}
