//! Opening a `WebStore`'s database, and giving it up when another connection needs it.
//!
//! An IndexedDB upgrade waits until every other connection to the database has closed.
//! Two rules keep a store from waiting on others for more than about a second, and from
//! ever making them wait on it:
//!
//! - The store's connection closes itself as soon as a newer version, or a delete, of
//!   its database is requested elsewhere (`versionchange`). The store then keeps working
//!   in memory only.
//! - Opening gives up if it is still `blocked` after a short wait ([`BLOCKED_GRACE_MS`]).
//!   The wait lets a connection that is already closing finish: `close()` takes effect
//!   only once that connection's running transactions are done, and until then it blocks
//!   the upgrade. Once the upgrade has started, the wait is over: `success` or `error`
//!   follows when it commits or fails. A connection that does not close, such as a page
//!   loaded before a deploy that raised [`DB_VERSION`], holds the database until that
//!   page goes away, so waiting longer would only hang the caller. The store then works
//!   in memory only. The abandoned request is left harmless: if it later gets to
//!   upgrade, the upgrade is aborted, so the database stays exactly as it was for the
//!   next open; if it ever succeeds, the connection is closed at once.

use super::idb::{js_error, result_of, DB_VERSION, KEY_PATH, STORE_NAME};
use std::cell::{Cell, RefCell};
use std::io::{Error, Result};
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use web_sys::{IdbDatabase, IdbOpenDbRequest, IdbRequest};

fn warn(message: &str) {
	web_sys::console::warn_1(&message.into());
}

/// How long `open` still waits for `success` once the open is `blocked`, before it gives
/// up: long enough for a closing connection's last transactions to commit.
pub(super) const BLOCKED_GRACE_MS: i32 = 1000;

/// `Some` until `open` has its answer: success, failure, or giving up when blocked.
type Waiting = Rc<RefCell<Option<futures_channel::oneshot::Sender<Result<()>>>>>;

/// Gives `open` its answer, unless it has one already. Returns whether `open` was still
/// waiting for it: `false` too if the caller has dropped `open`'s future.
fn answer(waiting: &Waiting, outcome: Result<()>) -> bool {
	let sender = waiting.borrow_mut().take();
	sender.is_some_and(|sender| sender.send(outcome).is_ok())
}

/// Opens (or creates, or upgrades) the database `name` at [`DB_VERSION`].
///
/// Upgrading from an older version deletes every object store it has and creates the
/// current one empty: old records are discarded, not migrated. Fails, instead of
/// waiting any longer, if another connection still blocks the upgrade
/// [`BLOCKED_GRACE_MS`] after `blocked`.
pub(super) async fn open(name: &str) -> Result<IdbDatabase> {
	let window = web_sys::window().ok_or_else(|| Error::other("No window object"))?;
	let factory = window
		.indexed_db()
		.map_err(|e| js_error("IndexedDB error", e))?
		.ok_or_else(|| Error::other("IndexedDB not available"))?;
	let request = factory
		.open_with_f64(name, DB_VERSION as f64)
		.map_err(|e| js_error("Failed to open DB", e))?;

	let (sender, receiver) = futures_channel::oneshot::channel();
	let waiting: Waiting = Rc::new(RefCell::new(Some(sender)));
	// Set once this open's upgrade has started: the blockers are gone by then.
	let upgrading = Rc::new(Cell::new(false));

	let on_blocked = Closure::once({
		let (waiting, upgrading) = (waiting.clone(), upgrading.clone());
		move |_event: web_sys::Event| {
			// `success`, or `error`, answers first if the blocking connection closes in time.
			wasm_bindgen_futures::spawn_local(async move {
				delay(BLOCKED_GRACE_MS).await;
				if upgrading.get() {
					// The upgrade may still be committing; its `success` or `error` answers.
					return;
				}
				let gave_up = answer(
					&waiting,
					Err(Error::other("blocked by an open older version")),
				);
				if gave_up {
					warn(
						"IndexedDB upgrade blocked: another page holds the database open at an \
						 older version. Using memory-only storage.",
					);
				}
			});
		}
	});
	let on_upgrade = Closure::once({
		let waiting = waiting.clone();
		move |event: web_sys::IdbVersionChangeEvent| {
			let Some(request) = event.target().map(|t| t.unchecked_into::<IdbRequest>()) else {
				return;
			};
			let outcome = match *waiting.borrow() {
				Some(_) => {
					upgrading.set(true);
					create_schema(&request)
				}
				None => Err("the open was given up when it was blocked".into()),
			};
			if let Err(e) = outcome {
				warn(&format!("IndexedDB upgrade not done: {e:?}"));
				// Aborting fails the open request and leaves the database as it was.
				if let Some(transaction) = request.transaction() {
					let _ = transaction.abort();
				}
			}
		}
	});
	let on_success = Closure::once({
		let waiting = waiting.clone();
		move |event: web_sys::Event| {
			if !answer(&waiting, Ok(())) {
				// Opened after `open` gave up: nobody will use this connection.
				let request = event.target().map(|t| t.unchecked_into::<IdbRequest>());
				if let Some(Ok(db)) = request.map(|r| r.result()) {
					db.unchecked_into::<IdbDatabase>().close();
				}
			}
		}
	});
	let on_error = Closure::once({
		let waiting = waiting.clone();
		move |_event: web_sys::Event| {
			answer(&waiting, Err(Error::other("IndexedDB request failed")));
		}
	});
	set_handlers(&request, [on_blocked, on_success, on_error], on_upgrade);

	let outcome = receiver.await;
	outcome.unwrap_or_else(|_| Err(Error::other("Channel closed")))?;
	result_of(&request)
}

