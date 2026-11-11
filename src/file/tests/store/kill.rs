//! A kill at every byte boundary of an append through the store, into an existing slot and
//! into a new one at the end of the file, and a kill while removing; the next run opens
//! the file with `FileStore::new` and carries on.

use super::*;
use crate::contract::{append_all, available, item_of_len, tagged};
use crate::file::entry::LEN as ENTRY_LEN;
use crate::DataStore;
use serde_json::json;

/// The bytes of an entry that must reach the file before the item validates: everything
/// up to and including its CRC.
const ENTRY_COMMITTED: usize = 20;

/// Appends `item` to a copy of the store file `template` with `budget` bytes allowed to
/// reach it, and returns whether the append reported success.
fn killed_append(template: &[u8], path: &Path, item: &Value, budget: usize) -> bool {
	std::fs::write(path, template).unwrap();
	let (mut store, hook) = store_with(path, 10);
	hook.lock().unwrap().write_budget = Some(budget);
	store.append(item.clone()).is_ok()
}

#[test]
fn test_a_kill_at_every_byte_of_an_append_into_an_existing_slot() {
	let (dir, path) = temp_path();
	let mut store = store(&path, 10);
	append_all(&mut store, &[item_of_len(10, 1), item_of_len(10, 2)]);
	let first = store.fetch(1, 100).unwrap().unwrap();
	store.remove(first).unwrap();
	drop(store);
	let template = bytes_of(&path);
	let item = item_of_len(10, 3);
	let total = 10 + ENTRY_LEN;
	for budget in 0..=total {
		let run = dir.path().join(format!("existing-{budget}.tdb"));
		let acknowledged = killed_append(&template, &run, &item, budget);
		assert_eq!(acknowledged, budget >= total, "budget {budget}");
		let mut next = super::store(&run, 10);
		let mut expected = vec![item_of_len(10, 2)];
		if budget >= 10 + ENTRY_COMMITTED {
			expected.push(item.clone());
		}
		assert_eq!(available(&mut next), expected, "budget {budget}");
		assert_eq!(file_len(&run), len_of(2), "budget {budget}");
		next.append(json!(4)).unwrap();
		expected.push(json!(4));
		drop(next);
		assert_eq!(reopened(&run), expected, "budget {budget}");
	}
}

#[test]
fn test_a_kill_at_every_byte_of_an_append_into_a_new_slot() {
	let (dir, path) = temp_path();
	let mut store = store(&path, 10);
	store.append(item_of_len(10, 1)).unwrap();
	drop(store);
	let template = bytes_of(&path);
	let item = item_of_len(10, 2);
	// A new slot's payload is written whole.
	let total = SLOT + ENTRY_LEN;
	for budget in 0..=total {
		let run = dir.path().join(format!("new-{budget}.tdb"));
		let acknowledged = killed_append(&template, &run, &item, budget);
		assert_eq!(acknowledged, budget == total, "budget {budget}");
		let mut next = super::store(&run, 10);
		let mut expected = vec![item_of_len(10, 1)];
		if budget >= SLOT + ENTRY_COMMITTED {
			expected.push(item.clone());
		}
		assert_eq!(available(&mut next), expected, "budget {budget}");
		// The next append overwrites any partial slot, and the file ends on a slot.
		next.append(json!(3)).unwrap();
		expected.push(json!(3));
		assert_eq!(
			file_len(&run),
			len_of(expected.len() as u64),
			"budget {budget}"
		);
		drop(next);
		assert_eq!(reopened(&run), expected, "budget {budget}");
	}
}

#[test]
fn test_a_kill_while_removing_leaves_the_item_or_removes_it() {
	for budget in [0, 1] {
		let (_dir, path) = temp_path();
		let (mut store, hook) = store_with(&path, 10);
		append_all(&mut store, &tagged(0..2));
		let first = store.fetch(1, 100).unwrap().unwrap();
		hook.lock().unwrap().write_budget = Some(budget);
		assert_eq!(store.remove(first).is_ok(), budget == 1, "budget {budget}");
		drop(store);
		// At worst the item is sent again.
		let expected = if budget == 0 {
			tagged(0..2)
		} else {
			tagged(1..2)
		};
		assert_eq!(reopened(&path), expected, "budget {budget}");
	}
}
