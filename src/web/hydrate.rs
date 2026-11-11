//! Loading a `WebStore`'s queue from IndexedDB when it opens.

use super::{idb, WebStore};
use crate::serialize;
use std::io::Result;
use wasm_bindgen::JsValue;
use web_sys::IdbDatabase;

fn warn(message: &str) {
	web_sys::console::warn_1(&message.into());
}

impl WebStore {
	/// Loads every stored item from IndexedDB into memory, in sequence order, and sets the
	/// next sequence number above every one seen. Every item is available: nothing that
	/// was pending before a reload is pending after it.
	///
	/// What this store would not have accepted is discarded, deleted from IndexedDB and
	/// logged, the way FileStore discards what does not match at open:
	///
	/// - a record without the record shape (a sequence number in `1..=MAX_SEQ` and a
	///   string), or one that cannot be read as JSON at all, whose sequence number, if it
	///   has one, still counts, so no new item is given it;
	/// - an item that is not well-formed JSON, is larger than `max_item_size` (stored under
	///   a larger limit), or is nested too deeply;
	/// - the oldest items beyond `max_items`, if more are stored (under a larger limit).
	///
	/// Only reading can fail; the deletes are fire-and-forget, like every write.
	pub(super) async fn hydrate(&mut self, db: &IdbDatabase) -> Result<()> {
		let mut discarded: Vec<JsValue> = Vec::new();
		for record in idb::read_all(db).await? {
			if let Some(seq) = idb::record_seq(&record) {
				self.queue.skip_past(seq);
			}
			let Some((seq, text)) = idb::parse_record(&record) else {
				warn("Discarding an IndexedDB record that is not a stored item");
				discarded.extend(idb::record_key(&record));
				continue;
			};
			match serialize::check_text(&text, self.config.max_item_size) {
				Ok(()) => self.queue.restore(seq, text),
				Err(unfit) => {
					warn(&format!("Discarding stored item {seq}: {unfit}"));
					discarded.push(JsValue::from_f64(seq as f64));
				}
			}
		}

		let dropped = self.queue.drop_oldest_beyond(self.config.max_items);
		if !dropped.is_empty() {
			warn(&format!(
				"Dropping the {} oldest stored items: more are stored than max_items = {}",
				dropped.len(),
				self.config.max_items
			));
			discarded.extend(dropped.iter().map(|&seq| JsValue::from_f64(seq as f64)));
		}
		if !discarded.is_empty() {
			idb::fire_and_forget("discard", idb::delete_keys(db, &discarded));
		}
		Ok(())
	}
}
