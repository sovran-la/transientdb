//! When the `pageshow` listener acts: only on a restore from the back/forward cache, only
//! for a store that opened persisted and still is, and never once the store is dropped.

use super::no_locks::NoLocks;
use super::*;
use contract::{append_all, tagged};
use std::cell::Cell;
use std::rc::Rc;

#[wasm_bindgen_test]
async fn test_a_pageshow_that_is_not_a_restore_changes_nothing() {
	let db_name = "test-bfcache-not-persisted";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut store, &tagged(0..4));
	let items = store.fetch(2, 4096).unwrap().unwrap();
	let (tab, passes) = (store.tab(), store.shared.passes.get());
	let seen = pageshows();
	pageshow(false);
	settle().await;
	assert_eq!(pageshows(), seen + 1);
	assert_eq!(store.tab(), tab);
	assert_eq!(store.shared.passes.get(), passes);
	assert!(lock_held(db_name, &tab).await);
	assert_eq!(queued(&store), tagged(0..4));
	// What was handed out before still counts.
	store.remove(items).unwrap();
	assert_eq!(queued(&store), tagged(2..4));
	assert_eq!(flushed(&store).await, stored(&store));
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_a_memory_only_store_ignores_pageshow() {
	warnings();
	let hidden = NoLocks::hide();
	let mut store = WebStore::new(config("test-bfcache-memory-only", 4096, 1000)).await;
	drop(hidden);
	warnings();
	assert!(!store.is_persisted() && store.pageshow.is_none());
	append_all(&mut store, &tagged(0..3));
	let tab = store.tab();
	let seen = pageshows();
	pageshow(true);
	settle().await;
	assert_eq!(pageshows(), seen);
	assert_eq!(store.tab(), tab);
	assert_eq!(queued(&store), tagged(0..3));
}

/// A store whose connection closed for another page's upgrade holds its items in memory
/// only: starting over would lose them, so it carries on as it is.
#[wasm_bindgen_test]
async fn test_a_store_whose_connection_closed_ignores_a_restore() {
	let db_name = "test-bfcache-closed";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut store, &tagged(0..3));
	store.db().close();
	store.shared.closed.set(true);
	let (tab, passes) = (store.tab(), store.shared.passes.get());
	pageshow(true);
	settle().await;
	assert_eq!(store.tab(), tab);
	assert_eq!(queued(&store), tagged(0..3));
	assert_eq!(store.shared.passes.get(), passes);
	drop(store);
	fresh_store(db_name, 4096, 1000).await;
	settle().await;
}

#[wasm_bindgen_test]
async fn test_dropping_a_store_removes_its_listener() {
	let Some(store) = persisted("test-bfcache-listener", 4096, 1000).await else {
		return;
	};
	let seen = pageshows();
	pageshow(false);
	assert_eq!(pageshows(), seen + 1);
	drop(store);
	// A listener left behind would call a freed closure, which throws: reported as an
	// `error` event on the window.
	let errors = Rc::new(Cell::new(0));
	let on_error = Closure::<dyn FnMut(web_sys::Event)>::new({
		let errors = errors.clone();
		move |_: web_sys::Event| errors.set(errors.get() + 1)
	});
	let window = web_sys::window().unwrap();
	let listener = on_error.as_ref().unchecked_ref();
	window
		.add_event_listener_with_callback("error", listener)
		.unwrap();
	pageshow(true);
	pageshow(false);
	window
		.remove_event_listener_with_callback("error", listener)
		.unwrap();
	assert_eq!(pageshows(), seen + 1);
	assert_eq!(errors.get(), 0);
}
