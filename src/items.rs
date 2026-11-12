//! [`Items`]: what a fetch hands out, and the store's claim on it.

use crate::claims::Ledger;
use serde_json::Value;
use std::fmt;

/// Items handed out by one `fetch`, oldest first, and the store's claim on them.
///
/// An `Items` is never empty. Its items are not returned by any other fetch until it is
/// given back, which happens in one of two ways:
///
/// - Pass it to the store's `remove` once its items are delivered: they are deleted.
/// - Release it, with [`release`](Self::release) or by dropping it: its items will be
///   returned by a later fetch, in their original order. An early return or a panic
///   while holding an `Items` therefore cannot leave items held back forever.
///
/// Releasing needs no reference to the store and never waits for the store's lock, so
/// an `Items` can be dropped anywhere: on another thread, while the store is busy, or
/// after the store itself is gone (then it does nothing). An `Items` is `Send` on native
/// targets, so a caller can fetch on one thread and remove on another.
///
/// The items are kept as the text that was stored. An item is checked when it is
/// appended, and what is fetched is what was stored, so each item's text is the compact
/// JSON its `append` serialized. [`json`](Self::json), [`values`](Self::values) and
/// [`iter`](Self::iter) are three views of that text; none of them can fail or panic,
/// whatever the text is, and none wraps the items in anything.
///
/// The one way stored text can be other than what `append` wrote is for something else to
/// change the storage underneath the store: another script on the page writing to a
/// `WebStore`'s database, say. Such text is handed out as it is: `json` and `iter` pass it
/// through, and `values` gives it as a JSON string if it does not parse.
///
/// ```
/// use serde_json::json;
/// use transientdb::{DataStore, MemoryConfig, MemoryStore};
///
/// let mut store = MemoryStore::new(MemoryConfig { max_item_size: 1024, max_items: 100 });
/// store.append(json!({"event": "open"})).unwrap();
/// store.append(json!({"event": "close"})).unwrap();
///
/// let items = store.fetch(10, 4096).unwrap().unwrap();
/// assert_eq!(items.len(), 2);
/// let body = format!("{{\"events\":{}}}", items.json());
/// assert_eq!(body, r#"{"events":[{"event":"open"},{"event":"close"}]}"#);
///
/// // Delivered: remove them. Had the send failed, dropping `items` would have
/// // released them for a later fetch.
/// store.remove(items).unwrap();
/// assert!(!store.has_data());
/// ```
pub struct Items {
	ledger: Ledger,
	/// The ledger's epoch when this was issued; a reset since then makes it match nothing.
	epoch: u64,
	/// Each item's sequence number and text, in sequence order. Empty only once disarmed
	/// by `surrender`, after which dropping it does nothing.
	entries: Vec<(u64, String)>,
	byte_len: usize,
}

impl Items {
	/// Items issued under `ledger` in `epoch`. `entries` is never empty.
	pub(crate) fn new(ledger: Ledger, epoch: u64, entries: Vec<(u64, String)>) -> Self {
		assert!(!entries.is_empty(), "an Items is never empty");
		let byte_len = entries.iter().map(|(_, text)| text.len()).sum();
		Self {
			ledger,
			epoch,
			entries,
			byte_len,
		}
	}

	/// How many items. Never 0.
	#[allow(clippy::len_without_is_empty)] // never empty, so `is_empty` would always be false
	pub fn len(&self) -> usize {
		self.entries.len()
	}

	/// Total bytes of the items' text, as counted against a fetch's `max_bytes`.
	pub fn byte_len(&self) -> usize {
		self.byte_len
	}

	/// One JSON array holding every item, oldest first: `[{...},{...}]`.
	///
	/// Built by joining each item's stored text as it is, with commas, inside brackets;
	/// nothing is parsed or checked. Its length is [`byte_len`](Self::byte_len), plus one
	/// comma between each pair of items, plus 2. It is a well-formed JSON array if every
	/// item's text is well-formed JSON, as everything `append` stored is; text written
	/// to the storage by something else is joined as it is.
	pub fn json(&self) -> String {
		let mut json = String::with_capacity(self.byte_len + self.entries.len() + 1);
		json.push('[');
		for (i, (_, text)) in self.entries.iter().enumerate() {
			if i > 0 {
				json.push(',');
			}
			json.push_str(text);
		}
		json.push(']');
		json
	}

	/// Every item parsed, oldest first. Each equals the value that was appended.
	///
	/// An item whose stored text does not parse (text written to the storage by
	/// something else) is given as a [`Value::String`] holding that text, and the others
	/// are parsed as usual. `serde_json` refuses text nested past its recursion limit
	/// rather than overflow the stack, so that too is given as a string.
	pub fn values(&self) -> Vec<Value> {
		self.iter()
			.map(|text| {
				serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
			})
			.collect()
	}

	/// Each item's stored text, oldest first, as it is: the JSON `append` serialized for
	/// it. Nothing is parsed or checked.
	pub fn iter(&self) -> impl Iterator<Item = &str> + '_ {
		self.entries.iter().map(|(_, text)| text.as_str())
	}

	/// Gives the items back to the store: a later fetch returns them again. The same as
	/// dropping this `Items`; this is the explicit spelling.
	pub fn release(self) {
		drop(self);
	}

	/// For `remove`: if this was issued under `ledger`, disarms it and returns its epoch
	/// and sequence numbers. Otherwise drops it, releasing it to the store that issued
	/// it, and returns `None`.
	pub(crate) fn surrender(mut self, ledger: &Ledger) -> Option<(u64, Vec<u64>)> {
		if !self.ledger.same(ledger) {
			return None;
		}
		let entries = std::mem::take(&mut self.entries);
		Some((
			self.epoch,
			entries.into_iter().map(|(seq, _)| seq).collect(),
		))
	}

	#[cfg(test)]
	pub(crate) fn seqs(&self) -> Vec<u64> {
		self.entries.iter().map(|(seq, _)| *seq).collect()
	}
}

impl Drop for Items {
	fn drop(&mut self) {
		if self.entries.is_empty() {
			return;
		}
		let seqs = self.entries.iter().map(|(seq, _)| *seq);
		self.ledger.lock().release(self.epoch, seqs);
	}
}

impl fmt::Debug for Items {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("Items")
			.field("len", &self.len())
			.field("byte_len", &self.byte_len)
			.field("items", &self.iter().collect::<Vec<_>>())
			.finish()
	}
}

// Fetch on one thread, remove or release on another.
#[cfg(not(target_arch = "wasm32"))]
const _: () = {
	const fn assert_send<T: Send>() {}
	assert_send::<Items>();
};

#[cfg(test)]
mod tests;
