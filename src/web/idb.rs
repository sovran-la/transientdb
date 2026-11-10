//! IndexedDB plumbing for `WebStore`: the database schema, its record format, and the
//! requests the store issues.
//!
//! Every record wraps one queued item as `{ "key": <u32>, "value": "<item>", "batch": <u32> }`
//! in the object store [`STORE_NAME`], whose key path is `key` with no auto-increment.
//! The store assigns every key itself, so memory and IndexedDB always agree on an item's
//! key. `value` is the item serialized compactly with `serde_json`, as a string, and
//! hydration parses it back with `serde_json`: stored as a JavaScript value, an integer
//! above 2^53 would lose precision and `42.0` would come back as `42`. `batch` is the
//! batch number the item was appended under, so `end_batch` boundaries survive a reload.
//! `key` and `batch` are `u32`, which a JavaScript number holds exactly.
//!
//! One more record may exist, `{ "key": "boundary", "batch": <u32> }`: the batch number
//! the store moved to at its last `end_batch`. It keeps a boundary set after the last
//! append across a reload, when no item carries the new number yet. Its string key sorts
//! after every numeric key, so reading the object store returns it after the items.
//!
//! Writes are issued synchronously by the caller and only their outcome is awaited
//! in the background. IndexedDB runs read/write transactions on the same object store
//! in the order they were created, so effects land in the order of the calls that
//! issued them, whatever the executor does.

use serde_json::{json, Value};
use std::cell::RefCell;
use std::future::Future;
use std::io::{Error, Result};
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{IdbDatabase, IdbObjectStore, IdbRequest, IdbTransaction, IdbTransactionMode};

/// Version 1 stored items in-line under an auto-incremented `_idb_key`. Version 2
/// stores wrapper records keyed by the store's own counter. Version 3 adds each item's
/// batch number, and the boundary record. Version 4 stores each item as JSON text.
pub(super) const DB_VERSION: u32 = 4;
pub(super) const STORE_NAME: &str = "events";
const KEY_PATH: &str = "key";
const VALUE_FIELD: &str = "value";
const BATCH_FIELD: &str = "batch";
/// The key of the boundary record; every item's key is a number.
pub(super) const BOUNDARY_KEY: &str = "boundary";

fn js_error(what: &str, e: JsValue) -> Error {
	Error::other(format!("{what}: {e:?}"))
}

/// The record stored for the item `value` under `key`, appended under `batch`: the item
/// is held as its compact JSON text.
pub(super) fn record(key: u32, batch: u32, value: &Value) -> Value {
	json!({ (KEY_PATH): key, (VALUE_FIELD): value.to_string(), (BATCH_FIELD): batch })
}

/// The key of a record, if it has one this store could have assigned.
pub(super) fn record_key(record: &Value) -> Option<u32> {
	let key = record.get(KEY_PATH)?.as_u64()?;
	u32::try_from(key).ok()
}

/// The batch number of a record, if it has one this store could have assigned.
pub(super) fn record_batch(record: &Value) -> Option<u32> {
	let batch = record.get(BATCH_FIELD)?.as_u64()?;
	u32::try_from(batch).ok()
}

/// The item a record wraps, if the record has the wrapper shape: a key, a batch number
/// and a value that is a string holding JSON text.
pub(super) fn record_value(record: &Value) -> Option<Value> {
	record_key(record)?;
	record_batch(record)?;
	serde_json::from_str(record.get(VALUE_FIELD)?.as_str()?).ok()
}

/// Whether a record is the boundary record, valid or not.
pub(super) fn is_boundary(record: &Value) -> bool {
	record.get(KEY_PATH).and_then(Value::as_str) == Some(BOUNDARY_KEY)
}

