//! IndexedDB plumbing for `WebStore`: the database schema, its record format, and the
//! requests the store issues.
//!
//! Every record holds one stored item as `{ "seq": <number>, "text": "<item>" }` in the
//! object store [`STORE_NAME`], whose key path is `seq` with no auto-increment. `seq` is
//! the item's sequence number, assigned by the store, so memory and IndexedDB always
//! agree on it; `text` is the item's compact `serde_json` text, kept as a string so that
//! an integer above 2^53 or a float such as `42.0` comes back exactly. Nothing else is
//! stored: fetched items are pending in memory only.
//!
//! An IndexedDB number key is a JavaScript number, an `f64`. Sequence numbers are `u64`,
//! so the store gives out only `1..=`[`MAX_SEQ`], every one of which an `f64` holds
//! exactly; a key outside that range, or not a whole number, is not a sequence number.
//! At one append per microsecond, the range lasts more than 285 years.
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
/// Version 5 stores `{seq, text}` and nothing about batches.
pub(super) const DB_VERSION: u32 = 5;
pub(super) const STORE_NAME: &str = "events";
const KEY_PATH: &str = "seq";
const TEXT_FIELD: &str = "text";
/// The last sequence number the store gives out: `Number.MAX_SAFE_INTEGER`, 2^53 - 1.
pub(super) const MAX_SEQ: u64 = (1 << 53) - 1;

fn js_error(what: &str, e: JsValue) -> Error {
	Error::other(format!("{what}: {e:?}"))
}

/// The record stored for the item `text` under `seq`.
pub(super) fn record(seq: u64, text: &str) -> Value {
	json!({ (KEY_PATH): seq, (TEXT_FIELD): text })
}

/// The sequence number of a record, if its key is one the store could have given out.
pub(super) fn record_seq(record: &Value) -> Option<u64> {
	let seq = record.get(KEY_PATH)?.as_u64()?;
	(1..=MAX_SEQ).contains(&seq).then_some(seq)
}

/// The sequence number and text of a record, if it has the record shape: a sequence
/// number and a string. Whether the text is a fit item is the store's to check.
pub(super) fn parse_record(record: &Value) -> Option<(u64, String)> {
	let seq = record_seq(record)?;
	let text = record.get(TEXT_FIELD)?.as_str()?;
	Some((seq, text.to_owned()))
}

/// The key a record is stored under, whatever it is, for deleting the record.
pub(super) fn record_key(record: &Value) -> Option<JsValue> {
	js_sys::JSON::parse(&record.get(KEY_PATH)?.to_string()).ok()
}

/// Opens (or creates, or upgrades) the database `name` at [`DB_VERSION`].
///
/// Upgrading from an older version deletes every object store it has and creates the
/// current one empty: old records are discarded, not migrated.
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
	js_sys::JSON::parse(&record.to_string()).map_err(|e| js_error("JS JSON parse error", e))
}

/// Issues the add of the item `text` under `seq`.
pub(super) fn add(db: &IdbDatabase, seq: u64, text: &str) -> Result<IdbTransaction> {
	let (transaction, store) = begin_write(db)?;
	store
		.add(&to_js(&record(seq, text))?)
		.map_err(|e| js_error("Add error", e))?;
	Ok(transaction)
}

/// Issues the deletes of the records under `seqs`, in one transaction.
pub(super) fn delete(db: &IdbDatabase, seqs: &[u64]) -> Result<IdbTransaction> {
	let keys: Vec<JsValue> = seqs
		.iter()
		.map(|&seq| JsValue::from_f64(seq as f64))
		.collect();
	delete_keys(db, &keys)
}

/// Issues the deletes of the records under `keys`, whatever they are, in one transaction.
pub(super) fn delete_keys(db: &IdbDatabase, keys: &[JsValue]) -> Result<IdbTransaction> {
	let (transaction, store) = begin_write(db)?;
	for key in keys {
		store.delete(key).map_err(|e| js_error("Delete error", e))?;
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
