//! `fetch`: its two limits, its order, and the three views of what it hands over.

use super::*;
use crate::item_samples::{assert_exact, awkward_items};
use serde_json::json;

pub(crate) fn empty_store_hands_over_nothing(store: &mut dyn DataStore) {
	assert!(!store.has_data());
	assert!(store.fetch(10, 1000).unwrap().is_none());
	assert!(store.fetch(usize::MAX, usize::MAX).unwrap().is_none());
	assert!(store.fetch(0, 0).unwrap().is_none());
	store.reset();
	assert!(!store.has_data());
}

pub(crate) fn count_zero_hands_over_nothing(store: &mut dyn DataStore) {
	append_all(store, &tagged(0..3));
	assert!(store.fetch(0, usize::MAX).unwrap().is_none());
	// Nothing was held back by it.
	assert!(store.has_data());
	assert_eq!(available(store), tagged(0..3));
}

pub(crate) fn count_limits_the_items(store: &mut dyn DataStore) {
	append_all(store, &tagged(0..10));
	let first = store.fetch(3, 1000).unwrap().unwrap();
	assert_eq!(first.values(), tagged(0..3));
	let second = store.fetch(1, 1000).unwrap().unwrap();
	assert_eq!(second.values(), tagged(3..4));
	// More than are waiting: all of them.
	let rest = store.fetch(100, 1000).unwrap().unwrap();
	assert_eq!(rest.values(), tagged(4..10));
	assert_eq!(rest.len(), 6);
}

pub(crate) fn max_bytes_at_and_below_the_boundary(store: &mut dyn DataStore) {
	let items: Vec<Value> = (0..3).map(|tag| item_of_len(20, tag)).collect();
	append_all(store, &items);
	let fetched_len = |store: &mut dyn DataStore, max_bytes| {
		store.fetch(10, max_bytes).unwrap().map_or(0, |items| {
			assert!(items.byte_len() <= max_bytes);
			items.len()
		})
	};
	// Item bytes only: no separators or brackets are counted.
	assert_eq!(fetched_len(store, 60), 3);
	assert_eq!(fetched_len(store, 59), 2);
	assert_eq!(fetched_len(store, 41), 2);
	assert_eq!(fetched_len(store, 40), 2);
	assert_eq!(fetched_len(store, 39), 1);
	assert_eq!(fetched_len(store, 20), 1);
	assert_eq!(fetched_len(store, 19), 0);
	assert_eq!(fetched_len(store, 0), 0);
	let all = store.fetch(10, 60).unwrap().unwrap();
	assert_eq!((all.values(), all.byte_len()), (items, 60));
}

pub(crate) fn oldest_that_does_not_fit_hands_over_nothing(store: &mut dyn DataStore) {
	let (large, small) = (item_of_len(40, 0), item_of_len(5, 1));
	append_all(store, &[large.clone(), small.clone()]);
	assert!(store.fetch(10, 39).unwrap().is_none());
	// Nothing is lost or held back for it.
	assert!(store.has_data());
	let first = store.fetch(10, 44).unwrap().unwrap();
	assert_eq!(first.values(), vec![large.clone()]);
	// With the large one pending, the small one is the oldest available.
	let second = store.fetch(10, 5).unwrap().unwrap();
	assert_eq!(second.values(), vec![small.clone()]);
	drop((first, second));
	assert_eq!(available(store), vec![large, small]);
}

pub(crate) fn no_item_jumps_the_queue(store: &mut dyn DataStore) {
	let items = [item_of_len(10, 0), item_of_len(50, 1), item_of_len(5, 2)];
	append_all(store, &items);
	// The 5-byte item would fit beside the first, but the 50-byte one is ahead of it.
	let first = store.fetch(10, 30).unwrap().unwrap();
	assert_eq!(first.values(), items[..1]);
	first.release();
	let all = store.fetch(10, 65).unwrap().unwrap();
	assert_eq!(all.values(), items);
}

