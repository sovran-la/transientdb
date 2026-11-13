//! Starting over after a restore from the back/forward cache, against everything else
//! going on: a drop while it starts over, an adoption pass in flight, another restore
//! before the first finishes, and another tab adopting at the same time.

use super::*;
use contract::{append_all, tagged};

/// Lets `ticks` rounds of timers run.
async fn ticks(ticks: usize) {
	for _ in 0..ticks {
		gloo_timers::future::TimeoutFuture::new(0).await;
	}
}

/// Dropped before the new tab's lock is granted, the store writes nothing under it, and
/// the lock, granted later, is released at once. The old tab's records are an orphan.
#[wasm_bindgen_test]
async fn test_dropping_a_store_before_its_new_lock_is_granted() {
	let db_name = "test-bfcache-drop-pending";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut store, &tagged(0..3));
	let old = store.tab();
	let new = "bfcache-drop-pending-tab".to_owned();
	let name = lock::name(db_name, &new);
	let blocker = lock::acquire(&name, false).await.unwrap().unwrap();
	tab::NEXT_ID.set(Some(new.clone()));
	pageshow(true);
	append_all(&mut store, &tagged(10..12));
	drop(store);
	drop(blocker);
	until_free(db_name, &old).await;
	until_free(db_name, &new).await;
	let mut next = WebStore::new(config(db_name, 4096, 1000)).await;
	assert_eq!(queued(&next), tagged(0..3));
	assert_eq!(tabs(&next).await, vec![next.tab()]);
	next.reset();
	settle().await;
}

/// Dropped at any point while it starts over, the store loses none of its records: the
/// next store adopts each exactly once, from whichever tab they were under.
#[wasm_bindgen_test]
async fn test_dropping_a_store_while_it_starts_over_loses_nothing() {
	let db_name = "test-bfcache-drop";
	let Some(cleaner) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	drop(cleaner);
	for delay in 0..10 {
		let mut store = WebStore::new(config(db_name, 4096, 1000)).await;
		let items: Vec<Value> = (0..5).map(|i| json!({"delay": delay, "i": i})).collect();
		append_all(&mut store, &items);
		let old = store.tab();
		pageshow(true);
		let new = store.tab();
		ticks(delay).await;
		drop(store);
		// Both tabs' locks free, so that the next store can adopt from either.
		until_free(db_name, &old).await;
		until_free(db_name, &new).await;
		let mut next = WebStore::new(config(db_name, 4096, 1000)).await;
		assert_eq!(sorted(queued(&next)), sorted(items), "delay {delay}");
		assert_eq!(flushed(&next).await, stored(&next), "delay {delay}");
		assert_eq!(tabs(&next).await, vec![next.tab()], "delay {delay}");
		next.reset();
		drop(next);
		settle().await;
	}
}

/// A pass in flight when the page is restored adopts nothing into the reset queue; the
/// new tab adopts everything, its own old records and the orphans, exactly once.
#[wasm_bindgen_test]
async fn test_a_pass_in_flight_across_a_restore() {
	let db_name = "test-bfcache-pass-in-flight";
	let Some(mut b) = persisted(db_name, 4096, 1 << 20).await else {
		return;
	};
	for delay in 0..12 {
		append_all(&mut b, &[json!({"b": delay})]);
		let all = orphans(db_name, 3, 20).await;
		let pass = spawn_pass(&b);
		ticks(delay).await;
		pageshow(true);
		until_all(&[pass]).await;
		started_over(&b).await;
		let expected = [vec![json!({"b": delay})], all].concat();
		assert_eq!(sorted(queued(&b)), sorted(expected), "delay {delay}");
		assert_eq!(flushed(&b).await, stored(&b), "delay {delay}");
		assert_eq!(records(&b).await.len(), stored(&b).len(), "delay {delay}");
		assert_eq!(tabs(&b).await, vec![b.tab()], "delay {delay}");
		b.reset();
	}
	settle().await;
}

/// The same, with the new tab's lock held elsewhere until the pass in flight is over:
/// whatever that pass was moving when the page was restored, it writes nothing under the
/// new tab, and the queue stays as the restore left it.
#[wasm_bindgen_test]
async fn test_a_pass_in_flight_writes_nothing_before_the_new_lock() {
	let db_name = "test-bfcache-pass-pending-lock";
	let Some(mut b) = persisted(db_name, 4096, 1 << 20).await else {
		return;
	};
	for delay in 0..12 {
		let all = orphans(db_name, 3, 20).await;
		let new = format!("bfcache-pass-pending-lock-{delay}");
		let name = lock::name(db_name, &new);
		let blocker = lock::acquire(&name, false).await.unwrap().unwrap();
		tab::NEXT_ID.set(Some(new.clone()));
		let pass = spawn_pass(&b);
		ticks(delay).await;
		pageshow(true);
		until_all(&[pass]).await;
		settle().await;
		assert!(queued(&b).is_empty(), "delay {delay}");
		assert!(values_under(&b, &new).await.is_empty(), "delay {delay}");
		drop(blocker);
		started_over(&b).await;
		assert_eq!(sorted(queued(&b)), sorted(all), "delay {delay}");
		assert_eq!(flushed(&b).await, stored(&b), "delay {delay}");
		assert_eq!(tabs(&b).await, vec![new], "delay {delay}");
		b.reset();
	}
	settle().await;
}

