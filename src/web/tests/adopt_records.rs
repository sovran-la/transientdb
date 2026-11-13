//! What adopting does with the items it finds: items larger than `max_item_size` are
//! discarded, deleted from IndexedDB and logged; past `max_items` the oldest are dropped,
//! deleted and logged. Records that are not items are in `adopt_invalid`.

use super::*;
use contract::{append_all, item_of_len, tagged};

/// A database `db_name` holding nothing but `items`, appended by a roomy store that is
/// gone by the time this returns. `None`, after logging, when IndexedDB is unavailable.
async fn orphaned(db_name: &str, items: &[Value]) -> Option<()> {
	let mut store = persisted(db_name, 1 << 20, 1 << 20).await?;
	append_all(&mut store, items);
	Some(())
}

#[wasm_bindgen_test]
async fn test_adopting_past_a_lowered_max_items_drops_the_oldest() {
	let db_name = "test-adopt-lowered-max-items";
	if orphaned(db_name, &tagged(0..10)).await.is_none() {
		return;
	}
	warnings();
	let mut store = WebStore::new(config(db_name, 4096, 3)).await;
	assert_eq!(queued(&store), tagged(7..10));
	assert_eq!(
		warnings(),
		["Dropping the 7 oldest items: adopting orphaned items took the store past max_items = 3"]
	);
	// The drop reached IndexedDB: nothing but what the store holds is left.
	assert_eq!(flushed(&store).await, stored(&store));
	assert_eq!(records(&store).await.len(), 3);
	// From then on the store holds at most 3, as usual.
	store.append(json!({"i": 10})).unwrap();
	assert_eq!(queued(&store), tagged(8..11));
	drop(store);
	assert_eq!(adopted(db_name).await, tagged(8..11));
	fresh_store(db_name, 4096, 1000).await;
	settle().await;
}