pub(crate) fn bytes_are_counted_not_characters(store: &mut dyn DataStore) {
	let words = ["🦀", "漢字", "é", "👾💥", "日本語テキスト", "ascii"];
	let items: Vec<Value> = (0..30)
		.map(|i| json!(words[i % words.len()].repeat(1 + i % 2)))
		.collect();
	append_all(store, &items);
	let texts: Vec<String> = items.iter().map(Value::to_string).collect();
	let first_two = texts[0].len() + texts[1].len();
	let fetched = store.fetch(10, first_two).unwrap().unwrap();
	assert_eq!(fetched.values(), items[..2]);
	assert_eq!(fetched.byte_len(), first_two);
	drop(fetched);
	let batches = drain(store, 100, 40);
	assert!(batches.len() > 1);
	assert_eq!(batches.concat(), items);
}

pub(crate) fn json_values_and_iter_are_exact(store: &mut dyn DataStore) {
	let items = awkward_items();
	append_all(store, &items);
	let fetched = store.fetch(100, usize::MAX).unwrap().unwrap();
	let texts: Vec<String> = items.iter().map(Value::to_string).collect();

	assert_eq!(fetched.len(), items.len());
	assert_eq!(
		fetched.byte_len(),
		texts.iter().map(String::len).sum::<usize>()
	);
	assert_eq!(fetched.iter().collect::<Vec<_>>(), texts);
	assert_exact(&fetched.values(), &items);

	let json = fetched.json();
	assert_eq!(json, format!("[{}]", texts.join(",")));
	assert_eq!(json.len(), fetched.byte_len() + (fetched.len() - 1) + 2);
	let parsed: Vec<Value> = serde_json::from_str(&json).unwrap();
	assert_exact(&parsed, &items);
	for expected in ["18446744073709551615", "42.0", "-0.0", "9007199254740993"] {
		assert!(fetched.iter().any(|text| text == expected), "{expected}");
	}
}

pub(crate) fn json_of_one_item(store: &mut dyn DataStore) {
	store.append(json!({"a": [1, "b"]})).unwrap();
	let fetched = store.fetch(1, 64).unwrap().unwrap();
	assert_eq!(fetched.json(), r#"[{"a":[1,"b"]}]"#);
	assert_eq!(fetched.json().len(), fetched.byte_len() + 2);
	assert_eq!(fetched.iter().collect::<Vec<_>>(), [r#"{"a":[1,"b"]}"#]);
	// Every view can be asked for again, and in any order.
	assert_eq!(fetched.values(), fetched.values());
	assert_eq!(fetched.json(), fetched.json());
}

pub(crate) fn draining_in_batches_returns_everything_in_order(store: &mut dyn DataStore) {
	append_all(store, &tagged(0..20));
	let batches = drain(store, 8, 1000);
	let sizes: Vec<usize> = batches.iter().map(Vec::len).collect();
	assert_eq!(sizes, vec![8, 8, 4]);
	assert_eq!(batches.concat(), tagged(0..20));
}

/// Mixed sizes under several byte limits: each fetch stays within its limits and takes
/// every item that fits, so the next waiting item would have broken one of them.
pub(crate) fn every_fetch_is_as_large_as_the_limits_allow(store: &mut dyn DataStore) {
	let items: Vec<Value> = (0..200)
		.map(|i| item_of_len(5 + (i * 37) % 60, i))
		.collect();
	for (count, max_bytes) in [(usize::MAX, 64), (usize::MAX, 150), (7, 1000), (3, 100)] {
		append_all(store, &items);
		let mut seen = Vec::new();
		while let Some(fetched) = store.fetch(count, max_bytes).unwrap() {
			assert!(fetched.len() <= count && fetched.byte_len() <= max_bytes);
			seen.extend(fetched.values());
			if let Some(next) = items.get(seen.len()) {
				let full = fetched.len() == count;
				let no_room = fetched.byte_len() + next.to_string().len() > max_bytes;
				assert!(full || no_room, "fetch stopped early at {}", seen.len());
			}
			store.remove(fetched).unwrap();
		}
		assert_eq!(seen, items, "count {count}, max_bytes {max_bytes}");
		assert!(!store.has_data());
	}
}
