//! A fetched batch, serialized compactly, never exceeds the byte limit in effect.

use super::support::*;
use super::*;
use crate::envelope::samples::expected_len;
use chrono::{DateTime, Utc};

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
	let result = store.fetch(None, Some(exact)).unwrap().unwrap();
	let batch = serde_json::to_string(result.data.as_ref().unwrap()).unwrap();
	assert_eq!(batch.len(), exact);
	assert_eq!(fetched(&mut store, exact), 3);
	assert_eq!(fetched(&mut store, exact - 1), 2);
}

#[test]
fn test_separators_are_counted() {
	let mut store = store(4096);
	for tag in 0..3 {
		store.append(item_of_len(40, tag)).unwrap();
	}
	// Room for the three items only if the two commas were free.
	assert_eq!(fetched(&mut store, envelope(KEY) + 3 * 40), 2);
	assert_eq!(fetched(&mut store, envelope(KEY) + 3 * 40 + 1), 2);
}

#[test]
fn test_envelope_is_counted() {
	let mut store = store(4096);
	for tag in 0..3 {
		store.append(item_of_len(40, tag)).unwrap();
	}
	// Room for the three items and commas, and all but one byte of the envelope.
	assert_eq!(fetched(&mut store, envelope(KEY) - 1 + 3 * 40 + 2), 2);
	// One item and its envelope, exactly; a byte less fetches nothing.
	assert_eq!(fetched(&mut store, envelope(KEY) + 40), 1);
	assert_eq!(fetched(&mut store, envelope(KEY) + 39), 0);
}

#[test]
fn test_timestamp_width_is_the_same_at_every_instant() {
	let store = store(4096);
	// Whole seconds, whole milliseconds, nanoseconds past them, and the extremes of the
	// four-digit years: the timestamp always prints three fraction digits, so a batch's
	// envelope is the length the store counted whenever it is finished.
	let instants = [
		DateTime::UNIX_EPOCH,
		DateTime::from_timestamp(1_762_691_696, 0).unwrap(),
		DateTime::from_timestamp(1_762_691_696, 120_000_000).unwrap(),
		DateTime::from_timestamp(1_762_691_696, 123_456_789).unwrap(),
		DateTime::from_timestamp(-62_167_219_200, 0).unwrap(),
		DateTime::from_timestamp(253_402_300_799, 999_999_999).unwrap(),
		Utc::now(),
	];
	for at in instants {
		let batch = store.config.envelope.batch(Vec::new(), at);
		let len = serde_json::to_string(&batch).unwrap().len();
		assert_eq!(len, store.envelope_len, "{at}");
		assert_eq!(len, envelope(KEY), "{at}");
	}
}

#[test]
fn test_longer_field_value_fits_fewer_items() -> Result<()> {
	let long_source = "k".repeat(60);
	let mut sizes = Vec::new();
	for source in [KEY, long_source.as_str()] {
		let mut store = store_with(400, with_source(source));
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
		envelope: Envelope::default(),
		max_items: Some(1000),
		max_fetch_size: 1000,
		max_fetch_items: Some(6),
	});
	for tag in 0..100 {
		store.append(item_of_len(40, tag))?;
	}
	let count = |store: &mut MemoryStore, count, max_bytes| {
		let result = store.fetch(count, max_bytes).unwrap().unwrap();
		checked_batch_with(&result, max_bytes.unwrap_or(1000), &Envelope::default()).len()
	};
	// count wins
	assert_eq!(count(&mut store, Some(2), None), 2);
	// max_fetch_items wins
	assert_eq!(count(&mut store, Some(50), None), 6);
	assert_eq!(count(&mut store, None, None), 6);
	// bytes win: the envelope plus 3 items and 2 commas, short of a 4th
	let three = expected_len(&Envelope::default()) + 3 * 40 + 2;
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
		envelope: Envelope::default(),
		max_items: Some(10),
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
