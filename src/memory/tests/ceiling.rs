//! A fetched batch, serialized compactly, never exceeds the byte limit in effect.

use super::support::*;
use super::*;
use crate::batch;

#[test]
fn test_ceiling_holds_for_mixed_sizes_and_drains_in_order() -> Result<()> {
	for max in [100usize, 150, 256, 1000, 4096] {
		let mut store = store(max);
		let largest = store.max_item_size;
		let appended: Vec<Value> = (0..200)
			.map(|i| item_of_len(5 + (i * 37) % (largest - 4), i))
			.collect();
		for item in &appended {
			store.append(item.clone())?;
		}

		let batches = drain(&mut store, None, None)?;
		assert!(batches.len() > 1, "max={max} should take several batches");
		assert_eq!(batches.concat(), appended, "max={max}");
	}
	Ok(())
}

#[test]
fn test_exact_fit_and_one_byte_over() {
	let mut store = store(4096);
	for tag in 0..3 {
		store.append(item_of_len(40, tag)).unwrap();
	}
	let exact = envelope(KEY) + 3 * 40 + 2;
	let result = store
		.fetch_with_sent_at(None, Some(exact), SENT_AT)
		.unwrap();
	let batch = serde_json::to_string(result.data.as_ref().unwrap()).unwrap();
	assert_eq!(batch.len(), exact);
	assert_eq!(fetched_at_fixed_time(&mut store, exact), 3);
	assert_eq!(fetched_at_fixed_time(&mut store, exact - 1), 2);
}

#[test]
fn test_separators_are_counted() {
	let mut store = store(4096);
	for tag in 0..3 {
		store.append(item_of_len(40, tag)).unwrap();
	}
	// Room for the three items only if the two commas were free.
	assert_eq!(fetched_at_fixed_time(&mut store, envelope(KEY) + 3 * 40), 2);
	assert_eq!(
		fetched_at_fixed_time(&mut store, envelope(KEY) + 3 * 40 + 1),
		2
	);
}

#[test]
fn test_envelope_is_counted() {
	let mut store = store(4096);
	for tag in 0..3 {
		store.append(item_of_len(40, tag)).unwrap();
	}
	// Room for the three items and commas, and all but one byte of the envelope.
	assert_eq!(
		fetched_at_fixed_time(&mut store, envelope(KEY) - 1 + 3 * 40 + 2),
		2
	);
	// One item and its envelope, exactly; a byte less fetches nothing.
	assert_eq!(fetched_at_fixed_time(&mut store, envelope(KEY) + 40), 1);
	assert_eq!(fetched_at_fixed_time(&mut store, envelope(KEY) + 39), 0);
}

#[test]
fn test_actual_timestamp_width_is_measured() {
	let mut store = store(4096);
	for tag in 0..3 {
		store.append(item_of_len(40, tag)).unwrap();
	}
	// Whole seconds print no fraction; nonzero nanoseconds print nine digits.
	let short = "2026-11-09T12:34:56+00:00";
	let wide = "2026-11-09T12:34:56.123456789+00:00";
	let max = batch::envelope_len(short, KEY) + 3 * 40 + 2;
	let count = |store: &mut MemoryStore, sent_at| {
		let result = store.fetch_with_sent_at(None, Some(max), sent_at).unwrap();
		checked_batch(&result, max).len()
	};
	assert_eq!(count(&mut store, short), 3);
	assert_eq!(count(&mut store, wide), 2);
	assert_eq!(
		batch::envelope_len(wide, KEY),
		MemoryStore::widest_envelope_len(KEY)
	);
}

#[test]
fn test_longer_write_key_fits_fewer_items() -> Result<()> {
	let long_key = "k".repeat(60);
	let mut sizes = Vec::new();
	for key in [KEY, long_key.as_str()] {
		let mut store = store_with(400, key);
		let appended: Vec<Value> = (0..30).map(|tag| item_of_len(40, tag)).collect();
		for item in &appended {
			store.append(item.clone())?;
		}
		let batches = drain(&mut store, None, None)?;
		assert_eq!(batches.concat(), appended);
		sizes.push(batches[0].len());
	}
	assert!(sizes[1] < sizes[0], "{sizes:?}");
	Ok(())
}

#[test]
fn test_multibyte_utf8_is_measured_in_bytes() -> Result<()> {
	let mut store = store(256);
	let words = ["🦀", "漢字", "é", "👾💥", "日本語テキスト", "ascii"];
	let appended: Vec<Value> = (0..120)
		.map(|i| json!({"i": i, "s": words[i % words.len()].repeat(1 + i % 7)}))
		.collect();
	for item in &appended {
		store.append(item.clone())?;
	}
	let chars: usize = appended.iter().map(|v| v.to_string().chars().count()).sum();
	let bytes: usize = appended.iter().map(item_len).sum();
	assert!(bytes > chars, "test data must be multi-byte");

	assert_eq!(drain(&mut store, None, None)?.concat(), appended);
	Ok(())
}

#[test]
fn test_per_call_max_bytes_is_the_ceiling() -> Result<()> {
	for max_bytes in [150usize, 300, 2000] {
		let mut store = store(500);
		let appended: Vec<Value> = (0..60).map(|i| item_of_len(20 + i % 30, i)).collect();
		for item in &appended {
			store.append(item.clone())?;
		}
		// Smaller and larger than the configured 500: each fetch obeys max_bytes.
		let batches = drain(&mut store, None, Some(max_bytes))?;
		assert_eq!(batches.concat(), appended, "max_bytes={max_bytes}");
	}
	Ok(())
}

#[test]
fn test_count_item_limit_and_bytes_each_win() -> Result<()> {
	let mut store = MemoryStore::new(MemoryConfig {
		write_key: KEY.to_string(),
		max_items: 1000,
		max_fetch_size: 1000,
		max_fetch_items: Some(6),
	});
	for tag in 0..100 {
		store.append(item_of_len(40, tag))?;
	}
	let count = |store: &mut MemoryStore, count, max_bytes| {
		let result = store.fetch(count, max_bytes).unwrap().unwrap();
		checked_batch(&result, max_bytes.unwrap_or(1000)).len()
	};
	// count wins
	assert_eq!(count(&mut store, Some(2), None), 2);
	// max_fetch_items wins
	assert_eq!(count(&mut store, Some(50), None), 6);
	assert_eq!(count(&mut store, None, None), 6);
	// bytes win: the widest envelope plus 3 items and 2 commas, short of a 4th
	let three = MemoryStore::widest_envelope_len(KEY) + 3 * 40 + 2;
	assert_eq!(count(&mut store, Some(50), Some(three)), 3);
	assert_eq!(count(&mut store, None, Some(three)), 3);

	let batches = drain(&mut store, Some(4), Some(three))?;
	assert!(batches.iter().all(|b| b.len() <= 3));
	assert_eq!(batches.concat().len(), 100);
	Ok(())
}

#[test]
fn test_max_items_drop_oldest_with_ceiling() -> Result<()> {
	let mut store = MemoryStore::new(MemoryConfig {
		write_key: KEY.to_string(),
		max_items: 10,
		max_fetch_size: 200,
		max_fetch_items: None,
	});
	let appended: Vec<Value> = (0..15).map(|tag| item_of_len(30, tag)).collect();
	for item in &appended {
		store.append(item.clone())?;
	}
	assert_eq!(store.items.len(), 10);
	let batches = drain(&mut store, None, None)?;
	assert!(batches.len() > 1);
	assert_eq!(batches.concat(), appended[5..]);
	Ok(())
}