/// Opens (or creates, or upgrades) the database `name` at [`DB_VERSION`].
///
/// Upgrading from an older version deletes its object store and creates the new one
/// empty: old records are discarded, not migrated.
pub(super) async fn open(name: &str) -> Result<IdbDatabase> {
	let window = web_sys::window().ok_or_else(|| Error::other("No window object"))?;
	let factory = window
		.indexed_db()
		.map_err(|e| js_error("IndexedDB error", e))?
		.ok_or_else(|| Error::other("IndexedDB not available"))?;
	let open_request = factory
		.open_with_f64(name, DB_VERSION as f64)
		.map_err(|e| js_error("Failed to open DB", e))?;

	let on_upgrade = Closure::once(move |event: web_sys::IdbVersionChangeEvent| {
		let Some(request) = event.target().map(|t| t.unchecked_into::<IdbRequest>()) else {
			return;
		};
		if let Err(e) = create_schema(&request) {
			web_sys::console::warn_1(&format!("IndexedDB upgrade failed: {e:?}").into());
			// Aborting fails the open request, so the store falls back to memory-only.
			if let Some(transaction) = request.transaction() {
				let _ = transaction.abort();
			}
		}
	});
	open_request.set_onupgradeneeded(Some(on_upgrade.as_ref().unchecked_ref()));
	on_upgrade.forget();

	await_request::<IdbDatabase>(&open_request).await
}

/// Replaces whatever object store an older version left with an empty current one.
fn create_schema(request: &IdbRequest) -> std::result::Result<(), JsValue> {
	let db: IdbDatabase = request.result()?.unchecked_into();
	if db.object_store_names().contains(STORE_NAME) {
		db.delete_object_store(STORE_NAME)?;
	}
	let params = web_sys::IdbObjectStoreParameters::new();
	params.set_key_path(&JsValue::from_str(KEY_PATH));
	db.create_object_store_with_optional_parameters(STORE_NAME, &params)?;
	Ok(())
}

/// Every record in the object store, in key order, as JSON.
///
/// A record that cannot be read back as JSON is skipped with a warning.
pub(super) async fn read_all(db: &IdbDatabase) -> Result<Vec<Value>> {
	let transaction = db
		.transaction_with_str_and_mode(STORE_NAME, IdbTransactionMode::Readonly)
		.map_err(|e| js_error("Transaction error", e))?;
	let store = transaction
		.object_store(STORE_NAME)
		.map_err(|e| js_error("Object store error", e))?;
	let request = store.get_all().map_err(|e| js_error("GetAll error", e))?;
	let array = await_request::<js_sys::Array>(&request).await?;

	let mut records = Vec::with_capacity(array.length() as usize);
	for record in array.iter() {
		let json = js_sys::JSON::stringify(&record)
			.ok()
			.and_then(|s| s.as_string())
			.and_then(|s| serde_json::from_str::<Value>(&s).ok());
		match json {
			Some(json) => records.push(json),
			None => web_sys::console::warn_1(&"Skipping an unreadable IndexedDB record".into()),
		}
	}
	Ok(records)
}

/// Starts a read/write transaction on the object store.
fn begin_write(db: &IdbDatabase) -> Result<(IdbTransaction, IdbObjectStore)> {
	let transaction = db
		.transaction_with_str_and_mode(STORE_NAME, IdbTransactionMode::Readwrite)
		.map_err(|e| js_error("Transaction error", e))?;
	let store = transaction
		.object_store(STORE_NAME)
		.map_err(|e| js_error("Object store error", e))?;
	Ok((transaction, store))
}

fn to_js(record: &Value) -> Result<JsValue> {
	let json =
		serde_json::to_string(record).map_err(|e| Error::other(format!("JSON error: {e:?}")))?;
	js_sys::JSON::parse(&json).map_err(|e| js_error("JS JSON parse error", e))
}

/// Issues the add of `value` under `key`, appended under `batch`.
pub(super) fn add(db: &IdbDatabase, key: u32, batch: u32, value: &Value) -> Result<IdbTransaction> {
	let (transaction, store) = begin_write(db)?;
	let js_value = to_js(&record(key, batch, value))?;
	store.add(&js_value).map_err(|e| js_error("Add error", e))?;
	Ok(transaction)
}

