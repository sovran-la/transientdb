//! IndexedDB plumbing for `WebStore`: the database schema, its record format, and the
//! requests the store issues. Opening the database is in [`connect`](super::connect).
//!
//! Every record holds one stored item as `{ "tab": "<id>", "seq": <number>, "text":
//! "<item>" }` in the object store [`STORE_NAME`], whose key path is `["tab", "seq"]`
//! with no auto-increment, and which has the index [`TAB_INDEX`] on `tab`. `tab` is the
//! id of the store instance that holds the item (see [`tab`](super::tab)); `seq` is the
//! item's sequence number in that instance's queue, assigned by the store, so memory and
//! IndexedDB always agree on it, and the pair is unique across instances. `text` is the
//! item's compact `serde_json` text, kept as a string so that an integer above 2^53 or a
//! float such as `42.0` comes back exactly. Nothing else is stored: fetched items are
//! pending in memory only.
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
/// Version 5 stores `{seq, text}` and nothing about batches. Version 6 stores
/// `{tab, seq, text}` keyed by `[tab, seq]`, with an index on `tab`.
pub(super) const DB_VERSION: u32 = 6;
pub(super) const STORE_NAME: &str = "events";
pub(super) const TAB_FIELD: &str = "tab";
pub(super) const SEQ_FIELD: &str = "seq";
const TEXT_FIELD: &str = "text";
/// The index on [`TAB_FIELD`], for finding every tab with records, and each one's records.
pub(super) const TAB_INDEX: &str = "tab";
/// The last sequence number the store gives out: `Number.MAX_SAFE_INTEGER`, 2^53 - 1.
pub(super) const MAX_SEQ: u64 = (1 << 53) - 1;

pub(super) fn js_error(what: &str, e: JsValue) -> Error {
	Error::other(format!("{what}: {e:?}"))
}

/// The key path of the object store: `["tab", "seq"]`.
pub(super) fn key_path() -> JsValue {
	js_sys::Array::of2(&TAB_FIELD.into(), &SEQ_FIELD.into()).into()
}

/// The record stored for the item `text` under `seq` in tab `tab`.
pub(super) fn record(tab: &str, seq: u64, text: &str) -> Value {
	json!({ (TAB_FIELD): tab, (SEQ_FIELD): seq, (TEXT_FIELD): text })
}

/// The key of the record under `seq` in tab `tab`.
pub(super) fn key(tab: &str, seq: u64) -> JsValue {
	js_sys::Array::of2(&tab.into(), &JsValue::from_f64(seq as f64)).into()
}

/// The sequence number of a record, if its key is one the store could have given out.
pub(super) fn record_seq(record: &Value) -> Option<u64> {
	let seq = record.get(SEQ_FIELD)?.as_u64()?;
	(1..=MAX_SEQ).contains(&seq).then_some(seq)
}

/// The sequence number and text of a record, if it has the record shape: a sequence
/// number and a non-empty string. No `append` stores empty text, so a record with it
/// is not an item. Whether the text is a fit item otherwise is the store's to check.
pub(super) fn parse_record(record: &Value) -> Option<(u64, String)> {
	let seq = record_seq(record)?;
	let text = record.get(TEXT_FIELD)?.as_str()?;
	(!text.is_empty()).then(|| (seq, text.to_owned()))
}

/// Every record in the object store, in key order, as JSON.
///
/// A record that `JSON.stringify` or `serde_json` rejects (a BigInt or a cycle in it, say)
/// comes back as `{}`: not a stored item. Only for tests and diagnostics; adopting reads
/// a tab's records together with their keys instead.
#[cfg(test)]
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
		records.push(as_json(&record).unwrap_or_else(|| json!({})));
	}
	Ok(records)
}

/// `value` as JSON, if `JSON.stringify` and `serde_json` both accept it.
pub(super) fn as_json(value: &JsValue) -> Option<Value> {
	let text = js_sys::JSON::stringify(value).ok()?.as_string()?;
	serde_json::from_str(&text).ok()
}

/// Starts a read/write transaction on the object store.
pub(super) fn begin_write(db: &IdbDatabase) -> Result<(IdbTransaction, IdbObjectStore)> {
	let transaction = db
		.transaction_with_str_and_mode(STORE_NAME, IdbTransactionMode::Readwrite)
		.map_err(|e| js_error("Transaction error", e))?;
	let store = transaction
		.object_store(STORE_NAME)
		.map_err(|e| js_error("Object store error", e))?;
	Ok((transaction, store))
}

pub(super) fn to_js(record: &Value) -> Result<JsValue> {
	js_sys::JSON::parse(&record.to_string()).map_err(|e| js_error("JS JSON parse error", e))
}

/// Issues the add of the item `text` under `seq` in tab `tab`.
pub(super) fn add(db: &IdbDatabase, tab: &str, seq: u64, text: &str) -> Result<IdbTransaction> {
	let (transaction, store) = begin_write(db)?;
	store
		.add(&to_js(&record(tab, seq, text))?)
		.map_err(|e| js_error("Add error", e))?;
	Ok(transaction)
}

/// Issues the deletes of the records under `seqs` in tab `tab`, in one transaction.
pub(super) fn delete(db: &IdbDatabase, tab: &str, seqs: &[u64]) -> Result<IdbTransaction> {
	let (transaction, store) = begin_write(db)?;
	for &seq in seqs {
		store
			.delete(&key(tab, seq))
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
			super::warn(&format!("IndexedDB {what} failed: {e}"));
			return;
		}
	};
	spawn_local(async move {
		if let Err(e) = done.await {
			super::warn(&format!("IndexedDB {what} failed: {e}"));
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
#[cfg(test)]
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
	result_of(request)
}

/// The result of a request that has succeeded.
pub(super) fn result_of<T: JsCast>(request: &IdbRequest) -> Result<T> {
	request
		.result()
		.map_err(|e| js_error("Result error", e))?
		.dyn_into::<T>()
		.map_err(|_| Error::other("Type cast failed"))
}
