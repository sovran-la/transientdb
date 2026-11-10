//! `write_key` round-trips through the batch envelope whatever it contains, and the
//! byte ceiling counts its escaped bytes.

use super::support::*;
use super::*;
use crate::batch::AWKWARD_WRITE_KEYS;
use crate::DataResult;

/// The envelope length for `write_key` at `SENT_AT`, built independently of the store.
fn expected_envelope(write_key: &str) -> usize {
	let key = serde_json::to_string(write_key).unwrap();
	format!("{{\"batch\":[],\"sentAt\":\"{SENT_AT}\",\"writeKey\":{key}}}").len()
}

/// The fetched batch, serialized and parsed back.
fn reparsed(result: &DataResult<Value>) -> Value {
	let text = serde_json::to_string(result.data.as_ref().unwrap()).unwrap();
	serde_json::from_str(&text).unwrap()
}

#[test]
fn test_awkward_keys_round_trip_under_the_ceiling() -> Result<()> {
	for key in AWKWARD_WRITE_KEYS {
		let mut store = store_with(300, key);
		let appended: Vec<Value> = (0..40).map(|i| json!({"i": i})).collect();
		for item in &appended {
			store.append(item.clone())?;
		}

		let mut seen = Vec::new();
		let mut batches = 0;
		while let Some(result) = store.fetch(None, None)? {
			seen.extend(checked_batch(&result, 300));
			assert_eq!(reparsed(&result)["writeKey"], key);
			store.remove(&result.removable.unwrap())?;
			batches += 1;
		}
		assert!(batches > 1, "key {key:?} should take several batches");
		assert_eq!(seen, appended, "key {key:?}");
	}
	Ok(())
}

#[test]
fn test_awkward_keys_exact_fit_lands_on_the_limit() {
	for key in AWKWARD_WRITE_KEYS {
		assert_eq!(envelope(key), expected_envelope(key), "key {key:?}");
		let mut store = store_with(4096, key);
		for tag in 0..3 {
			store.append(item_of_len(40, tag)).unwrap();
		}
		let exact = expected_envelope(key) + 3 * 40 + 2;
		let result = store
			.fetch_with_sent_at(None, Some(exact), SENT_AT)
			.unwrap();
		let text = serde_json::to_string(result.data.as_ref().unwrap()).unwrap();
		assert_eq!(text.len(), exact, "key {key:?}");
		assert_eq!(reparsed(&result)["writeKey"], key);
		assert_eq!(
			fetched_at_fixed_time(&mut store, exact - 1),
			2,
			"key {key:?}"
		);
	}
}

#[test]
fn test_awkward_keys_largest_item_fits_alone() -> Result<()> {
	for key in AWKWARD_WRITE_KEYS {
		let mut store = store_with(200, key);
		let largest = store.max_item_size;
		assert_eq!(largest, 200 - MemoryStore::widest_envelope_len(key));
		assert_invalid_input(&store.append(item_of_len(largest + 1, 0)).unwrap_err());
		store.append(item_of_len(largest, 1))?;
		assert_eq!(drain(&mut store, None, None)?.concat().len(), 1);
	}
	Ok(())
}
