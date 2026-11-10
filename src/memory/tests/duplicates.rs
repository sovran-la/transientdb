//! Identical items are distinct entries: `remove` deletes exactly what a fetch returned.

use super::*;
use crate::Equivalent;

fn store_of(max_items: usize) -> MemoryStore {
	MemoryStore::new(MemoryConfig {
		write_key: "test-key".to_string(),
		max_items: Some(max_items),
		max_fetch_size: 4096,
		max_fetch_items: None,
	})
}

fn queued(store: &MemoryStore) -> Vec<Value> {
	store.items.iter().map(|item| item.value.clone()).collect()
}

fn append_all(store: &mut MemoryStore, items: &[Value]) -> Result<()> {
	items.iter().try_for_each(|item| store.append(item.clone()))
}

type Removables = Vec<Box<dyn Equivalent>>;

/// Fetches up to `count` items; returns them and their removables.
fn fetch(store: &mut MemoryStore, count: usize) -> Result<(Vec<Value>, Removables)> {
	let result = store.fetch(Some(count), None)?.expect("data");
	let items = result.data.unwrap()["batch"].as_array().unwrap().clone();
	Ok((items, result.removable.unwrap()))
}

#[test]
fn test_removing_one_fetched_duplicate_keeps_the_rest() -> Result<()> {
	let mut store = store_of(100);
	let same = json!({"event": "click"});
	append_all(&mut store, &vec![same.clone(); 4])?;

	let (items, removable) = fetch(&mut store, 1)?;
	assert_eq!(items, vec![same.clone()]);
	store.remove(&removable)?;
	assert_eq!(queued(&store), vec![same.clone(); 3]);

	let (items, removable) = fetch(&mut store, 2)?;
	assert_eq!(items, vec![same.clone(); 2]);
	store.remove(&removable)?;
	assert_eq!(queued(&store), vec![same]);
	Ok(())
}

#[test]
fn test_walking_the_store_returns_every_duplicate() -> Result<()> {
	let mut store = store_of(100);
	let (a, b) = (json!("a"), json!({"b": 1}));
	let appended = vec![
		a.clone(),
		a.clone(),
		b.clone(),
		a.clone(),
		b.clone(),
		b.clone(),
		a,
	];
	append_all(&mut store, &appended)?;

	let mut seen = Vec::new();
	while let Some(result) = store.fetch(Some(2), None)? {
		seen.extend(result.data.unwrap()["batch"].as_array().unwrap().clone());
		store.remove(&result.removable.unwrap())?;
	}
	assert_eq!(seen, appended);
	assert!(!store.has_data());
	Ok(())
}

#[test]
fn test_removing_twice_is_a_noop() -> Result<()> {
	let mut store = store_of(100);
	let same = json!("dup");
	append_all(&mut store, &[same.clone(), same.clone(), json!("other")])?;

	let (_, removable) = fetch(&mut store, 1)?;
	store.remove(&removable)?;
	store.remove(&removable)?;
	assert_eq!(queued(&store), vec![same, json!("other")]);
	Ok(())
}

#[test]
fn test_removing_an_item_evicted_since_fetch_is_a_noop() -> Result<()> {
	let mut store = store_of(3);
	let (a, b, c) = (json!("a"), json!("b"), json!("c"));
	append_all(&mut store, &[a.clone(), b.clone(), c.clone()])?;

	let (items, removable) = fetch(&mut store, 1)?;
	assert_eq!(items, vec![a.clone()]);
	// Drop-oldest evicts the fetched `a`; an identical `a` arrives behind it.
	store.append(a.clone())?;
	assert_eq!(queued(&store), vec![b.clone(), c.clone(), a.clone()]);

	store.remove(&removable)?;
	assert_eq!(queued(&store), vec![b, c, a]);
	Ok(())
}

#[test]
fn test_distinct_items_are_removed_as_before() -> Result<()> {
	let mut store = store_of(100);
	let appended: Vec<Value> = (0..10).map(|i| json!({"i": i})).collect();
	append_all(&mut store, &appended)?;

	let (items, removable) = fetch(&mut store, 3)?;
	assert_eq!(items, appended[..3]);
	store.remove(&removable)?;
	assert_eq!(queued(&store), appended[3..]);

	store.remove(&[])?;
	assert_eq!(queued(&store), appended[3..]);
	Ok(())
}

#[test]
fn test_removables_that_are_not_from_fetch_remove_nothing() -> Result<()> {
	let mut store = store_of(100);
	append_all(&mut store, &[json!("a"), json!("b")])?;

	// An equal value is not a fetched item; nor is another store's removable type.
	let foreign: Vec<Box<dyn Equivalent>> = vec![
		Box::new(json!("a")),
		Box::new(std::path::PathBuf::from("0-events")),
	];
	store.remove(&foreign)?;
	assert_eq!(queued(&store), vec![json!("a"), json!("b")]);
	Ok(())
}

#[test]
fn test_sequence_numbers_stop_before_wrapping() -> Result<()> {
	let mut store = store_of(100);
	store.append(json!("first"))?;
	store.next_seq = u64::MAX - 1;
	store.append(json!("last"))?;
	assert_eq!(store.items.back().unwrap().seq, u64::MAX - 1);
	// The next item would need a sequence number the first item could end up sharing.
	for _ in 0..3 {
		let err = store.append(json!("one too many")).unwrap_err();
		assert!(err.to_string().contains("exhausted"), "{err}");
	}
	assert_eq!(store.next_seq, u64::MAX);
	assert_eq!(queued(&store), vec![json!("first"), json!("last")]);

	// What is queued is still delivered and removed exactly.
	let (items, removable) = fetch(&mut store, 1)?;
	assert_eq!(items, vec![json!("first")]);
	store.remove(&removable)?;
	assert_eq!(queued(&store), vec![json!("last")]);
	Ok(())
}
