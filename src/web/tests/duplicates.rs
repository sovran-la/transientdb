//! Identical items are distinct entries: `remove` deletes exactly what a fetch returned,
//! in memory and in IndexedDB.

use super::support::*;
use super::*;

fn append_all(store: &mut WebStore, items: &[Value]) {
	for item in items {
		store.append(item.clone()).unwrap();
	}
}

type Removables = Vec<Box<dyn Equivalent>>;

/// Fetches up to `count` items; returns them and their removables.
fn fetch(store: &mut WebStore, count: usize) -> (Vec<Value>, Removables) {
	let result = store.fetch(Some(count), None).unwrap().expect("data");
	let items = checked_batch(&result, 4096);
	(items, result.removable.unwrap())
}

/// What a new instance on `db_name` would hydrate, or the queue itself when IndexedDB is
/// unavailable (then there is nothing persisted to compare).
async fn persisted_or_queued(store: &WebStore, db_name: &str) -> Vec<Value> {
	if !store.is_persisted() {
		return queued(store);
	}
	settle().await;
	hydrated(db_name, 4096).await
}

#[wasm_bindgen_test]
async fn test_walking_the_store_returns_every_duplicate() {
	let db_name = "test-dup-walk";
	let mut store = fresh_store(db_name, 4096).await;
	let (a, b) = (json!("a"), json!({"b": 1}));
	let appended = vec![a.clone(), a.clone(), b.clone(), a.clone(), b.clone(), b, a];
	append_all(&mut store, &appended);

	let batches = drain(&mut store, Some(2), None);
	assert_eq!(batches.concat(), appended);
	assert!(persisted_or_queued(&store, db_name).await.is_empty());
}

#[wasm_bindgen_test]
async fn test_removing_twice_is_a_noop() {
	let db_name = "test-dup-remove-twice";
	let mut store = fresh_store(db_name, 4096).await;
	let same = json!("dup");
	append_all(&mut store, &[same.clone(), same.clone(), json!("other")]);

	let (_, removable) = fetch(&mut store, 1);
	store.remove(&removable).unwrap();
	store.remove(&removable).unwrap();
	let expected = vec![same, json!("other")];
	assert_eq!(queued(&store), expected);
	assert_eq!(persisted_or_queued(&store, db_name).await, expected);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_removing_an_item_evicted_since_fetch_is_a_noop() {
	let db_name = "test-dup-evicted";
	let mut config = config(db_name, 4096, KEY);
	config.max_items = 3;
	let mut store = fresh_store_with(config).await;
	let (a, b, c) = (json!("a"), json!("b"), json!("c"));
	append_all(&mut store, &[a.clone(), b.clone(), c.clone()]);

	let (items, removable) = fetch(&mut store, 1);
	assert_eq!(items, vec![a.clone()]);
	// Drop-oldest evicts the fetched `a`; an identical `a` arrives behind it.
	store.append(a.clone()).unwrap();
	store.remove(&removable).unwrap();

	let expected = vec![b, c, a];
	assert_eq!(queued(&store), expected);
	assert_eq!(persisted_or_queued(&store, db_name).await, expected);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_removables_that_are_not_from_fetch_remove_nothing() {
	let db_name = "test-dup-foreign";
	let mut store = fresh_store(db_name, 4096).await;
	append_all(&mut store, &[json!("a"), json!("b")]);

	let foreign: Vec<Box<dyn Equivalent>> = vec![Box::new(json!("a"))];
	store.remove(&foreign).unwrap();
	let expected = vec![json!("a"), json!("b")];
	assert_eq!(queued(&store), expected);
	assert_eq!(persisted_or_queued(&store, db_name).await, expected);
	store.reset();
	settle().await;
}
