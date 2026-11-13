//! The IndexedDB side of adopting: finding which tabs have records, and moving one tab's
//! records under this store's tab in a single transaction.

use super::idb::{self, js_error, result_of};
use super::{warn, Shared};
use js_sys::Array;
use std::cell::RefCell;
use std::io::{Error, Result};
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use web_sys::{IdbCursor, IdbCursorDirection, IdbDatabase, IdbKeyRange, IdbObjectStore};

/// The distinct tab keys that have records, in key order: a key cursor on the tab index
/// that visits each key once.
pub(super) async fn distinct_tabs(db: &IdbDatabase) -> Result<Vec<JsValue>> {
	let transaction = db
		.transaction_with_str(idb::STORE_NAME)
		.map_err(|e| js_error("Transaction error", e))?;
	let index = transaction
		.object_store(idb::STORE_NAME)
		.and_then(|store| store.index(idb::TAB_INDEX))
		.map_err(|e| js_error("Index error", e))?;
	let request = index
		.open_key_cursor_with_range_and_direction(&JsValue::NULL, IdbCursorDirection::Nextunique)
		.map_err(|e| js_error("Cursor error", e))?;

	let tabs = Rc::new(RefCell::new(Vec::new()));
	// Advanced from its own success handler, so the transaction stays active throughout;
	// it completes once the cursor has run off the end.
	let on_success = Closure::<dyn FnMut(web_sys::Event)>::new({
		let (request, tabs) = (request.clone(), tabs.clone());
		move |_event: web_sys::Event| {
			let Ok(cursor) = request.result() else { return };
			let Ok(cursor) = cursor.dyn_into::<IdbCursor>() else {
				return; // null: the end
			};
			if let Ok(key) = cursor.key() {
				tabs.borrow_mut().push(key);
			}
			let _ = cursor.continue_();
		}
	});
	request.set_onsuccess(Some(on_success.as_ref().unchecked_ref()));
	let outcome = idb::completion(&transaction).await;
	drop(on_success);
	outcome?;
	Ok(tabs.take())
}

/// What happened to one tab's records.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Taken {
	/// The sequence numbers the adopted items got in this store's queue, in order.
	pub(super) adopted: Vec<u64>,
	/// Records discarded: not items, or items over `max_item_size`.
	pub(super) discarded: usize,
}

/// In one read/write transaction, reads every record under the tab key `tab` in `seq`
/// order and deletes it. With `adopt`, each valid item is added back under this store's
/// tab with a new sequence number and queued in memory, in that order; without it, or for
/// a record that is not a valid item, the record is discarded, with a warning.
///
/// If the store is reset before the records are read, nothing is adopted: the reset's
/// clear, ordered after this transaction, deletes them. If the store starts over as a new
/// tab before then, nothing is adopted either, and the records are left as they are, for
/// the new tab's pass to adopt.
///
/// The caller must hold `tab`'s liveness lock to adopt. If the transaction aborts,
/// nothing changed in IndexedDB, and the items it had queued are taken out of memory
/// again (an `Items` already holding one matches nothing for it).
///
/// If this store runs out of sequence numbers, the records not yet adopted are left as
/// they are, for another tab.
pub(super) async fn take_over(
	shared: &Rc<Shared>,
	db: &IdbDatabase,
	tab: &JsValue,
	adopt: bool,
) -> Result<Taken> {
	let (transaction, store) = idb::begin_write(db)?;
	let range = IdbKeyRange::only(tab).map_err(|e| js_error("Key range error", e))?;
	let index = store
		.index(idb::TAB_INDEX)
		.map_err(|e| js_error("Index error", e))?;
	let values = index
		.get_all_with_key(&range)
		.map_err(|e| js_error("GetAll error", e))?;
	let keys = index
		.get_all_keys_with_key(&range)
		.map_err(|e| js_error("GetAllKeys error", e))?;

	let taken = Rc::new(RefCell::new(Taken::default()));
	let generation = shared.generation.get();
	// The keys' request succeeds after the values' one, issued before it. Everything is
	// done in its handler, while the transaction is active.
	let on_keys = Closure::<dyn FnMut(web_sys::Event)>::new({
		let (shared, taken, transaction) = (shared.clone(), taken.clone(), transaction.clone());
		let keys_request = keys.clone();
		move |_event: web_sys::Event| {
			if shared.generation.get() != generation {
				return;
			}
			let read = result_of::<Array>(&values).and_then(|values| {
				let keys = result_of::<Array>(&keys_request)?;
				Ok((values, keys))
			});
			let moved = read.and_then(|(values, keys)| {
				let mut taken = taken.borrow_mut();
				rewrite(&shared, &store, &values, &keys, adopt, &mut taken)
			});
			if let Err(e) = moved {
				warn(&format!("Adopting orphaned items failed: {e}"));
				let _ = transaction.abort();
			}
		}
	});
	keys.set_onsuccess(Some(on_keys.as_ref().unchecked_ref()));
	#[cfg(test)]
	if let Some(hook) = BETWEEN_READS.take() {
		hook();
	}
	let outcome = idb::completion(&transaction).await;
	drop(on_keys);
	let taken = taken.take();
	if let Err(e) = outcome {
		shared.queue.borrow_mut().discard(&taken.adopted);
		return Err(e);
	}
	Ok(taken)
}

/// Deletes each of a tab's records, read as `values` under `keys`, and adds each valid
/// item back under this store's tab with the next sequence number of its queue.
fn rewrite(
	shared: &Shared,
	store: &IdbObjectStore,
	values: &Array,
	keys: &Array,
	adopt: bool,
	taken: &mut Taken,
) -> Result<()> {
	if values.length() != keys.length() {
		return Err(Error::other("records and keys do not match"));
	}
	let max = shared.config.max_item_size;
	for (value, key) in values.iter().zip(keys.iter()) {
		let record = idb::as_json(&value);
		let item = record
			.as_ref()
			.and_then(idb::parse_record)
			.filter(|_| adopt);
		let Some((old_seq, text)) = item else {
			warn("Discarding an IndexedDB record that is not a stored item");
			store
				.delete(&key)
				.map_err(|e| js_error("Delete error", e))?;
			taken.discarded += 1;
			continue;
		};
		if text.len() > max {
			warn(&format!(
				"Discarding stored item {old_seq}: it is {} bytes, over max_item_size = {max} bytes",
				text.len()
			));
			store
				.delete(&key)
				.map_err(|e| js_error("Delete error", e))?;
			taken.discarded += 1;
			continue;
		}
		let seq = match shared.queue.borrow_mut().adopt(text.clone()) {
			Ok(seq) => seq,
			Err(_) => {
				warn("Sequence numbers exhausted: leaving the remaining orphaned items for another tab");
				return Ok(());
			}
		};
		taken.adopted.push(seq);
		store
			.delete(&key)
			.map_err(|e| js_error("Delete error", e))?;
		store
			.add(&idb::to_js(&idb::record(&shared.tab.borrow(), seq, &text))?)
			.map_err(|e| js_error("Add error", e))?;
	}
	Ok(())
}

#[cfg(test)]
thread_local! {
	/// Called once, if set, by the next move, after it has issued its reads and before
	/// their results come back, for tests.
	pub(super) static BETWEEN_READS: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}
