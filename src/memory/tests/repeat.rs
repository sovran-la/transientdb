//! MemoryStore has no recovery step; its counterpart is that repeating a fetch, which
//! closes a batch, never changes or duplicates what is queued.

use super::support::*;
use super::*;

#[test]
fn test_repeated_fetches_change_nothing() -> Result<()> {
	let mut store = store(4096);
	let appended: Vec<Value> = (0..5).map(|tag| item_of_len(20, tag)).collect();
	for item in &appended {
		store.append(item.clone())?;
	}
	let before = store.items.clone();

	for _ in 0..4 {
		let result = store.fetch(None, None)?.unwrap();
		assert_eq!(checked_batch(&result, 4096), appended);
		assert_eq!(store.items, before);
	}
	assert_eq!(drain(&mut store, None, None)?.concat(), appended);
	Ok(())
}
