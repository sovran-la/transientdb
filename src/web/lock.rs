//! Web Locks for `WebStore`: the liveness lock each store holds for its whole life, and
//! taking a dead tab's lock to adopt its records.
//!
//! A store's records are kept under its tab id, and the store holds the lock
//! [`name`]`(database, tab)` for as long as it lives. The browser releases a page's locks
//! when the page goes away (closed, crashed, discarded, reloaded), so "the store that
//! wrote tab T's records is gone" is exactly "T's lock can be acquired".
//!
//! Bound here rather than through `web-sys`, which marks `LockManager` unstable (it needs
//! `--cfg web_sys_unstable_apis`, which every crate depending on this one would have to
//! set). Only `navigator.locks.request(name, options, callback)` is used.

use futures_channel::oneshot;
use js_sys::{Function, Object, Promise, Reflect};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
extern "C" {
	type LockManager;

	#[wasm_bindgen(method, catch, js_name = request)]
	fn request(
		this: &LockManager,
		name: &str,
		options: &Object,
		callback: &Function,
	) -> Result<Promise, JsValue>;
}

/// The name of the liveness lock of the store with tab id `tab` on `database`.
pub(super) fn name(database: &str, tab: &str) -> String {
	format!("transientdb:{database}:tab:{tab}")
}

/// `navigator.locks`, if this context has it: Web Locks need a secure context (https or
/// localhost) and a browser from 2022 or later.
fn manager() -> Option<LockManager> {
	let navigator = web_sys::window()?.navigator();
	let locks = Reflect::get(&navigator, &JsValue::from_str("locks")).ok()?;
	locks.is_object().then(|| locks.unchecked_into())
}

/// Whether Web Locks are available here.
pub(super) fn available() -> bool {
	manager().is_some()
}

/// A granted lock. It is held until this is dropped.
pub(super) struct Held {
	/// Resolves the promise the lock's callback returned, which releases the lock.
	release: Function,
}

impl Drop for Held {
	fn drop(&mut self) {
		let _ = self.release.call0(&JsValue::UNDEFINED);
	}
}

#[cfg(test)]
impl Held {
	/// Releases the lock while this still claims to hold it, as Safari does to a page it
	/// puts in the back/forward cache. Dropping this afterwards changes nothing.
	pub(super) fn lose(&self) {
		let _ = self.release.call0(&JsValue::UNDEFINED);
	}
}

/// What a request comes to: the lock, `None` if it was asked for only if available and is
/// held elsewhere, or why the request failed.
type Grant = std::result::Result<Option<Held>, String>;

/// Requests the lock `name`. With `if_available`, answers `None` at once if it is held
/// elsewhere; otherwise waits until it is granted.
///
/// If the caller drops the returned future before the answer, a lock granted later is
/// released at once.
pub(super) async fn acquire(name: &str, if_available: bool) -> Grant {
	let manager = manager().ok_or("Web Locks are unavailable")?;
	let (sender, receiver) = oneshot::channel::<Grant>();
	let sender = Rc::new(RefCell::new(Some(sender)));

	let granted = sender.clone();
	// Frees itself once called; the lock manager calls it at most once.
	let callback = Closure::once_into_js(move |lock: JsValue| -> JsValue {
		let Some(sender) = granted.borrow_mut().take() else {
			return JsValue::UNDEFINED;
		};
		if lock.is_null() {
			let _ = sender.send(Ok(None));
			return JsValue::UNDEFINED;
		}
		let mut release = None;
		let held = Promise::new(&mut |resolve, _reject| release = Some(resolve));
		let lock = Held {
			release: release.expect("a Promise runs its executor at once"),
		};
		// Not waited for any more: dropping the lock releases it.
		drop(sender.send(Ok(Some(lock))));
		held.into()
	});

	let options = Object::new();
	Reflect::set(&options, &"ifAvailable".into(), &if_available.into())
		.map_err(|e| format!("{e:?}"))?;
	let request = manager
		.request(name, &options, callback.unchecked_ref())
		.map_err(|e| format!("{e:?}"))?;
	// The request's promise settles once the lock is released, or rejects if the request
	// fails, in which case the callback is never called: answer with the failure.
	wasm_bindgen_futures::spawn_local(async move {
		if let Err(e) = JsFuture::from(request).await {
			if let Some(sender) = sender.borrow_mut().take() {
				let _ = sender.send(Err(format!("{e:?}")));
			}
		}
	});
	receiver
		.await
		.unwrap_or_else(|_| Err("the lock request was dropped".into()))
}

#[cfg(test)]
mod tests {
	use super::*;
	use wasm_bindgen_test::wasm_bindgen_test;

	#[wasm_bindgen_test]
	fn test_name_scopes_by_database_and_tab() {
		assert_eq!(name("events", "abc"), "transientdb:events:tab:abc");
		assert!(available(), "the test browsers have Web Locks");
	}

	#[wasm_bindgen_test]
	async fn test_a_held_lock_is_not_available_until_dropped() {
		let name = "transientdb-test:lock:held";
		let held = acquire(name, false).await.unwrap().unwrap();
		assert!(acquire(name, true).await.unwrap().is_none());
		drop(held);
		// Released once the lock manager sees the callback's promise settle: soon, but
		// not within this call.
		for _ in 0..100 {
			if acquire(name, true).await.unwrap().is_some() {
				return;
			}
			gloo_timers::future::TimeoutFuture::new(5).await;
		}
		panic!("a dropped lock was not released");
	}

	#[wasm_bindgen_test]
	async fn test_waiting_requests_are_granted_on_release() {
		let name = "transientdb-test:lock:wait";
		let held = acquire(name, false).await.unwrap().unwrap();
		let slot = Rc::new(RefCell::new(None));
		let out = slot.clone();
		wasm_bindgen_futures::spawn_local(async move {
			*out.borrow_mut() = Some(acquire(name, false).await.unwrap().is_some());
		});
		gloo_timers::future::TimeoutFuture::new(50).await;
		assert!(slot.borrow().is_none(), "granted while still held");
		drop(held);
		gloo_timers::future::TimeoutFuture::new(50).await;
		assert_eq!(*slot.borrow(), Some(true));
	}

	#[wasm_bindgen_test]
	async fn test_a_failed_request_is_an_error_not_a_hang() {
		// Lock names starting with '-' are reserved: the request rejects.
		let err = acquire("-reserved", true).await.err().unwrap();
		assert!(!err.is_empty());
	}
}
