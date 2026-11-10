//! `max_items`: `None` never drops an item for capacity; `Some(n)` keeps the newest `n`.

use super::support::*;
use super::*;
use std::collections::VecDeque;

fn store_capped(
	max_items: Option<usize>,
	max_fetch_size: usize,
	max_fetch_items: Option<usize>,
) -> MemoryStore {
	MemoryStore::new(MemoryConfig {
		envelope: with_source(KEY),
		max_items,
		max_fetch_size,
		max_fetch_items,
	})
}

fn tagged(range: std::ops::Range<usize>) -> Vec<Value> {
	range.map(|i| json!({"i": i})).collect()
}

fn append_all(store: &mut MemoryStore, items: &[Value]) -> Result<()> {
	items.iter().try_for_each(|item| store.append(item.clone()))
}

fn queued(store: &MemoryStore) -> Vec<Value> {
	store.items.iter().map(|item| item.value.clone()).collect()
}

#[test]
fn test_unbounded_never_drops() -> Result<()> {
	let mut store = store_capped(None, 8 * 1024, None);
	let appended = tagged(0..5000);
	append_all(&mut store, &appended)?;
	assert_eq!(store.items.len(), 5000);

	let batches = drain(&mut store, None, None)?;
	assert!(batches.len() > 1, "expected several batches");
	assert_eq!(batches.concat(), appended);
	Ok(())
}

#[test]
fn test_unbounded_batches_still_limited_by_item_limit() -> Result<()> {
	let mut store = store_capped(None, 64 * 1024, Some(8));
	let appended: Vec<Value> = (0..2000).map(|tag| item_of_len(20, tag)).collect();
	append_all(&mut store, &appended)?;

	let batches = drain(&mut store, None, None)?;
	assert_eq!(batches.len(), 250);
	assert!(batches.iter().all(|batch| batch.len() == 8));
	assert_eq!(batches.concat(), appended);
	Ok(())
}

#[test]
fn test_unbounded_batches_still_limited_by_bytes() -> Result<()> {
	// Room for 5 items of 20 bytes and their separators, with 15 bytes to spare, not 6.
	let max_fetch_size = envelope(KEY) + 5 * 20 + 4 + 15;
	let mut store = store_capped(None, max_fetch_size, Some(8));
	let appended: Vec<Value> = (0..2000).map(|tag| item_of_len(20, tag)).collect();
	append_all(&mut store, &appended)?;

	// `drain` checks every batch against the ceiling.
	let batches = drain(&mut store, None, None)?;
	assert!(batches.iter().all(|batch| batch.len() == 5));
	assert_eq!(batches.concat(), appended);

	// A smaller per-call ceiling still applies too.
	append_all(&mut store, &appended)?;
	let per_call = envelope(KEY) + 2 * 20 + 1 + 15;
	let batches = drain(&mut store, None, Some(per_call))?;
	assert!(batches.iter().all(|batch| batch.len() == 2));
	assert_eq!(batches.concat(), appended);
	Ok(())
}

#[test]
fn test_cap_of_one_holds_only_the_newest_item() -> Result<()> {
	let mut store = store_capped(Some(1), 1024, None);
	for item in tagged(0..5) {
		store.append(item.clone())?;
		assert_eq!(queued(&store), vec![item]);
	}
	assert_eq!(drain(&mut store, None, None)?.concat(), tagged(4..5));
	Ok(())
}

#[test]
fn test_unbounded_append_remove_cycles_keep_counters_and_contents() -> Result<()> {
	let mut store = store_capped(None, 64 * 1024, None);
	let mut expected: VecDeque<Value> = VecDeque::new();
	let mut next = 0;
	for cycle in 0..50 {
		let items = tagged(next..next + 100);
		next += 100;
		append_all(&mut store, &items)?;
		expected.extend(items);

		// Remove fewer than were appended, so the store keeps growing.
		let result = store.fetch(Some(60 + cycle % 7), None)?.unwrap();
		let fetched = checked_batch(&result, 64 * 1024);
		let oldest: Vec<Value> = expected.drain(..fetched.len()).collect();
		assert_eq!(fetched, oldest);
		store.remove(&result.removable.unwrap())?;

		assert_eq!(queued(&store), Vec::from(expected.clone()));
		assert_eq!(store.next_seq, next as u64);
		let seqs: Vec<u64> = store.items.iter().map(|item| item.seq).collect();
		assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
	}
	assert!(store.items.len() > 1000, "the store should have grown");
	assert_eq!(drain(&mut store, None, None)?.concat(), Vec::from(expected));
	Ok(())
}
