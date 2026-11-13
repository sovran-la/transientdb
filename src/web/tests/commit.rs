//! Adopted items are queued only once the move that re-keys them has committed. Each test
//! calls the store, as app code on a timer would, at the end of the move's handler: every
//! write is issued and the transaction has not committed yet.

use super::super::takeover::BEFORE_COMMIT;
use super::*;
use contract::{append_all, tagged};
use std::cell::RefCell;
use std::rc::Rc;

/// A dead tab on `db_name` holding `items`; returns its tab id.
async fn orphan(db_name: &str, items: &[Value]) -> String {
	let mut store = WebStore::new(config(db_name, 4096, 1000)).await;
	append_all(&mut store, items);
	store.tab()
}

/// Runs one adoption pass of `store`, calling `app` with the store and the move's
/// transaction once the first move's writes are issued, before it commits.
async fn pass_calling(
	store: WebStore,
	app: impl FnOnce(&mut WebStore, &web_sys::IdbTransaction) + 'static,
) -> WebStore {
	let store = Rc::new(RefCell::new(store));
	let shared = store.borrow().shared.clone();
	BEFORE_COMMIT.set(Some(Box::new({
		let store = store.clone();
		move |transaction| app(&mut store.borrow_mut(), transaction)
	})));
	adopt::pass(&shared).await;
	assert!(
		BEFORE_COMMIT.with_borrow(Option::is_none),
		"no move reached its commit"
	);
	let Ok(store) = Rc::try_unwrap(store) else {
		panic!("the hook kept the store");
	};
	store.into_inner()
}

/// What the app saw: whether it had data, and what a fetch handed out.
type Seen = Rc<RefCell<Option<(bool, Option<Vec<Value>>)>>>;

#[wasm_bindgen_test]
async fn test_adopted_items_are_fetchable_only_once_the_move_commits() {
	let db_name = "test-commit-fetchable";
	let Some(b) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let dead = orphan(db_name, &tagged(0..3)).await;
	let seen: Seen = Rc::default();
	let mut b = pass_calling(b, {
		let seen = seen.clone();
		move |b, _| {
			let has_data = b.has_data();
			let fetched = b.fetch(100, 1 << 20).unwrap().map(|items| items.values());
			// Appended while the move is in flight: its number comes after theirs.
			b.append(json!("own")).unwrap();
			*seen.borrow_mut() = Some((has_data, fetched));
		}
	})
	.await;
	assert_eq!(
		seen.take(),
		Some((false, None)),
		"fetchable before the commit"
	);
	let all = [tagged(0..3), vec![json!("own")]].concat();
	assert_eq!(queued(&b), all);
	assert!(b.has_data());
	assert_eq!(available(&mut b), all);
	assert_eq!(flushed(&b).await, stored(&b));
	assert!(values_under(&b, &dead).await.is_empty());
	b.reset();
	settle().await;
}

/// A move that aborts after its writes were issued (a commit-time failure, such as a full
/// disk) hands out nothing: the app cannot have sent an item that the abort gives back to
/// the dead tab, so a later pass adopts each exactly once.
#[wasm_bindgen_test]
async fn test_an_abort_after_the_writes_hands_out_nothing() {
	let db_name = "test-commit-abort";
	let Some(b) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let dead = orphan(db_name, &tagged(0..4)).await;
	let sent = Rc::new(RefCell::new(Vec::new()));
	let seen: Seen = Rc::default();
	warnings();
	let mut b = pass_calling(b, {
		let (sent, seen) = (sent.clone(), seen.clone());
		move |b, transaction| {
			let has_data = b.has_data();
			// Fetched, sent and removed, as the app would.
			let fetched = b.fetch(100, 1 << 20).unwrap();
			let values = fetched.as_ref().map(Items::values);
			if let Some(items) = fetched {
				sent.borrow_mut().extend(items.values());
				b.remove(items).unwrap();
			}
			*seen.borrow_mut() = Some((has_data, values));
			transaction.abort().unwrap();
		}
	})
	.await;
	assert_eq!(
		seen.take(),
		Some((false, None)),
		"handed out before the commit"
	);
	assert!(warnings()
		.iter()
		.any(|w| w.starts_with(&format!("Adopting the items of tab {dead} failed"))));
	assert!(!b.has_data(), "{:?}", queued(&b));
	assert_eq!(values_under(&b, &dead).await, tagged(0..4), "rolled back");
	assert!(flushed(&b).await.is_empty());

	b.adopt_now().await;
	assert_eq!(queued(&b), tagged(0..4));
	let delivered = [sent.take(), available(&mut b)].concat();
	assert_eq!(delivered, tagged(0..4), "each item exactly once");
	assert!(values_under(&b, &dead).await.is_empty());
	assert_eq!(flushed(&b).await, stored(&b));
	b.reset();
	settle().await;
}

/// A reset after the move's writes, before its commit: the reset's clear, ordered after
/// the move, deletes what it wrote, and nothing is queued.
#[wasm_bindgen_test]
async fn test_a_reset_before_the_commit_adopts_nothing() {
	let db_name = "test-commit-reset";
	let Some(b) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	orphan(db_name, &tagged(0..3)).await;
	let mut b = pass_calling(b, |b, _| b.reset()).await;
	assert!(queued(&b).is_empty());
	assert!(records(&b).await.is_empty());
	// The store goes on normally.
	append_all(&mut b, &tagged(5..6));
	assert_eq!(queued(&b), tagged(5..6));
	assert_eq!(persisted_values(&b).await, tagged(5..6));
	b.reset();
	settle().await;
}

/// Restored from the back/forward cache after the move's writes, before its commit: the
/// records are moved under the old tab, which is an orphan from then on, and nothing is
/// queued for the new tab until its own pass adopts them, once.
#[wasm_bindgen_test]
async fn test_a_restore_before_the_commit_queues_nothing_for_the_new_tab() {
	let db_name = "test-commit-restore";
	let Some(b) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	orphan(db_name, &tagged(0..3)).await;
	let new = "commit-restore-tab".to_owned();
	let blocker = lock::acquire(&lock::name(db_name, &new), false)
		.await
		.unwrap()
		.unwrap();
	tab::NEXT_ID.set(Some(new.clone()));
	let old = b.tab();
	let mut b = pass_calling(b, |_, _| pageshow(true)).await;
	assert_eq!(b.tab(), new);
	assert!(queued(&b).is_empty(), "{:?}", queued(&b));
	assert_eq!(values_under(&b, &old).await, tagged(0..3));
	assert!(values_under(&b, &new).await.is_empty());

	drop(blocker);
	started_over(&b).await;
	assert_eq!(queued(&b), tagged(0..3));
	assert_eq!(tabs(&b).await, vec![new]);
	assert_eq!(flushed(&b).await, stored(&b));
	b.reset();
	settle().await;
}
