//! A fetched batch is exactly the configured envelope around the items, and the byte
//! ceiling and the oversized-item rejection follow its size.

use super::support::*;
use super::*;
use crate::envelope::samples::{expected_len, is_timestamp, sample_envelopes};

/// The one batch fetched after appending `items` under `envelope`.
fn only_batch(envelope: Envelope, items: &[Value]) -> Result<Value> {
	let mut store = store_with(4096, envelope);
	for item in items {
		store.append(item.clone())?;
	}
	let result = store.fetch(None, None)?.unwrap();
	assert_eq!(result.removable.as_ref().unwrap().len(), items.len());
	Ok(result.data.unwrap())
}

#[test]
fn test_batch_has_exactly_the_configured_keys() -> Result<()> {
	let items = [json!({"a": 1}), json!("b"), json!([3])];
	for envelope in sample_envelopes() {
		let batch = only_batch(envelope.clone(), &items)?;
		let object = batch.as_object().unwrap();
		let mut keys: Vec<&String> = object.keys().collect();
		keys.sort();
		let mut expected: Vec<&String> = std::iter::once(&envelope.items_key)
			.chain(envelope.fields.iter().map(|(key, _)| key))
			.chain(envelope.timestamp_key.as_ref())
			.collect();
		expected.sort();
		assert_eq!(keys, expected, "{envelope:?}");
		// Items in order under the items key; fields as configured.
		assert_eq!(object[&envelope.items_key], json!(items), "{envelope:?}");
		for (key, value) in &envelope.fields {
			assert_eq!(&object[key], value, "{envelope:?}");
		}
		// The timestamp is present only if configured, in the documented format.
		if let Some(key) = &envelope.timestamp_key {
			assert!(is_timestamp(&object[key]), "{envelope:?}: {batch}");
			assert_eq!(object[key].as_str().unwrap().len(), 24);
		}
	}
	Ok(())
}

#[test]
fn test_default_envelope_serializes_to_exactly_items() -> Result<()> {
	let batch = only_batch(Envelope::default(), &[json!({"event": "a"}), json!(2)])?;
	assert_eq!(
		serde_json::to_string(&batch).unwrap(),
		r#"{"items":[{"event":"a"},2]}"#
	);
	Ok(())
}

#[test]
fn test_no_timestamp_unless_configured() -> Result<()> {
	let batch = only_batch(
		Envelope::new("events").field("source", "sensor-7"),
		&[json!(1)],
	)?;
	assert_eq!(batch, json!({"events": [1], "source": "sensor-7"}));
	Ok(())
}

/// Envelopes of very different sizes for the ceiling tests.
fn ceiling_envelopes() -> Vec<Envelope> {
	vec![
		Envelope::default(),
		Envelope::new("events").timestamp("sentAt"),
		Envelope::new("events").field("note", "n".repeat(150)),
		Envelope::new("events")
			.field("note", "n".repeat(150))
			.timestamp("sentAt"),
		Envelope::new("項目🦀")
			.field("ключ-🦀", "значение-鍵")
			.timestamp("時刻"),
		Envelope::new("項目🦀").field("ключ-🦀", "значение-鍵"),
	]
}

#[test]
fn test_ceiling_holds_under_every_envelope() -> Result<()> {
	for envelope in ceiling_envelopes() {
		for room in [40usize, 100, 333] {
			let max = (expected_len(&envelope) + room).max(100);
			let mut store = store_with(max, envelope.clone());
			let largest = store.max_item_size;
			let appended: Vec<Value> = (0..120)
				.map(|i| item_of_len(5 + (i * 37) % (largest - 4), i))
				.collect();
			for item in &appended {
				store.append(item.clone())?;
			}
			// drain checks every batch's length and shape.
			let batches = drain(&mut store, None, None)?;
			assert!(batches.len() > 1, "{envelope:?} max={max}");
			assert_eq!(batches.concat(), appended, "{envelope:?} max={max}");
		}
	}
	Ok(())
}

#[test]
fn test_exact_fit_under_every_envelope() {
	for envelope in ceiling_envelopes() {
		let mut store = store_with(4096, envelope.clone());
		for tag in 0..3 {
			store.append(item_of_len(40, tag)).unwrap();
		}
		let exact = expected_len(&envelope) + 3 * 40 + 2;
		let result = store.fetch(None, Some(exact)).unwrap().unwrap();
		let text = serde_json::to_string(result.data.as_ref().unwrap()).unwrap();
		assert_eq!(text.len(), exact, "{envelope:?}");
		assert_eq!(fetched(&mut store, exact - 1), 2, "{envelope:?}");
	}
}

#[test]
fn test_oversized_boundary_moves_with_the_envelope() -> Result<()> {
	let max = 400;
	let mut largest_seen = Vec::new();
	for envelope in ceiling_envelopes() {
		let mut store = store_with(max, envelope.clone());
		let largest = max - expected_len(&envelope);
		assert_eq!(store.max_item_size, largest, "{envelope:?}");
		assert_invalid_input(&store.append(item_of_len(largest + 1, 0)).unwrap_err());
		assert!(!store.has_data());
		store.append(item_of_len(largest, 1))?;
		let result = store.fetch(None, None)?.unwrap();
		let text = serde_json::to_string(result.data.as_ref().unwrap()).unwrap();
		assert_eq!(text.len(), max, "{envelope:?}");
		largest_seen.push(largest);
	}
	largest_seen.sort();
	largest_seen.dedup();
	assert_eq!(largest_seen.len(), ceiling_envelopes().len());
	Ok(())
}

#[test]
#[should_panic(
	expected = "Envelope key \"items\" is used twice? A JSON object only keeps one of them."
)]
fn test_new_rejects_an_invalid_envelope() {
	let _ = store_with(1024, Envelope::new("items").field("items", 1));
}