/// Resolves after `ms` milliseconds, on a browser timer; at once if there is none.
///
/// A `setTimeout` promise rather than a `Closure`, so it adds no closure shape for
/// wasm-bindgen to export (see the note on `settle` in `tests/common/mod.rs`).
async fn delay(ms: i32) {
	let timer = js_sys::Promise::new(&mut |resolve, _reject| {
		let set = web_sys::window()
			.map(|w| w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms));
		if !matches!(set, Some(Ok(_))) {
			let _ = resolve.call0(&JsValue::UNDEFINED);
		}
	});
	// The promise only ever resolves.
	let _ = wasm_bindgen_futures::JsFuture::from(timer).await;
}

/// Attaches the open request's handlers, which live as long as the page: the request
/// may fire after `open` has returned.
fn set_handlers(
	request: &IdbOpenDbRequest,
	[on_blocked, on_success, on_error]: [Closure<dyn FnMut(web_sys::Event)>; 3],
	on_upgrade: Closure<dyn FnMut(web_sys::IdbVersionChangeEvent)>,
) {
	request.set_onblocked(Some(on_blocked.as_ref().unchecked_ref()));
	request.set_onupgradeneeded(Some(on_upgrade.as_ref().unchecked_ref()));
	request.set_onsuccess(Some(on_success.as_ref().unchecked_ref()));
	request.set_onerror(Some(on_error.as_ref().unchecked_ref()));
	for handler in [on_blocked, on_success, on_error] {
		handler.forget();
	}
	on_upgrade.forget();
}

/// Replaces whatever object stores an older version left with an empty current one.
fn create_schema(request: &IdbRequest) -> std::result::Result<(), JsValue> {
	let db: IdbDatabase = request.result()?.unchecked_into();
	let names = db.object_store_names();
	let old: Vec<String> = (0..names.length()).filter_map(|i| names.item(i)).collect();
	for name in old {
		db.delete_object_store(&name)?;
	}
	let params = web_sys::IdbObjectStoreParameters::new();
	params.set_key_path(&JsValue::from_str(KEY_PATH));
	db.create_object_store_with_optional_parameters(STORE_NAME, &params)?;
	Ok(())
}

/// Makes `db` close itself when a newer version, or a delete, of its database is
/// requested elsewhere, so it never blocks that. Returns the flag that is set once it has.
pub(super) fn close_on_versionchange(db: &IdbDatabase) -> Rc<Cell<bool>> {
	let closed = Rc::new(Cell::new(false));
	let on_versionchange = Closure::once({
		let (db, closed) = (db.clone(), closed.clone());
		move |_event: web_sys::IdbVersionChangeEvent| {
			db.close();
			closed.set(true);
			warn(
				"IndexedDB connection closed: another page needs a newer version of the \
				 database. Using memory-only storage.",
			);
		}
	});
	db.set_onversionchange(Some(on_versionchange.as_ref().unchecked_ref()));
	on_versionchange.forget();
	closed
}

#[cfg(test)]
mod tests {
	use super::*;
	use wasm_bindgen_test::wasm_bindgen_test;

	fn waiting() -> (Waiting, futures_channel::oneshot::Receiver<Result<()>>) {
		let (sender, receiver) = futures_channel::oneshot::channel();
		(Rc::new(RefCell::new(Some(sender))), receiver)
	}

	#[wasm_bindgen_test]
	fn test_answer_is_given_once() {
		let (waiting, mut receiver) = waiting();
		assert!(answer(&waiting, Ok(())));
		assert!(!answer(&waiting, Err(Error::other("late"))));
		assert!(matches!(receiver.try_recv(), Ok(Some(Ok(())))));
	}

	/// A caller that dropped `open`'s future is not waiting: the connection a later
	/// `success` brings is closed rather than left open with no one to close it.
	#[wasm_bindgen_test]
	fn test_answer_reports_a_dropped_open_as_not_waiting() {
		let (waiting, receiver) = waiting();
		drop(receiver);
		assert!(!answer(&waiting, Ok(())));
		assert!(waiting.borrow().is_none());
	}
}
