//! The close-flush contract on `WebStore`: `json()`, then `remove`, then the page goes
//! away. The delete is issued during `remove`, so it is ordered before any other store's
//! adoption of what is left, and the removed batch is never adopted.

use super::*;
use contract::{append_all, tagged};

#[wasm_bindgen_test]
async fn test_a_batch_removed_before_closing_is_never_adopted() {
	let db_name = "test-close-flush";
	if persisted(db_name, 4096, 1000).await.is_none() {
		return;
	}
	for n in 1..=6 {
		let mut store = WebStore::new(config(db_name, 4096, 1000)).await;
		append_all(&mut store, &tagged(0..n));
		let items = store.fetch(3, 4096).unwrap().unwrap();
		let body = items.json();
		store.remove(items).unwrap();
		// The page goes away: no await between the remove and the drop.
		drop(store);
		let sent: Vec<Value> = serde_json::from_str(&body).unwrap();
		assert_eq!(sent, tagged(0..n.min(3)));

		let mut next = WebStore::new(config(db_name, 4096, 1000)).await;
		assert_eq!(queued(&next), tagged(n.min(3)..n), "after {n}");
		next.reset();
		drop(next);
		settle().await;
	}
}

/// A tab that sends its last batch without removing it first gets it adopted and sent
/// again: at least once, never lost.
#[wasm_bindgen_test]
async fn test_a_batch_not_removed_before_closing_is_adopted() {
	let db_name = "test-close-unremoved";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut store, &tagged(0..4));
	let items = store.fetch(3, 4096).unwrap().unwrap();
	let _body = items.json();
	drop(store);
	let mut next = WebStore::new(config(db_name, 4096, 1000)).await;
	assert_eq!(queued(&next), tagged(0..4));
	drop(items);
	next.reset();
	settle().await;
}
