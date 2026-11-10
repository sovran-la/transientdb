//! `end_batch` sets a boundary no fetched batch crosses: a fetch returns only items
//! appended under the oldest item's batch number.

use super::support::*;
use super::*;

pub(super) fn tag(n: usize) -> Value {
	json!({"tag": n})
}

pub(super) fn tags(items: &[Value]) -> Vec<usize> {
	items
		.iter()
		.map(|v| v["tag"].as_u64().unwrap() as usize)
		.collect()
}

pub(super) fn append_tags(store: &mut MemoryStore, range: std::ops::Range<usize>) -> Result<()> {
	for n in range {
		store.append(tag(n))?;
	}
	Ok(())
}

pub(super) fn store_with_limits(
	max_items: Option<usize>,
	max_fetch_items: Option<usize>,
) -> MemoryStore {
	MemoryStore::new(MemoryConfig {
		envelope: with_source(KEY),
		max_items,
		max_fetch_size: 64 * 1024,
		max_fetch_items,
	})
}

/// Drains the store with `count` and returns the tags of each batch.
pub(super) fn drained(
	store: &mut MemoryStore,
	count: Option<usize>,
	max_bytes: Option<usize>,
) -> Result<Vec<Vec<usize>>> {
	Ok(drain(store, count, max_bytes)?
		.iter()
		.map(|b| tags(b))
		.collect())
}

/// The tags of one fetch, removing nothing.
fn peek(store: &mut MemoryStore, count: Option<usize>) -> Result<Vec<usize>> {
	let limit = store.config.max_fetch_size;
	Ok(match store.fetch(count, None)? {
		Some(result) => tags(&checked_batch(&result, limit)),
		None => Vec::new(),
	})
}

#[test]
fn test_items_before_and_after_never_share_a_batch() -> Result<()> {
	let mut store = store(4096);
	append_tags(&mut store, 0..2)?;
	store.end_batch()?;
	append_tags(&mut store, 2..4)?;
	assert_eq!(
		drained(&mut store, None, None)?,
		vec![vec![0, 1], vec![2, 3]]
	);
	Ok(())
}

#[test]
fn test_a_fetch_stops_at_the_boundary_until_removed() -> Result<()> {
	let mut store = store(4096);
	append_tags(&mut store, 0..3)?;
	store.end_batch()?;
	append_tags(&mut store, 3..5)?;

	// A large count still stops at the boundary, every time until removed.
	for _ in 0..3 {
		assert_eq!(peek(&mut store, Some(1000))?, vec![0, 1, 2]);
	}
	let result = store.fetch(Some(1000), None)?.unwrap();
	assert_eq!(result.removable.as_ref().unwrap().len(), 3);
	store.remove(&result.removable.unwrap())?;
	assert_eq!(peek(&mut store, Some(1000))?, vec![3, 4]);
	Ok(())
}

#[test]
fn test_several_boundaries_make_one_batch_per_span() -> Result<()> {
	let mut store = store(4096);
	append_tags(&mut store, 0..1)?;
	store.end_batch()?;
	append_tags(&mut store, 1..3)?;
	store.end_batch()?;
	append_tags(&mut store, 3..6)?;
	store.end_batch()?;
	append_tags(&mut store, 6..7)?;
	assert_eq!(
		drained(&mut store, None, None)?,
		vec![vec![0], vec![1, 2], vec![3, 4, 5], vec![6]]
	);
	Ok(())
}

#[test]
fn test_nothing_to_end_changes_nothing() -> Result<()> {
	let mut store = store(4096);

	// A new store.
	store.end_batch()?;
	store.end_batch()?;
	assert_eq!(store.current_batch, 0);
	assert!(!store.has_data());
	assert!(store.fetch(None, None)?.is_none());

	// After everything was removed.
	append_tags(&mut store, 0..2)?;
	drain(&mut store, None, None)?;
	let current = store.current_batch;
	store.end_batch()?;
	store.end_batch()?;
	assert_eq!(store.current_batch, current);
	assert!(!store.has_data());

	// After reset.
	append_tags(&mut store, 2..4)?;
	store.reset();
	let current = store.current_batch;
	store.end_batch()?;
	store.end_batch()?;
	assert_eq!(store.current_batch, current);
	assert!(!store.has_data());

	// The next appends form one batch.
	append_tags(&mut store, 4..7)?;
	assert_eq!(drained(&mut store, None, None)?, vec![vec![4, 5, 6]]);
	Ok(())
}

