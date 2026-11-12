//! A damaged item is caught at fetch: one that fails its CRC or is not UTF-8 is freed,
//! logged and skipped, without counting toward the fetch's limits, so it cannot be fetched
//! forever.

use super::*;
use crate::contract::{append_all, available, tagged};
use crate::DataStore;
use serde_json::json;

/// Replaces item 2 (in slot 1) of the file at `path`, whose slots are `slot_size` bytes,
/// with `payload` under a valid entry, or damages its payload in place for `None`.
fn damage(path: &Path, slot_size: usize, payload: Option<&[u8]>) {
	match payload {
		None => {
			let layout = crate::file::layout::Layout {
				slot_size: slot_size as u32,
			};
			poke(path, layout.payload_offset(1) + 1, b"Z");
		}
		Some(bytes) => write_item(path, slot_size, 1, 2, bytes),
	}
}

#[test]
fn test_a_damaged_item_is_freed_and_skipped_and_does_not_count() {
	let size = 512;
	let cases: [(&str, Option<&[u8]>); 2] =
		[("crc", None), ("utf8", Some(&[b'"', 0xFF, 0xFE, b'"']))];
	for (how, payload) in cases {
		let (_dir, path) = temp_path();
		let config = config(&path, size, 100);
		let mut first = FileStore::new(config.clone());
		append_all(&mut first, &tagged(0..4));
		drop(first);
		// Damaged between runs, so the next store's index has the damaged entry.
		damage(&path, size, payload);
		let mut store = FileStore::new(config.clone());
		// The damaged item does not use up the count or the bytes. It is read, so it must
		// fit what is left of `max_bytes`; once found damaged, its bytes are not counted.
		let damaged_len = payload.map_or(7, <[u8]>::len);
		let items = store.fetch(2, 7 + damaged_len.max(7)).unwrap().unwrap();
		assert_eq!(
			items.values(),
			vec![json!({"i": 0}), json!({"i": 2})],
			"{how}"
		);
		// Freed in the file, not only skipped.
		let entry = crate::file::layout::Layout {
			slot_size: size as u32,
		}
		.entry_offset(1);
		assert_eq!(bytes_of(&path)[entry as usize], 0, "{how}");
		drop(items);
		assert_eq!(
			available(&mut store),
			vec![json!({"i": 0}), json!({"i": 2}), json!({"i": 3})]
		);
		drop(store);
		assert_eq!(stored(&mut FileStore::new(config)).len(), 3, "{how}");
	}
}

#[test]
fn test_a_store_whose_only_items_are_damaged_hands_out_nothing() {
	// `has_data` is true until a fetch finds the damage: no payload is read before a
	// fetch, in the store that wrote them or in the next one.
	for reopen in [false, true] {
		let (_dir, path) = temp_path();
		let mut store = store(&path, 100);
		append_all(&mut store, &tagged(0..2));
		for slot in 0..2 {
			poke(&path, layout().payload_offset(slot) + 1, b"Z");
		}
		if reopen {
			drop(store);
			store = super::store(&path, 100);
		}
		assert!(store.has_data(), "nothing is read until a fetch ({reopen})");
		assert!(store.fetch(10, 100).unwrap().is_none(), "{reopen}");
		assert!(!store.has_data(), "{reopen}");
		assert_eq!(file_len(&path), crate::file::layout::HEADER_LEN, "{reopen}");
	}
}
