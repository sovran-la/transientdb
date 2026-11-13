//! The windows inside an adoption, set up by construction rather than by timing: the
//! transaction that moves an orphan's records is created and its reads issued, then the
//! store is called before the reads come back, or between the move and the trim.

use super::super::takeover::{take_over, Taken};
use super::*;
use contract::{append_all, tagged};
use std::cell::RefCell;
use std::io::Result;
use std::rc::Rc;

/// A dead tab on `db_name` holding `items`; returns its tab key.
async fn orphan(db_name: &str, items: &[Value]) -> JsValue {
	let mut store = WebStore::new(config(db_name, 4096, 1 << 20)).await;
	append_all(&mut store, items);
	JsValue::from_str(&store.tab())
}

/// Starts `store` taking over `tab`'s records, and returns once the move's transaction
/// exists and its reads are issued, before they are answered. The slot is filled once it
/// is done.
async fn started(store: &WebStore, tab: JsValue) -> Rc<RefCell<Option<Result<Taken>>>> {
	let slot = Rc::new(RefCell::new(None));
	let (shared, out) = (store.shared.clone(), slot.clone());
	wasm_bindgen_futures::spawn_local(async move {
		let db = shared.db.clone().unwrap();
		let taken = take_over(&shared, &db, &tab, true).await;
		*out.borrow_mut() = Some(taken);
	});
	// The spawned task runs, up to its first await, in a microtask queued before this one.
	let resolved = js_sys::Promise::resolve(&JsValue::NULL);
	wasm_bindgen_futures::JsFuture::from(resolved)
		.await
		.unwrap();
	assert!(
		slot.borrow().is_none(),
		"the reads were answered within microtasks"
	);
	slot
}

async fn finished(slot: &Rc<RefCell<Option<Result<Taken>>>>) -> Result<Taken> {
	for _ in 0..400 {
		if let Some(taken) = slot.borrow_mut().take() {
			return taken;
		}
		gloo_timers::future::TimeoutFuture::new(5).await;
	}
	panic!("the take-over did not finish");
}

/// A reset issued after the move's transaction was created, but before its reads came
/// back, wins: nothing is adopted into memory, and the reset's clear, ordered after the
/// move, leaves nothing in IndexedDB.
#[wasm_bindgen_test]
async fn test_a_reset_before_the_reads_return_adopts_nothing() {
	let db_name = "test-windows-reset";
	let Some(mut b) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let tab = orphan(db_name, &tagged(0..5)).await;
	let slot = started(&b, tab).await;
	b.reset();
	let taken = finished(&slot).await.unwrap();
	assert_eq!(taken, Taken::default());
	assert!(!b.has_data(), "{:?}", queued(&b));
	assert!(records(&b).await.is_empty());
	// The store goes on normally.
	append_all(&mut b, &tagged(5..6));
	assert_eq!(persisted_values(&b).await, tagged(5..6));
	b.reset();
	settle().await;
}

/// A sync call made while the move's reads are outstanding runs normally, and the adopted
/// items land behind what it appended.
#[wasm_bindgen_test]
async fn test_calls_while_the_reads_are_outstanding_run_normally() {
	let db_name = "test-windows-calls";
	let Some(mut b) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let tab = orphan(db_name, &tagged(0..3)).await;
	let slot = started(&b, tab).await;
	append_all(&mut b, &[json!("own")]);
	let held = b.fetch(1, 4096).unwrap().unwrap();
	b.remove(held).unwrap();
	append_all(&mut b, &[json!("own 2")]);
	let taken = finished(&slot).await.unwrap();
	assert_eq!(taken.adopted, vec![3, 4, 5]);
	assert_eq!(queued(&b), [vec![json!("own 2")], tagged(0..3)].concat());
	assert_eq!(flushed(&b).await, stored(&b));
	assert_eq!(records(&b).await.len(), 4);
	b.reset();
	settle().await;
}

/// Between a move and the trim that ends a pass, the queue can hold more than
/// `max_items`. An append then drops everything past the cap, and deletes every one of
/// them from IndexedDB, not just the last.
#[wasm_bindgen_test]
async fn test_an_append_over_an_untrimmed_queue_deletes_all_it_drops() {
	let db_name = "test-windows-untrimmed";
	let Some(mut b) = persisted(db_name, 4096, 3).await else {
		return;
	};
	append_all(&mut b, &[json!("own")]);
	let tab = orphan(db_name, &tagged(0..6)).await;
	let slot = started(&b, tab).await;
	let taken = finished(&slot).await.unwrap();
	assert_eq!(taken.adopted.len(), 6);
	assert_eq!(queued(&b).len(), 7, "not trimmed yet");
	b.append(json!("new")).unwrap();
	assert_eq!(queued(&b), [tagged(4..6), vec![json!("new")]].concat());
	assert_eq!(flushed(&b).await, stored(&b));
	assert_eq!(records(&b).await.len(), 3);
	b.reset();
	settle().await;
}