#[test]
fn test_repeated_end_batch_is_one_boundary() -> Result<()> {
	let mut store = store(4096);
	append_tags(&mut store, 0..2)?;
	store.end_batch()?;
	let after_first = store.current_batch;
	for _ in 0..3 {
		store.end_batch()?;
	}
	assert_eq!(store.current_batch, after_first);
	append_tags(&mut store, 2..3)?;
	store.end_batch()?;
	store.end_batch()?;
	assert_eq!(store.current_batch, after_first + 1);
	assert_eq!(drained(&mut store, None, None)?, vec![vec![0, 1], vec![2]]);
	Ok(())
}

#[test]
fn test_drop_oldest_keeps_batch_numbers() -> Result<()> {
	let mut store = store_with_limits(Some(4), None);
	append_tags(&mut store, 0..3)?;
	store.end_batch()?;
	append_tags(&mut store, 3..6)?;
	// 0 and 1 were dropped; 2 is still alone before the boundary.
	assert_eq!(
		tags(
			&store
				.items
				.iter()
				.map(|i| i.value.clone())
				.collect::<Vec<_>>()
		),
		vec![2, 3, 4, 5]
	);
	assert_eq!(
		drained(&mut store, None, None)?,
		vec![vec![2], vec![3, 4, 5]]
	);

	// The whole span before a boundary dropped: the boundary goes with it.
	append_tags(&mut store, 6..7)?;
	store.end_batch()?;
	append_tags(&mut store, 7..11)?;
	assert_eq!(drained(&mut store, None, None)?, vec![vec![7, 8, 9, 10]]);
	Ok(())
}

#[test]
fn test_partial_removal_continues_the_same_span() -> Result<()> {
	let mut store = store(4096);
	append_tags(&mut store, 0..4)?;
	store.end_batch()?;
	append_tags(&mut store, 4..6)?;
	let result = store.fetch(Some(3), None)?.unwrap();
	store.remove(&result.removable.unwrap())?;
	assert_eq!(peek(&mut store, None)?, vec![3]);
	Ok(())
}

#[test]
fn test_boundary_does_not_leak_across_reset() -> Result<()> {
	let mut store = store(4096);
	append_tags(&mut store, 0..1)?;
	store.end_batch()?;
	append_tags(&mut store, 1..2)?;
	store.reset();
	append_tags(&mut store, 2..4)?;
	store.end_batch()?;
	append_tags(&mut store, 4..5)?;
	assert_eq!(drained(&mut store, None, None)?, vec![vec![2, 3], vec![4]]);

	// A boundary ended right before reset leaves nothing behind either.
	append_tags(&mut store, 5..6)?;
	store.end_batch()?;
	store.reset();
	append_tags(&mut store, 6..8)?;
	assert_eq!(drained(&mut store, None, None)?, vec![vec![6, 7]]);
	Ok(())
}

#[test]
fn test_batch_numbers_never_wrap() -> Result<()> {
	let mut store = store(4096);
	store.current_batch = u64::MAX - 1;
	append_tags(&mut store, 0..1)?;
	store.end_batch()?;
	assert_eq!(store.current_batch, u64::MAX);
	append_tags(&mut store, 1..2)?;
	for _ in 0..3 {
		let err = store.end_batch().unwrap_err();
		assert!(err.to_string().contains("batch numbers exhausted"), "{err}");
		assert_eq!(store.current_batch, u64::MAX);
	}
	// Nothing was corrupted: appends still work, under the last batch number.
	append_tags(&mut store, 2..3)?;
	assert_eq!(
		store.items.iter().map(|i| i.batch).collect::<Vec<_>>(),
		vec![u64::MAX - 1, u64::MAX, u64::MAX]
	);
	assert_eq!(drained(&mut store, None, None)?, vec![vec![0], vec![1, 2]]);
	Ok(())
}