/// Issues the write of the boundary record, replacing any earlier one.
pub(super) fn put_boundary(db: &IdbDatabase, batch: u32) -> Result<IdbTransaction> {
	let (transaction, store) = begin_write(db)?;
	let js_value = to_js(&json!({ (KEY_PATH): BOUNDARY_KEY, (BATCH_FIELD): batch }))?;
	store.put(&js_value).map_err(|e| js_error("Put error", e))?;
	Ok(transaction)
}

/// Issues the deletes of `keys`, in one transaction.
pub(super) fn delete(db: &IdbDatabase, keys: &[u32]) -> Result<IdbTransaction> {
	let (transaction, store) = begin_write(db)?;
	for &key in keys {
		store
			.delete(&JsValue::from(key))
			.map_err(|e| js_error("Delete error", e))?;
	}
	Ok(transaction)
}

/// Issues the deletion of every record.
pub(super) fn clear(db: &IdbDatabase) -> Result<IdbTransaction> {
	let (transaction, store) = begin_write(db)?;
	store.clear().map_err(|e| js_error("Clear error", e))?;
	Ok(transaction)
}

/// Logs, in the background, if the issued write fails or its transaction aborts.
pub(super) fn fire_and_forget(what: &'static str, issued: Result<IdbTransaction>) {
	let done = match issued {
		Ok(transaction) => completion(&transaction),
		Err(e) => {
			web_sys::console::warn_1(&format!("IndexedDB {what} failed: {e}").into());
			return;
		}
	};
	spawn_local(async move {
		if let Err(e) = done.await {
			web_sys::console::warn_1(&format!("IndexedDB {what} failed: {e}").into());
		}
	});
}

/// Resolves when `transaction` commits, or fails when it aborts.
///
/// The handlers are attached before this returns, so no outcome is missed however late
/// the future is first polled. A failed request aborts its transaction (nothing here
/// handles its error event), so `complete` and `abort` cover every outcome and only one
/// of them ever fires; the handlers are dropped once it has.
pub(super) fn completion(transaction: &IdbTransaction) -> impl Future<Output = Result<()>> {
	let (sender, receiver) = futures_channel::oneshot::channel();
	let sender = Rc::new(RefCell::new(Some(sender)));

	let complete_sender = sender.clone();
	let on_complete = Closure::<dyn FnMut()>::new(move || {
		if let Some(sender) = complete_sender.borrow_mut().take() {
			let _ = sender.send(Ok(()));
		}
	});
	let on_abort = Closure::<dyn FnMut()>::new(move || {
		if let Some(sender) = sender.borrow_mut().take() {
			let _ = sender.send(Err(Error::other("transaction aborted")));
		}
	});
	transaction.set_oncomplete(Some(on_complete.as_ref().unchecked_ref()));
	transaction.set_onabort(Some(on_abort.as_ref().unchecked_ref()));

	async move {
		let outcome = receiver
			.await
			.unwrap_or_else(|_| Err(Error::other("Channel closed")));
		drop((on_complete, on_abort));
		outcome
	}
}

/// Awaits an IdbRequest and extracts its result.
pub(super) async fn await_request<T: JsCast>(request: &IdbRequest) -> Result<T> {
	let (sender, receiver) = futures_channel::oneshot::channel();
	let sender = Rc::new(RefCell::new(Some(sender)));

	let success_sender = sender.clone();
	let onsuccess = Closure::once(move |_event: web_sys::Event| {
		if let Some(sender) = success_sender.borrow_mut().take() {
			let _ = sender.send(Ok(()));
		}
	});

	let error_sender = sender.clone();
	let onerror = Closure::once(move |_event: web_sys::Event| {
		if let Some(sender) = error_sender.borrow_mut().take() {
			let _ = sender.send(Err(Error::other("IndexedDB request failed")));
		}
	});

	request.set_onsuccess(Some(onsuccess.as_ref().unchecked_ref()));
	request.set_onerror(Some(onerror.as_ref().unchecked_ref()));

	onsuccess.forget();
	onerror.forget();

	receiver
		.await
		.map_err(|_| Error::other("Channel closed"))??;

	request
		.result()
		.map_err(|e| js_error("Result error", e))?
		.dyn_into::<T>()
		.map_err(|_| Error::other("Type cast failed"))
}
