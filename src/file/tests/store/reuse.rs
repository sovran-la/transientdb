//! Slot reuse through the store: each item fetched on its own, removed in every order,
//! then appends into the lowest free slot, with the file not growing while a slot is
//! free and fetch order by sequence throughout.

use super::*;
use crate::contract::{append_all, available, tagged};
use crate::{DataStore, Items};
use serde_json::json;

const ITEMS: usize = 4;

#[test]
fn test_removal_in_every_order_then_appends_reuse_the_lowest_free_slot() {
	for order in permutations(ITEMS) {
		// Remove some, never all: removing the last item cuts the file (see shrink).
		for removed in 1..ITEMS {
			let (_dir, path) = temp_path();
			let mut store = store(&path, 100);
			append_all(&mut store, &tagged(0..ITEMS));
			let mut fetches: Vec<Option<Items>> =
				(0..ITEMS).map(|_| store.fetch(1, 100).unwrap()).collect();
			let gone = &order[..removed];
			for &i in gone {
				store.remove(fetches[i].take().unwrap()).unwrap();
				assert_eq!(file_len(&path), len_of(ITEMS as u64), "{order:?}");
			}
			drop(fetches);
			let mut kept: Vec<Value> = (0..ITEMS)
				.filter(|i| !gone.contains(i))
				.map(|i| json!({ "i": i }))
				.collect();
			assert_eq!(available(&mut store), kept, "{order:?}");

			// Each append takes the lowest free slot, whatever order they were freed in.
			let mut free: Vec<u64> = gone.iter().map(|&i| i as u64).collect();
			free.sort_unstable();
			for (n, slot) in free.into_iter().enumerate() {
				store.append(json!({ "n": n })).unwrap();
				let slots = slots_of(store.core().unwrap());
				assert_eq!(slots.last(), Some(&slot), "{order:?}");
				assert_eq!(file_len(&path), len_of(ITEMS as u64), "{order:?}");
				kept.push(json!({ "n": n }));
			}
			// Fetch order is by sequence, not slot.
			assert_eq!(available(&mut store), kept, "{order:?}");
			// No slot is free: the next append grows the file by one slot.
			store.append(json!("last")).unwrap();
			assert_eq!(file_len(&path), len_of(ITEMS as u64 + 1), "{order:?}");
			kept.push(json!("last"));
			drop(store);
			assert_eq!(reopened(&path), kept, "{order:?}");
		}
	}
}