/// Adopted orphans count toward the adopter's cap, and are queued behind its own items,
/// so its own oldest go first.
#[wasm_bindgen_test]
async fn test_adopted_items_count_toward_max_items() {
	let db_name = "test-adopt-cap-shared";
	let Some(mut b) = persisted(db_name, 4096, 5).await else {
		return;
	};
	let mut a = WebStore::new(config(db_name, 4096, 5)).await;
	append_all(&mut a, &tagged(0..4));
	append_all(&mut b, &tagged(10..13));
	let held = b.fetch(1, 4096).unwrap().unwrap();
	drop(a);
	warnings();
	b.adopt_now().await;
	assert_eq!(queued(&b), [tagged(12..13), tagged(0..4)].concat());
	assert_eq!(warnings().len(), 1);
	assert_eq!(flushed(&b).await, stored(&b));
	assert_eq!(records(&b).await.len(), 5);
	// The pending item that was dropped matches nothing.
	b.remove(held).unwrap();
	assert_eq!(queued(&b).len(), 5);
	b.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_a_raised_max_items_keeps_everything() {
	let db_name = "test-adopt-raised-max-items";
	let Some(mut a) = persisted(db_name, 4096, 10).await else {
		return;
	};
	append_all(&mut a, &tagged(0..10));
	drop(a);
	let mut store = WebStore::new(config(db_name, 4096, 1000)).await;
	assert_eq!(queued(&store), tagged(0..10));
	append_all(&mut store, &tagged(10..20));
	assert_eq!(persisted_values(&store).await, tagged(0..20));
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_items_over_a_smaller_max_item_size_are_discarded() {
	let db_name = "test-adopt-smaller-max-item-size";
	let items = [
		item_of_len(100, 0),
		json!("a"),
		item_of_len(51, 1),
		item_of_len(50, 2),
		item_of_len(500, 3),
		json!("b"),
	];
	if orphaned(db_name, &items).await.is_none() {
		return;
	}
	warnings();
	let mut store = WebStore::new(config(db_name, 50, 1000)).await;
	let kept = vec![json!("a"), item_of_len(50, 2), json!("b")];
	assert_eq!(queued(&store), kept);
	assert_eq!(
		warnings(),
		[
			"Discarding stored item 1: it is 100 bytes, over max_item_size = 50 bytes",
			"Discarding stored item 3: it is 51 bytes, over max_item_size = 50 bytes",
			"Discarding stored item 5: it is 500 bytes, over max_item_size = 50 bytes",
		]
	);
	assert_eq!(flushed(&store).await, stored(&store));
	assert_eq!(records(&store).await.len(), 3);
	// Every item left can be fetched under the new limit.
	let all = store.fetch(10, 1000).unwrap().unwrap();
	assert_eq!(all.values(), kept);
	drop(all);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_both_limits_lowered_at_once() {
	let db_name = "test-adopt-both-lowered";
	let items: Vec<Value> = (0..8)
		.map(|i| item_of_len(if i % 2 == 0 { 40 } else { 10 }, i))
		.collect();
	if orphaned(db_name, &items).await.is_none() {
		return;
	}
	// The oversized ones go first, then the oldest of the rest beyond max_items.
	let mut store = WebStore::new(config(db_name, 20, 2)).await;
	assert_eq!(queued(&store), vec![items[5].clone(), items[7].clone()]);
	assert_eq!(flushed(&store).await, stored(&store));
	assert_eq!(records(&store).await.len(), 2);
	store.reset();
	settle().await;
}

/// An adopter out of sequence numbers adopts what it can and leaves the rest where they
/// are, for another tab, rather than lose them.
#[wasm_bindgen_test]
async fn test_an_adopter_out_of_sequence_numbers_leaves_the_rest() {
	let db_name = "test-adopt-exhausted";
	let Some(mut b) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let mut a = WebStore::new(config(db_name, 4096, 1000)).await;
	append_all(&mut a, &tagged(0..3));
	let a_tab = a.tab().to_owned();
	drop(a);
	b.shared.queue.borrow_mut().next_seq = idb::MAX_SEQ;
	warnings();
	b.adopt_now().await;
	assert_eq!(stored(&b), vec![(idb::MAX_SEQ, r#"{"i":0}"#.to_string())]);
	assert!(warnings()
		.iter()
		.any(|w| w.starts_with("Sequence numbers exhausted")));
	// The other two are still the orphan's, for a tab with numbers to spare.
	let left: Vec<Value> = records(&b)
		.await
		.into_iter()
		.filter(|r| r["tab"] == a_tab.as_str())
		.collect();
	assert_eq!(left.len(), 2);
	let c = WebStore::new(config(db_name, 4096, 1000)).await;
	assert_eq!(queued(&c), tagged(1..3));
	assert_eq!(flushed(&b).await, stored(&b));
	b.reset();
	drop(c);
	settle().await;
}

/// If the transaction that moves an orphan's records aborts, nothing moved: the orphan's
/// records are all still there, and the adopter holds none of them in memory. A later
/// pass adopts them.
#[wasm_bindgen_test]
async fn test_an_aborted_adoption_changes_nothing() {
	let db_name = "test-adopt-aborted";
	let Some(b) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let mut a = WebStore::new(config(db_name, 4096, 1000)).await;
	append_all(&mut a, &tagged(0..4));
	drop(a);
	// Occupy the key B's second adopted item would get: its add fails, which aborts the
	// whole transaction, after the first was queued in memory.
	let blocker = idb::add(&b.db(), b.tab(), 2, "\"blocker\"").unwrap();
	idb::completion(&blocker).await.unwrap();
	warnings();
	b.adopt_now().await;
	assert!(!b.has_data(), "{:?}", queued(&b));
	assert!(warnings()
		.iter()
		.any(|w| w.starts_with("Adopting the items of tab")));
	assert_eq!(records(&b).await.len(), 5);

	// With the blocker gone, the next pass adopts them all, under fresh numbers.
	let gone = idb::delete(&b.db(), b.tab(), &[2]).unwrap();
	idb::completion(&gone).await.unwrap();
	b.adopt_now().await;
	assert_eq!(queued(&b), tagged(0..4));
	assert_eq!(flushed(&b).await, stored(&b));
	assert_eq!(records(&b).await.len(), 4);
	let mut b = b;
	b.reset();
	settle().await;
}
