//! Stores on different databases never see, count, fetch or clear each other's records,
//! in memory or in IndexedDB.

use super::*;

fn items(tag: &str, n: i64) -> Vec<Value> {
	(0..n).map(|i| json!({ "store": tag, "i": i })).collect()
}

#[wasm_bindgen_test]
async fn test_stores_on_different_databases_are_isolated() {
	let (name_a, name_b) = ("test-isolation-a", "test-isolation-b");
	let Some(mut a) = persisted(name_a, 4096, 1000).await else {
		return;
	};
	let Some(mut b) = persisted(name_b, 4096, 1000).await else {
		return;
	};
	let (a_items, b_items) = (items("a", 3), items("b", 4));
	contract::append_all(&mut a, &a_items);
	contract::append_all(&mut b, &b_items);
	settle().await;

	assert_eq!(available(&mut a), a_items);
	assert_eq!(available(&mut b), b_items);
	assert_eq!(persisted_values(&a).await, a_items);
	assert_eq!(persisted_values(&b).await, b_items);

	// Resetting one store clears only its own database.
	a.reset();
	settle().await;
	assert!(!a.has_data());
	assert!(a.fetch(10, 4096).unwrap().is_none());
	assert_eq!(available(&mut b), b_items);
	assert!(records(&a).await.is_empty());
	assert_eq!(persisted_values(&b).await, b_items);

	// Removing from one store's fetch never removes the other's records.
	let more_a = items("a2", 2);
	contract::append_all(&mut a, &more_a);
	let fetched = b.fetch(100, 4096).unwrap().unwrap();
	b.remove(fetched).unwrap();
	settle().await;
	assert!(!b.has_data());
	assert_eq!(available(&mut a), more_a);
	assert_eq!(persisted_values(&a).await, more_a);
	assert!(records(&b).await.is_empty());
	drop((a, b));
	// Once both are gone, each one's items are adopted from its own database only.
	assert_eq!(adopted(name_a).await, more_a);
	assert!(adopted(name_b).await.is_empty());
	fresh_store(name_a, 4096, 1000).await;
	settle().await;
}

/// IndexedDB names are arbitrary strings, so no `database_name` is refused: even an
/// empty one, or one with what would be path separators or a NUL byte in a file name,
/// persists and is isolated from the others.
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
		let Some(store) = persisted(name, 4096, 1000).await else {
			return;
		};
		stores.push(store);
	}
	for (store, name) in stores.iter_mut().zip(names) {
		contract::append_all(store, &items(name, 2));
	}
	settle().await;

	for (store, name) in stores.iter_mut().zip(names) {
		assert_eq!(available(store), items(name, 2), "{name:?}");
		assert_eq!(persisted_values(store).await, items(name, 2), "{name:?}");
	}
	// Each database's lock names are its own: a store gone from one is adopted there.
	drop(stores);
	for name in names {
		assert_eq!(adopted(name).await, items(name, 2), "{name:?}");
	}
	for name in names {
		fresh_store(name, 4096, 1000).await;
	}
	settle().await;
	for name in names {
		assert!(adopted(name).await.is_empty(), "{name:?}");
	}
}
