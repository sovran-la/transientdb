//! Two live stores on one database, as two tabs of a page: each keeps, fetches and
//! removes only its own items, their keys never collide, and neither adopts the other's
//! while it lives.

use super::*;
use contract::{append_all, tagged};

fn items(tag: &str, range: std::ops::Range<usize>) -> Vec<Value> {
	range.map(|i| json!({ "tab": tag, "i": i })).collect()
}

/// No IndexedDB write has failed since the warnings were last taken.
fn assert_no_failed_writes() {
	let failed: Vec<String> = warnings()
		.into_iter()
		.filter(|w| w.starts_with("IndexedDB"))
		.collect();
	assert!(failed.is_empty(), "{failed:?}");
}

#[wasm_bindgen_test]
async fn test_two_instances_keep_to_their_own_items() {
	let db_name = "test-tabs-own-items";
	let Some(mut a) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let mut b = WebStore::new(config(db_name, 4096, 1000)).await;
	assert!(b.is_persisted());
	assert_ne!(a.tab(), b.tab());
	warnings();

	// Interleaved appends: both give out the same sequence numbers, under their own tabs.
	for i in 0..20 {
		a.append(items("a", i..i + 1).remove(0)).unwrap();
		b.append(items("b", i..i + 1).remove(0)).unwrap();
	}
	assert_eq!(
		stored(&a).iter().map(|s| s.0).collect::<Vec<_>>(),
		(1..=20).collect::<Vec<_>>()
	);
	assert_eq!(
		stored(&b).iter().map(|s| s.0).collect::<Vec<_>>(),
		(1..=20).collect::<Vec<_>>()
	);
	assert_eq!(flushed(&a).await, stored(&a));
	assert_eq!(flushed(&b).await, stored(&b));
	assert_eq!(records(&a).await.len(), 40);
	assert_no_failed_writes();

	// No shared fetch: each hands out its own items only, all of them.
	let from_a = a.fetch(100, 1 << 20).unwrap().unwrap();
	let from_b = b.fetch(100, 1 << 20).unwrap().unwrap();
	assert_eq!(from_a.values(), items("a", 0..20));
	assert_eq!(from_b.values(), items("b", 0..20));

	// A remove deletes the remover's records only, though the seqs are the same.
	a.remove(from_a).unwrap();
	assert!(flushed(&a).await.is_empty());
	assert_eq!(persisted_values(&b).await, items("b", 0..20));
	b.remove(from_b).unwrap();
	assert!(records(&b).await.is_empty());
	assert_no_failed_writes();
	a.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_a_live_owners_items_are_never_adopted() {
	let db_name = "test-tabs-live-owner";
	let Some(mut a) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut a, &items("a", 0..5));
	// Opened while A lives, and passing again and again: nothing of A's is taken.
	let mut b = WebStore::new(config(db_name, 4096, 1000)).await;
	for _ in 0..3 {
		b.adopt_now().await;
	}
	assert!(!b.has_data());
	assert!(b.fetch(10, 4096).unwrap().is_none());
	assert_eq!(queued(&a), items("a", 0..5));
	assert_eq!(flushed(&a).await, stored(&a));
	assert_eq!(tabs(&a).await, vec![a.tab().to_owned()]);

	// Even with items pending in A.
	let held = a.fetch(2, 4096).unwrap().unwrap();
	b.adopt_now().await;
	assert!(!b.has_data());
	a.remove(held).unwrap();
	assert_eq!(persisted_values(&a).await, items("a", 2..5));

	// Once A is gone, B takes what is left.
	drop(a);
	b.adopt_now().await;
	assert_eq!(queued(&b), items("a", 2..5));
	b.reset();
	settle().await;
}

/// `reset` clears every tab's records, as for a logout. Another live tab keeps what it
/// holds in memory, and goes on writing what it appends afterwards.
#[wasm_bindgen_test]
async fn test_reset_clears_every_tabs_records() {
	let db_name = "test-tabs-reset";
	let Some(mut a) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let mut b = WebStore::new(config(db_name, 4096, 1000)).await;
	let mut orphan = WebStore::new(config(db_name, 4096, 1000)).await;
	append_all(&mut a, &items("a", 0..3));
	append_all(&mut b, &items("b", 0..3));
	append_all(&mut orphan, &items("orphan", 0..3));
	drop(orphan);
	settle().await;

	a.reset();
	assert!(records(&a).await.is_empty());
	assert!(!a.has_data());
	// B still holds its items in memory, and can send them.
	assert_eq!(queued(&b), items("b", 0..3));
	let held = b.fetch(10, 4096).unwrap().unwrap();
	assert_eq!(held.values(), items("b", 0..3));
	b.remove(held).unwrap();
	// Nothing is left to adopt.
	b.adopt_now().await;
	assert!(!b.has_data());
	// And both go on persisting what they append.
	append_all(&mut a, &items("a", 3..4));
	append_all(&mut b, &items("b", 3..4));
	assert_eq!(persisted_values(&a).await, items("a", 3..4));
	assert_eq!(persisted_values(&b).await, items("b", 3..4));
	a.reset();
	settle().await;
}

/// `max_items` is per tab: each of two live tabs holds up to it.
#[wasm_bindgen_test]
async fn test_max_items_is_per_tab() {
	let db_name = "test-tabs-max-items";
	let Some(mut a) = persisted(db_name, 4096, 3).await else {
		return;
	};
	let mut b = WebStore::new(config(db_name, 4096, 3)).await;
	append_all(&mut a, &tagged(0..5));
	append_all(&mut b, &tagged(10..15));
	assert_eq!(queued(&a), tagged(2..5));
	assert_eq!(queued(&b), tagged(12..15));
	assert_eq!(flushed(&a).await, stored(&a));
	assert_eq!(flushed(&b).await, stored(&b));
	assert_eq!(records(&a).await.len(), 6);
	a.reset();
	settle().await;
}

/// A store holds its liveness lock from the moment `new` returns, before it writes
/// anything, until it is dropped.
#[wasm_bindgen_test]
async fn test_a_store_holds_its_lock_from_open_until_dropped() {
	let db_name = "test-tabs-lock";
	let Some(store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let name = lock::name(db_name, &store.tab());
	assert!(lock::acquire(&name, true).await.unwrap().is_none());
	drop(store);
	for _ in 0..100 {
		if lock::acquire(&name, true).await.unwrap().is_some() {
			return;
		}
		gloo_timers::future::TimeoutFuture::new(5).await;
	}
	panic!("a dropped store's lock was not released");
}
