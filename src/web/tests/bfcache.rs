//! A page restored from the back/forward cache starts over as a new tab: a new tab id and
//! lock, the old lock released, the queue reset in memory, and its old records adopted
//! again unless another tab took them while it was cached. Driven by synthetic `pageshow`
//! events; every store here is the only one in the window listening for them.

use super::*;
use contract::{append_all, tagged};

#[wasm_bindgen_test]
async fn test_a_restored_page_starts_over_as_a_new_tab() {
	let db_name = "test-bfcache-new-tab";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let old = store.tab();
	assert!(lock_held(db_name, &old).await);
	let seen = pageshows();
	pageshow(true);
	assert_eq!(pageshows(), seen + 1);
	let new = store.tab();
	assert_ne!(new, old);
	started_over(&store).await;
	assert_eq!(store.tab(), new);
	assert!(lock_held(db_name, &new).await);
	// Chrome keeps a cached page's locks: the store releases the old one itself.
	until_free(db_name, &old).await;
	assert!(store.is_persisted());
	store.reset();
	settle().await;
}

/// Its own records come back, adopted from the old tab, when no other tab took them.
#[wasm_bindgen_test]
async fn test_own_records_are_adopted_again_when_no_one_took_them() {
	let db_name = "test-bfcache-readopt";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut store, &tagged(0..5));
	let sent = store.fetch(2, 4096).unwrap().unwrap();
	store.remove(sent).unwrap();
	let old = store.tab();
	pageshow(true);
	started_over(&store).await;
	assert_eq!(queued(&store), tagged(2..5));
	assert_eq!(available(&mut store), tagged(2..5));
	assert_eq!(tabs(&store).await, vec![store.tab()]);
	assert!(values_under(&store, &old).await.is_empty());
	assert_eq!(flushed(&store).await, stored(&store));
	store.reset();
	settle().await;
}

/// An `Items` handed out before the restore matches nothing afterwards: removing it
/// deletes nothing, and dropping it releases nothing.
#[wasm_bindgen_test]
async fn test_items_handed_out_before_a_restore_match_nothing() {
	let db_name = "test-bfcache-stale-items";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut store, &tagged(0..4));
	let first = store.fetch(2, 4096).unwrap().unwrap();
	let second = store.fetch(1, 4096).unwrap().unwrap();
	pageshow(true);
	started_over(&store).await;
	// All four, available again, under new sequence numbers.
	assert_eq!(queued(&store), tagged(0..4));
	let before = stored(&store);
	let held = store.fetch(1, 4096).unwrap().unwrap();
	store.remove(first).unwrap();
	drop(second);
	assert_eq!(stored(&store), before);
	assert_eq!(available(&mut store), tagged(1..4));
	assert_eq!(flushed(&store).await, before);
	// What it handed out since still counts.
	store.remove(held).unwrap();
	assert_eq!(queued(&store), tagged(1..4));
	assert_eq!(flushed(&store).await, stored(&store));
	store.reset();
	settle().await;
}

/// Safari releases a page's locks when it caches it, so another tab may adopt the page's
/// records meanwhile. Once restored, the page starts over empty: what the other tab
/// adopted is that tab's to send, and is not sent again from here.
#[wasm_bindgen_test]
async fn test_records_another_tab_adopted_while_cached_are_not_sent_again() {
	let db_name = "test-bfcache-safari-adopted";
	let Some(mut other) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	// Another page: it does not see this page's pageshow.
	drop(other.pageshow.take());
	let mut store = WebStore::new(config(db_name, 4096, 1000)).await;
	append_all(&mut store, &tagged(0..3));
	let in_flight = store.fetch(1, 4096).unwrap().unwrap();
	let old = store.tab();
	store.shared.liveness.borrow().as_ref().unwrap().lose();
	until_free(db_name, &old).await;
	other.adopt_now().await;
	assert_eq!(queued(&other), tagged(0..3));

	pageshow(true);
	started_over(&store).await;
	assert!(queued(&store).is_empty());
	store.remove(in_flight).unwrap();
	assert_eq!(queued(&other), tagged(0..3));
	assert_eq!(tabs(&store).await, vec![other.tab()]);
	assert_eq!(flushed(&other).await, stored(&other));
	// It goes on as a new tab.
	append_all(&mut store, &tagged(10..11));
	assert_eq!(values_under(&store, &store.tab()).await, tagged(10..11));
	drop(store);
	other.reset();
	settle().await;
}

/// Safari released the lock, but no other tab adopted anything: the page adopts its own
/// records again, as in Chrome.
#[wasm_bindgen_test]
async fn test_a_page_whose_lock_was_lost_adopts_its_own_records_again() {
	let db_name = "test-bfcache-safari-alone";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut store, &tagged(0..3));
	let old = store.tab();
	store.shared.liveness.borrow().as_ref().unwrap().lose();
	until_free(db_name, &old).await;
	warnings();
	pageshow(true);
	started_over(&store).await;
	assert_eq!(queued(&store), tagged(0..3));
	assert_eq!(tabs(&store).await, vec![store.tab()]);
	assert_eq!(flushed(&store).await, stored(&store));
	assert!(warnings().is_empty());
	store.reset();
	settle().await;
}

/// Until the new tab's lock is granted, nothing is written under it: appends are kept in
/// memory, and nothing is adopted. Once it is, they are written under the new tab.
#[wasm_bindgen_test]
async fn test_appends_wait_for_the_new_tabs_lock() {
	let db_name = "test-bfcache-pending-lock";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut store, &tagged(0..2));
	let old = store.tab();
	let new = "bfcache-pending-lock-tab".to_owned();
	let name = lock::name(db_name, &new);
	let blocker = lock::acquire(&name, false).await.unwrap().unwrap();
	tab::NEXT_ID.set(Some(new.clone()));
	pageshow(true);
	assert_eq!(store.tab(), new);
	append_all(&mut store, &tagged(10..13));
	let sent = store.fetch(1, 4096).unwrap().unwrap();
	store.remove(sent).unwrap();
	store.adopt_now().await;
	settle().await;
	assert!(store.shared.liveness.borrow().is_none());
	assert!(store.is_persisted());
	assert_eq!(queued(&store), tagged(11..13));
	assert!(values_under(&store, &new).await.is_empty());
	assert_eq!(values_under(&store, &old).await, tagged(0..2));

	drop(blocker);
	started_over(&store).await;
	assert_eq!(queued(&store), [tagged(11..13), tagged(0..2)].concat());
	assert_eq!(tabs(&store).await, vec![new]);
	assert_eq!(flushed(&store).await, stored(&store));
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_restoring_again_and_again() {
	let db_name = "test-bfcache-repeated";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let mut all = Vec::new();
	let mut seen = vec![store.tab()];
	for round in 0..4 {
		let items: Vec<Value> = (0..3).map(|i| json!({"round": round, "i": i})).collect();
		append_all(&mut store, &items);
		all.extend(items);
		let in_flight = store.fetch(1, 4096).unwrap().unwrap();
		pageshow(true);
		started_over(&store).await;
		drop(in_flight);
		assert!(!seen.contains(&store.tab()), "round {round}");
		seen.push(store.tab());
		assert_eq!(sorted(queued(&store)), sorted(all.clone()), "round {round}");
		assert_eq!(tabs(&store).await, vec![store.tab()], "round {round}");
		assert_eq!(flushed(&store).await, stored(&store), "round {round}");
	}
	for tab in &seen[..seen.len() - 1] {
		until_free(db_name, tab).await;
	}
	assert!(lock_held(db_name, &store.tab()).await);
	store.reset();
	settle().await;
}
