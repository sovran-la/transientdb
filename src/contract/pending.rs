//! What a fetch holds back until it is removed or released, and `reset`.

use super::*;
use serde_json::json;

pub(crate) fn consecutive_fetches_are_disjoint(store: &mut dyn DataStore) {
	append_all(store, &tagged(0..7));
	let fetches: Vec<Items> = (0..3)
		.map(|_| store.fetch(2, 1000).unwrap().unwrap())
		.collect();
	let values: Vec<Vec<Value>> = fetches.iter().map(Items::values).collect();
	assert_eq!(values, vec![tagged(0..2), tagged(2..4), tagged(4..6)]);
	let last = store.fetch(2, 1000).unwrap().unwrap();
	assert_eq!(last.values(), tagged(6..7));
	assert!(store.fetch(2, 1000).unwrap().is_none());
}

/// Two fetches, then `give_back` on the first: its items come back ahead of newer ones,
/// in sequence order, while the second's stay held back.
fn given_back_items_return_in_order(store: &mut dyn DataStore, give_back: fn(Items)) {
	append_all(store, &tagged(0..6));
	let first = store.fetch(2, 1000).unwrap().unwrap();
	let second = store.fetch(2, 1000).unwrap().unwrap();
	give_back(first);
	store.append(json!({"i": 6})).unwrap();
	let mut expected = tagged(0..2);
	expected.extend(tagged(4..7));
	let third = store.fetch(10, 1000).unwrap().unwrap();
	assert_eq!(third.values(), expected);
	give_back(third);
	give_back(second);
	assert_eq!(available(store), tagged(0..7));
	assert_eq!(drain(store, 3, 1000).concat(), tagged(0..7));
}

pub(crate) fn release_makes_items_available_in_order(store: &mut dyn DataStore) {
	given_back_items_return_in_order(store, Items::release);
}

pub(crate) fn drop_makes_items_available_in_order(store: &mut dyn DataStore) {
	given_back_items_return_in_order(store, drop);
}

pub(crate) fn has_data_is_false_with_everything_pending(store: &mut dyn DataStore) {
	append_all(store, &tagged(0..4));
	let first = store.fetch(3, 1000).unwrap().unwrap();
	assert!(store.has_data());
	let second = store.fetch(3, 1000).unwrap().unwrap();
	assert!(!store.has_data());
	assert!(store.fetch(10, 1000).unwrap().is_none());
	drop(first);
	assert!(store.has_data());
	store.remove(second).unwrap();
	assert!(store.has_data());
	let all = store.fetch(10, 1000).unwrap().unwrap();
	assert!(!store.has_data());
	store.remove(all).unwrap();
	assert!(!store.has_data());
}

pub(crate) fn remove_deletes_exactly_what_was_fetched(store: &mut dyn DataStore) {
	append_all(store, &tagged(0..10));
	let first = store.fetch(3, 1000).unwrap().unwrap();
	let second = store.fetch(2, 1000).unwrap().unwrap();
	store.remove(first).unwrap();
	// The second fetch's items are still held back, not deleted.
	assert_eq!(available(store), tagged(5..10));
	second.release();
	assert_eq!(available(store), tagged(3..10));
}

/// Every permutation of `0..n`.
fn permutations(n: usize) -> Vec<Vec<usize>> {
	if n == 0 {
		return vec![Vec::new()];
	}
	let mut all = Vec::new();
	for rest in permutations(n - 1) {
		for at in 0..=rest.len() {
			let mut order = rest.clone();
			order.insert(at, n - 1);
			all.push(order);
		}
	}
	all
}

pub(crate) fn remove_in_every_order(store: &mut dyn DataStore) {
	let orders = permutations(3);
	assert_eq!(orders.len(), 6);
	for order in orders {
		store.reset();
		append_all(store, &tagged(0..7));
		let mut fetches: Vec<Option<Items>> =
			(0..3).map(|_| store.fetch(2, 1000).unwrap()).collect();
		let mut removed = [false; 3];
		for &which in &order {
			store.remove(fetches[which].take().unwrap()).unwrap();
			removed[which] = true;
			// What is left: every item of a fetch not yet removed, and the one never fetched.
			let mut left: Vec<Value> = (0..3)
				.filter(|&f| !removed[f])
				.flat_map(|f| tagged(2 * f..2 * f + 2))
				.collect();
			left.extend(tagged(6..7));
			assert_eq!(available(store), tagged(6..7), "{order:?}");
			let fetched_back: Vec<Value> = fetches
				.iter_mut()
				.filter_map(Option::take)
				.flat_map(|items| items.values())
				.collect();
			// Released, then fetched again as one: the same remaining items, in order.
			let again = store.fetch(100, 1000).unwrap().unwrap();
			assert_eq!(again.values(), left, "{order:?}");
			assert_eq!(fetched_back.len() + 1, left.len());
			drop(again);
			// Fetch the remaining groups again so the next removal has its Items back.
			for f in 0..3 {
				if !removed[f] {
					let items = store.fetch(2, 1000).unwrap().unwrap();
					assert_eq!(items.values(), tagged(2 * f..2 * f + 2), "{order:?}");
					fetches[f] = Some(items);
				}
			}
		}
		assert_eq!(drain(store, 100, 1000).concat(), tagged(6..7), "{order:?}");
	}
}

pub(crate) fn reset_clears_pending_and_old_items_match_nothing(store: &mut dyn DataStore) {
	append_all(store, &tagged(0..4));
	let old_released = store.fetch(2, 1000).unwrap().unwrap();
	let old_removed = store.fetch(2, 1000).unwrap().unwrap();
	assert!(!store.has_data());
	store.reset();
	assert!(!store.has_data());
	assert!(store.fetch(10, 1000).unwrap().is_none());

	append_all(store, &tagged(10..13));
	let new_pending = store.fetch(1, 1000).unwrap().unwrap();
	assert_eq!(new_pending.values(), tagged(10..11));
	// Neither old Items touches the new items: releasing one makes nothing available
	// that is pending, removing the other deletes nothing.
	old_released.release();
	assert_eq!(available(store), tagged(11..13));
	store.remove(old_removed).unwrap();
	assert_eq!(available(store), tagged(11..13));
	drop(new_pending);
	assert_eq!(drain(store, 10, 1000).concat(), tagged(10..13));
}

pub(crate) fn append_after_reset_continues_in_order(store: &mut dyn DataStore) {
	append_all(store, &tagged(0..3));
	store.reset();
	store.reset();
	append_all(store, &tagged(3..6));
	assert_eq!(drain(store, 2, 1000).concat(), tagged(3..6));
	store.reset();
	assert!(!store.has_data());
}
