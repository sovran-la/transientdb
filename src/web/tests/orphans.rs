//! Adopting orphans: the items of a store that is gone are taken over by the next pass of
//! a live one, in their order, behind its own items, re-keyed under its tab and deleted
//! from the orphan's.

use super::*;
use contract::{append_all, tagged};

#[wasm_bindgen_test]
async fn test_a_dropped_stores_items_are_adopted_at_the_next_pass() {
	let db_name = "test-orphans-adopt";
	let Some(mut b) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let mut a = WebStore::new(config(db_name, 4096, 1000)).await;
	let a_tab = a.tab().to_owned();
	append_all(&mut a, &tagged(0..6));
	// Some removed, some pending: what is left is adopted, all of it available.
	let first = a.fetch(2, 4096).unwrap().unwrap();
	a.remove(first).unwrap();
	let _pending = a.fetch(2, 4096).unwrap().unwrap();
	append_all(&mut b, &[json!("b's own")]);
	drop(a);

	assert_eq!(queued(&b), vec![json!("b's own")]);
	b.adopt_now().await;
	// Behind B's own item, in A's order, renumbered after it.
	let expected = [vec![json!("b's own")], tagged(2..6)].concat();
	assert_eq!(queued(&b), expected);
	let seqs: Vec<u64> = stored(&b).iter().map(|s| s.0).collect();
	assert_eq!(seqs, (1..=5).collect::<Vec<_>>());
	assert_eq!(available(&mut b), expected);
	// A's records are gone; every record is B's, as B holds them.
	assert_eq!(tabs(&b).await, vec![b.tab().to_owned()]);
	assert!(!tabs(&b).await.contains(&a_tab));
	assert_eq!(flushed(&b).await, stored(&b));

	// Adopted items are B's own now: removed through B, they are gone.
	let all = b.fetch(10, 4096).unwrap().unwrap();
	b.remove(all).unwrap();
	assert!(records(&b).await.is_empty());
	b.adopt_now().await;
	assert!(!b.has_data());
	b.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_a_new_store_adopts_before_new_returns() {
	let db_name = "test-orphans-at-open";
	let Some(mut a) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut a, &tagged(0..3));
	drop(a);
	// No settle and no pass called: `new` adopted before returning.
	let mut b = WebStore::new(config(db_name, 4096, 1000)).await;
	assert_eq!(b.shared.passes.get(), 1);
	assert_eq!(queued(&b), tagged(0..3));
	assert_eq!(flushed(&b).await, stored(&b));
	b.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_several_orphans_are_adopted_in_one_pass_each_in_order() {
	let db_name = "test-orphans-several";
	let Some(mut b) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let mut expected: Vec<Vec<Value>> = Vec::new();
	// All alive at once, so none adopts another's items when it opens.
	let mut orphans = Vec::new();
	for t in 0..4 {
		let mut orphan = WebStore::new(config(db_name, 4096, 1000)).await;
		let items: Vec<Value> = (0..5).map(|i| json!({"orphan": t, "i": i})).collect();
		append_all(&mut orphan, &items);
		expected.push(items);
		orphans.push(orphan);
	}
	drop(orphans);
	b.adopt_now().await;
	assert_eq!(b.shared.passes.get(), 2);

	// Orphans come in the order of their tab ids, which are random, but each one's items
	// stay together and in order.
	let got = queued(&b);
	assert_eq!(got.len(), 20);
	for chunk in got.chunks(5) {
		let t = chunk[0]["orphan"].as_u64().unwrap() as usize;
		assert_eq!(chunk, expected[t].as_slice());
	}
	let mut seen: Vec<u64> = got.iter().map(|v| v["orphan"].as_u64().unwrap()).collect();
	seen.dedup();
	seen.sort();
	assert_eq!(seen, vec![0, 1, 2, 3]);
	assert_eq!(tabs(&b).await, vec![b.tab().to_owned()]);
	assert_eq!(flushed(&b).await, stored(&b));
	b.reset();
	settle().await;
}

/// A reload is a new tab: it adopts what the page it replaces left, and its own items go
/// behind them only if appended after.
#[wasm_bindgen_test]
async fn test_adopted_items_are_queued_behind_the_tabs_own() {
	let db_name = "test-orphans-behind";
	let Some(mut b) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let mut a = WebStore::new(config(db_name, 4096, 1000)).await;
	append_all(&mut a, &tagged(0..3));
	append_all(&mut b, &tagged(10..13));
	let held = b.fetch(1, 4096).unwrap().unwrap();
	drop(a);
	b.adopt_now().await;
	append_all(&mut b, &tagged(20..21));
	assert_eq!(
		queued(&b),
		[tagged(10..13), tagged(0..3), tagged(20..21)].concat()
	);
	// Fetches go oldest first: B's own, then the adopted ones.
	let next = b.fetch(3, 4096).unwrap().unwrap();
	assert_eq!(next.values(), [tagged(11..13), tagged(0..1)].concat());
	drop((held, next));
	assert_eq!(flushed(&b).await, stored(&b));
	b.reset();
	settle().await;
}

/// Adopted items do not come back: once adopted, they are the adopter's, and adopting
/// again finds nothing more, from the same store or another.
#[wasm_bindgen_test]
async fn test_adopting_again_finds_nothing_more() {
	let db_name = "test-orphans-once";
	let Some(mut a) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut a, &tagged(0..4));
	drop(a);
	let b = WebStore::new(config(db_name, 4096, 1000)).await;
	let c = WebStore::new(config(db_name, 4096, 1000)).await;
	for _ in 0..3 {
		b.adopt_now().await;
		c.adopt_now().await;
	}
	assert_eq!(queued(&b), tagged(0..4));
	assert!(!c.has_data());
	assert_eq!(records(&b).await.len(), 4);
	drop((b, c));
	settle().await;
	fresh_store(db_name, 4096, 1000).await;
}
