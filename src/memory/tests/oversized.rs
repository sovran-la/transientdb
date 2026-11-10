//! Items too large for any batch are rejected at append, so nothing can wedge the queue.

use super::support::*;
use super::*;

#[test]
fn test_oversized_append_leaves_store_unchanged() -> Result<()> {
	let mut store = store(100);
	let err = store.append(item_of_len(500, 0)).unwrap_err();
	assert_invalid_input(&err);
	assert!(!store.has_data());
	assert!(store.fetch(None, None)?.is_none());

	store.append(json!("a"))?;
	store.append(json!("b"))?;
	let before = store.items.clone();
	assert_invalid_input(&store.append(item_of_len(500, 1)).unwrap_err());
	assert_eq!(store.items, before);
	assert!(store.has_data());
	Ok(())
}

#[test]
fn test_oversized_error_message_states_sizes() {
	let mut store = store(100);
	let largest = store.max_item_size;
	let message = store.append(item_of_len(1234, 0)).unwrap_err().to_string();
	assert!(message.contains("1234 bytes"), "{message}");
	assert!(message.contains(&format!("{largest} bytes")), "{message}");
	assert!(message.contains("max_fetch_size = 100 bytes"), "{message}");
}

#[test]
fn test_largest_item_accepted_and_fetchable_alone() -> Result<()> {
	for max in [100usize, 101, 512, 4096] {
		let mut store = store(max);
		let largest = store.max_item_size;
		assert_eq!(largest, max - MemoryStore::widest_envelope_len(KEY));

		assert_invalid_input(&store.append(item_of_len(largest + 1, 0)).unwrap_err());
		store.append(item_of_len(largest, 1))?;
		store.append(item_of_len(10, 2))?;
		assert_invalid_input(&store.append(item_of_len(largest + 1, 3)).unwrap_err());

		let batches = drain(&mut store, None, None)?;
		assert_eq!(batches[0], vec![item_of_len(largest, 1)], "max={max}");
		assert_eq!(batches.concat().len(), 2);
	}
	Ok(())
}

#[test]
fn test_rejections_mixed_with_accepted_items_never_wedge() -> Result<()> {
	let mut store = store(150);
	let mut accepted = Vec::new();
	for i in 0..60 {
		if i % 3 == 1 {
			let item = json!({"rejected": "r".repeat(70 + i)});
			assert_invalid_input(&store.append(item).unwrap_err());
		} else {
			let item = json!({"i": i, "pad": "p".repeat(i % 30)});
			store.append(item.clone())?;
			accepted.push(item);
		}
	}

	let items = drain(&mut store, None, None)?.concat();
	assert_eq!(items, accepted);
	assert!(items.iter().all(|item| item.get("rejected").is_none()));
	Ok(())
}

#[test]
fn test_small_per_call_max_bytes_returns_none_without_wedging() -> Result<()> {
	let mut store = store(300);
	let head = item_of_len(150, 0);
	store.append(head.clone())?;
	store.append(item_of_len(10, 1))?;

	// The caller's budget is too small for the head item: nothing, and nothing lost.
	assert!(store.fetch(None, Some(150))?.is_none());
	assert!(store.has_data());
	assert_eq!(store.items.len(), 2);

	// The configured limit always fits it.
	let result = store.fetch(None, None)?.unwrap();
	assert_eq!(checked_batch(&result, 300)[0], head);
	Ok(())
}

#[test]
fn test_rejected_item_does_not_trigger_drop_oldest() -> Result<()> {
	let mut store = MemoryStore::new(MemoryConfig {
		write_key: KEY.to_string(),
		max_items: 3,
		max_fetch_size: 200,
		max_fetch_items: None,
	});
	for tag in 0..3 {
		store.append(item_of_len(20, tag))?;
	}
	assert_invalid_input(&store.append(item_of_len(500, 9)).unwrap_err());
	let items = drain(&mut store, None, None)?.concat();
	assert_eq!(
		items,
		(0..3).map(|tag| item_of_len(20, tag)).collect::<Vec<_>>()
	);
	Ok(())
}

#[test]
fn test_reset_after_rejection() -> Result<()> {
	let mut store = store(100);
	store.append(json!("first"))?;
	assert_invalid_input(&store.append(item_of_len(500, 0)).unwrap_err());
	store.reset();
	assert!(!store.has_data());
	store.append(json!("second"))?;
	assert_eq!(
		drain(&mut store, None, None)?.concat(),
		vec![json!("second")]
	);
	Ok(())
}

#[test]
#[should_panic(
	expected = "max_fetch_size is all envelope and no room for an item? That's packaging, not a batch."
)]
fn test_rejects_envelope_that_leaves_no_room() {
	let envelope = MemoryStore::widest_envelope_len("");
	// The widest envelope with this key is exactly 100 bytes.
	let _ = store_with(100, &"k".repeat(100 - envelope));
}

#[test]
fn test_envelope_leaving_one_byte_is_accepted() -> Result<()> {
	let envelope = MemoryStore::widest_envelope_len("");
	let mut store = store_with(100, &"k".repeat(99 - envelope));
	assert_eq!(store.max_item_size, 1);
	assert_invalid_input(&store.append(json!(10)).unwrap_err());
	store.append(json!(7))?;
	assert_eq!(drain(&mut store, None, None)?.concat(), vec![json!(7)]);
	Ok(())
}
