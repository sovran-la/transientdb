//! Without Web Locks (an insecure context, an old browser) the store works in memory only:
//! records it wrote could never be safely adopted.

use super::*;
use contract::{append_all, tagged};
use js_sys::{Object, Reflect};

/// Hides `navigator.locks` behind an own property that is `undefined`, until dropped.
pub(super) struct NoLocks(web_sys::Navigator);

impl NoLocks {
	pub(super) fn hide() -> Self {
		let navigator = web_sys::window().unwrap().navigator();
		let descriptor = Object::new();
		Reflect::set(&descriptor, &"value".into(), &JsValue::UNDEFINED).unwrap();
		Reflect::set(&descriptor, &"configurable".into(), &true.into()).unwrap();
		Object::define_property(&navigator, &"locks".into(), &descriptor);
		assert!(!lock::available());
		Self(navigator)
	}
}

impl Drop for NoLocks {
	fn drop(&mut self) {
		Reflect::delete_property(&self.0, &"locks".into()).unwrap();
		assert!(lock::available());
	}
}

#[wasm_bindgen_test]
async fn test_without_web_locks_the_store_is_memory_only() {
	let db_name = "test-no-locks";
	let Some(mut a) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut a, &tagged(0..2));
	drop(a);
	settle().await;

	warnings();
	let hidden = NoLocks::hide();
	let mut store = WebStore::new(config(db_name, 4096, 1000)).await;
	drop(hidden);
	assert_eq!(store.persistence_state(), PersistenceState::MemoryOnly);
	assert!(!store.is_persisted());
	let warned = warnings();
	assert_eq!(warned.len(), 1, "{warned:?}");
	assert!(
		warned[0]
			.starts_with("Web Locks (navigator.locks) unavailable, falling back to memory-only"),
		"{warned:?}"
	);
	// No connection, no lock, no adoption: the orphan is left alone.
	assert!(store.shared.db.is_none());
	assert!(store.shared.liveness.borrow().is_none());
	assert!(store.adoption.is_none() && store.pageshow.is_none());
	assert!(!store.has_data());
	store.adopt_now().await;
	assert!(!store.has_data());

	// It works in memory, and writes nothing.
	append_all(&mut store, &tagged(10..13));
	let items = store.fetch(2, 4096).unwrap().unwrap();
	assert_eq!(items.values(), tagged(10..12));
	store.remove(items).unwrap();
	assert_eq!(queued(&store), tagged(12..13));
	store.reset();
	assert!(!store.has_data());
	append_all(&mut store, &tagged(20..21));
	settle().await;
	// The reset reached no database either: the orphan is still there to adopt.
	assert_eq!(adopted(db_name).await, tagged(0..2));
	fresh_store(db_name, 4096, 1000).await;
	settle().await;
}
