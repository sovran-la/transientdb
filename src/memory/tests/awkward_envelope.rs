//! Every envelope, awkward keys and values included, round-trips through a fetched batch,
//! and the byte ceiling counts its escaped bytes.

use super::support::*;
use super::*;
use crate::envelope::samples::{expected_len, items_of, sample_envelopes};
use crate::DataResult;

/// The fetched batch, serialized and parsed back.
fn reparsed(result: &DataResult<Value>) -> Value {
	let text = serde_json::to_string(result.data.as_ref().unwrap()).unwrap();
	serde_json::from_str(&text).unwrap()
}

#[test]
fn test_awkward_envelopes_round_trip_under_the_ceiling() -> Result<()> {
	for envelope in sample_envelopes() {
		let max = (expected_len(&envelope) + 120).max(100);
		let mut store = store_with(max, envelope.clone());
		let appended: Vec<Value> = (0..40).map(|i| json!({"i": i})).collect();
		for item in &appended {
			store.append(item.clone())?;
		}

		let mut seen = Vec::new();
		let mut batches = 0;
		while let Some(result) = store.fetch(None, None)? {
			let items = checked_batch_with(&result, max, &envelope);
			// What a consumer parses back is still exactly the configured shape.
			assert_eq!(items_of(&reparsed(&result), &envelope), items);
			seen.extend(items);
			store.remove(&result.removable.unwrap())?;
			batches += 1;
		}
		assert!(batches > 1, "{envelope:?} should take several batches");
		assert_eq!(seen, appended, "{envelope:?}");
	}
	Ok(())
}

#[test]
fn test_awkward_envelopes_exact_fit_lands_on_the_limit() {
	for envelope in sample_envelopes() {
		let mut store = store_with(4096, envelope.clone());
		assert_eq!(store.envelope_len, expected_len(&envelope), "{envelope:?}");
		for tag in 0..3 {
			store.append(item_of_len(40, tag)).unwrap();
		}
		let exact = expected_len(&envelope) + 3 * 40 + 2;
		let result = store.fetch(None, Some(exact)).unwrap().unwrap();
		let text = serde_json::to_string(result.data.as_ref().unwrap()).unwrap();
		assert_eq!(text.len(), exact, "{envelope:?}");
		assert_eq!(items_of(&reparsed(&result), &envelope).len(), 3);
		assert_eq!(fetched(&mut store, exact), 3, "{envelope:?}");
		assert_eq!(fetched(&mut store, exact - 1), 2, "{envelope:?}");
	}
}

#[test]
fn test_awkward_envelopes_largest_item_fits_alone() -> Result<()> {
	for envelope in sample_envelopes() {
		let max = expected_len(&envelope) + 150;
		let mut store = store_with(max, envelope.clone());
		let largest = store.max_item_size;
		assert_eq!(largest, 150, "{envelope:?}");
		assert_invalid_input(&store.append(item_of_len(largest + 1, 0)).unwrap_err());
		store.append(item_of_len(largest, 1))?;
		assert_eq!(drain(&mut store, None, None)?.concat().len(), 1);
	}
	Ok(())
}
