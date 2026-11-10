//! `remove` acts only on removables this store instance issued; another instance's,
//! with the same sequence numbers, are ignored, and that is not an error.

use super::support::*;
use super::*;
use crate::Equivalent;

fn store_holding(items: &[Value]) -> Result<MemoryStore> {
	let mut store = store(4096);
	for item in items {
		store.append(item.clone())?;
	}
	Ok(store)
}

fn queued(store: &MemoryStore) -> Vec<Value> {
	store.items.iter().map(|item| item.value.clone()).collect()
}

fn fetch(store: &mut MemoryStore, count: usize) -> Result<Vec<Box<dyn Equivalent>>> {
	Ok(store.fetch(Some(count), None)?.unwrap().removable.unwrap())
}

fn items() -> Vec<Value> {
	(0..5).map(|i| json!({"i": i})).collect()
}

#[test]
fn test_another_stores_removables_remove_nothing() -> Result<()> {
	// Same config and same appends: the same sequence numbers in both.
	let (mut a, mut b) = (store_holding(&items())?, store_holding(&items())?);
	let a_removable = fetch(&mut a, 3)?;
	let b_removable = fetch(&mut b, 5)?;

	b.remove(&a_removable)?;
	assert_eq!(queued(&b), items());
	a.remove(&b_removable)?;
	assert_eq!(queued(&a), items());

	// Each store's own removables still remove exactly what it fetched.
	a.remove(&a_removable)?;
	assert_eq!(queued(&a), items()[3..]);
	assert_eq!(queued(&b), items());
	b.remove(&b_removable)?;
	assert!(!b.has_data());
	assert_eq!(queued(&a), items()[3..]);
	Ok(())
}

#[test]
fn test_a_mix_removes_only_the_stores_own_items() -> Result<()> {
	let (mut a, mut b) = (store_holding(&items())?, store_holding(&items())?);
	let mut mixed = fetch(&mut a, 5)?;
	mixed.extend(fetch(&mut b, 2)?);
	b.remove(&mixed)?;
	assert_eq!(queued(&b), items()[2..]);
	assert_eq!(queued(&a), items());
	Ok(())
}

#[test]
fn test_every_instance_has_its_own_identity() {
	let stores: Vec<MemoryStore> = (0..10).map(|_| store(4096)).collect();
	let mut ids: Vec<u64> = stores.iter().map(|s| s.id).collect();
	ids.sort();
	ids.dedup();
	assert_eq!(ids.len(), stores.len());
}