/// Restored exactly while a move's reads are in flight: the move adopts nothing, writing
/// nothing under either tab, and leaves the orphan's records for the new tab to adopt.
#[wasm_bindgen_test]
async fn test_a_restore_between_a_moves_reads_and_their_results() {
	let db_name = "test-bfcache-between-reads";
	let Some(mut b) = persisted(db_name, 4096, 1 << 20).await else {
		return;
	};
	let all = orphans(db_name, 1, 20).await;
	let new = "bfcache-between-reads-tab".to_owned();
	let name = lock::name(db_name, &new);
	let blocker = lock::acquire(&name, false).await.unwrap().unwrap();
	tab::NEXT_ID.set(Some(new.clone()));
	takeover::BETWEEN_READS.set(Some(Box::new(|| pageshow(true))));
	let old = b.tab();
	b.adopt_now().await;
	assert!(takeover::BETWEEN_READS.with_borrow(Option::is_none));
	assert_eq!(b.tab(), new);
	settle().await;
	assert!(queued(&b).is_empty());
	assert!(values_under(&b, &new).await.is_empty());
	assert!(values_under(&b, &old).await.is_empty());
	assert_eq!(records(&b).await.len(), all.len());
	drop(blocker);
	started_over(&b).await;
	assert_eq!(queued(&b), all);
	assert_eq!(tabs(&b).await, vec![new]);
	assert_eq!(flushed(&b).await, stored(&b));
	b.reset();
	settle().await;
}

/// Restored again before the first restore's lock is granted: the first new tab is given
/// up, its lock released once granted, and the store ends up as the second.
#[wasm_bindgen_test]
async fn test_restored_again_before_starting_over_finishes() {
	let db_name = "test-bfcache-twice";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut store, &tagged(0..4));
	let old = store.tab();
	pageshow(true);
	let first = store.tab();
	pageshow(true);
	let second = store.tab();
	assert!(old != first && first != second && old != second);
	started_over(&store).await;
	assert_eq!(store.tab(), second);
	assert!(lock_held(db_name, &second).await);
	until_free(db_name, &first).await;
	until_free(db_name, &old).await;
	assert_eq!(queued(&store), tagged(0..4));
	assert_eq!(tabs(&store).await, vec![second]);
	assert_eq!(flushed(&store).await, stored(&store));
	store.reset();
	settle().await;
}

/// Safari released the lock of the cached page; another tab adopts while the page starts
/// over. Every item ends up in exactly one of them, whichever wins its old tab's lock.
#[wasm_bindgen_test]
async fn test_another_tab_adopting_while_a_page_starts_over() {
	let db_name = "test-bfcache-safari-race";
	let Some(mut other) = persisted(db_name, 4096, 1 << 20).await else {
		return;
	};
	drop(other.pageshow.take());
	for delay in 0..10 {
		let mut store = WebStore::new(config(db_name, 4096, 1 << 20)).await;
		let items: Vec<Value> = (0..20).map(|i| json!({"delay": delay, "i": i})).collect();
		append_all(&mut store, &items);
		store.shared.liveness.borrow().as_ref().unwrap().lose();
		let pass = spawn_pass(&other);
		ticks(delay).await;
		pageshow(true);
		until_all(&[pass]).await;
		started_over(&store).await;
		let (mine, theirs) = (queued(&store), queued(&other));
		assert!(mine.is_empty() || theirs.is_empty(), "delay {delay}: split");
		assert_eq!(
			sorted([mine, theirs].concat()),
			sorted(items),
			"delay {delay}"
		);
		assert_eq!(flushed(&store).await, stored(&store), "delay {delay}");
		assert_eq!(flushed(&other).await, stored(&other), "delay {delay}");
		assert_eq!(records(&other).await.len(), 20, "delay {delay}");
		drop(store);
		other.reset();
		settle().await;
	}
}

/// Restored while `new` runs its first pass: the store already listens, so it starts over
/// all the same, and adopts the orphan as the new tab.
#[wasm_bindgen_test]
async fn test_a_restore_during_the_first_pass() {
	let db_name = "test-bfcache-first-pass";
	let Some(cleaner) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	drop(cleaner);
	let all = orphans(db_name, 1, 10).await;
	let first = "bfcache-first-pass-tab".to_owned();
	tab::NEXT_ID.set(Some(first.clone()));
	takeover::BETWEEN_READS.set(Some(Box::new(|| pageshow(true))));
	let mut store = WebStore::new(config(db_name, 4096, 1000)).await;
	assert!(takeover::BETWEEN_READS.with_borrow(Option::is_none));
	assert_ne!(store.tab(), first);
	started_over(&store).await;
	assert_eq!(queued(&store), all);
	assert_eq!(tabs(&store).await, vec![store.tab()]);
	assert_eq!(flushed(&store).await, stored(&store));
	until_free(db_name, &first).await;
	store.reset();
	settle().await;
}
