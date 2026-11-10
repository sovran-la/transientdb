//! Finished files are byte for byte the configured envelope around the items, and the
//! byte ceiling, the oversized-item rejection and the validator follow it.

use super::support::*;
use super::*;
use crate::envelope::samples::{expected_len, is_timestamp, sample_envelopes};
use std::sync::{Arc, Mutex};

/// The bytes of the one finished file after appending `items` under `envelope`.
fn only_file(envelope: Envelope, items: &[Value]) -> Result<String> {
	let temp_dir = TempDir::new()?;
	let mut store = DirectoryStore::new(config(&temp_dir, 4096, envelope))?;
	for item in items {
		store.append(item.clone())?;
	}
	let files = store.fetch(None, None)?.unwrap().data.unwrap();
	assert_eq!(files.len(), 1);
	fs::read_to_string(&files[0])
}

#[test]
fn test_default_envelope_file_is_exactly_items() -> Result<()> {
	let items = [json!({"event": "a"}), json!([1, 2]), json!("three")];
	assert_eq!(
		only_file(Envelope::default(), &items)?,
		r#"{"items":[{"event":"a"},[1,2],"three"]}"#
	);
	Ok(())
}

#[test]
fn test_renamed_items_key_file_is_exactly_events() -> Result<()> {
	let items = [json!({"event": "a"}), json!(2)];
	assert_eq!(
		only_file(Envelope::new("events"), &items)?,
		r#"{"events":[{"event":"a"},2]}"#
	);
	Ok(())
}

#[test]
fn test_fields_and_timestamp_follow_the_documented_layout() -> Result<()> {
	let envelope = Envelope::new("events")
		.field("source", "sensor-7")
		.field("meta", json!({"v": [1, true, null]}))
		.timestamp("sentAt");
	let content = only_file(envelope, &[json!({"t": 1}), json!("x")])?;
	let prefix =
		r#"{"events":[{"t":1},"x"],"source":"sensor-7","meta":{"v":[1,true,null]},"sentAt":""#;
	assert!(content.starts_with(prefix), "{content}");
	assert!(content.ends_with("\"}"), "{content}");
	let timestamp = &content[prefix.len()..content.len() - 2];
	assert!(is_timestamp(&json!(timestamp)), "{content}");
	assert_eq!(content.len(), prefix.len() + 24 + 2);
	Ok(())
}

/// Envelopes of very different sizes for the ceiling tests: the default, long field
/// values, multi-byte keys, and the timestamp on and off.
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
			let temp_dir = TempDir::new()?;
			let mut store = DirectoryStore::new(config(&temp_dir, max, envelope.clone()))?;
			let largest = store.max_item_size();
			let appended: Vec<Value> = (0..120)
				.map(|i| item_of_len(2 + (i * 37) % (largest - 1)))
				.collect();
			for item in &appended {
				store.append(item.clone())?;
			}
			let files = finished_files(&mut store)?;
			assert!(files.len() > 1, "{envelope:?} max={max}");
			assert_eq!(
				assert_ceiling(&files, max),
				appended,
				"{envelope:?} max={max}"
			);
		}
	}
	Ok(())
}

#[test]
fn test_exact_fit_and_one_byte_more_under_every_envelope() -> Result<()> {
	for envelope in ceiling_envelopes() {
		let max = expected_len(&envelope) + 200;
		// 60 + 1 + 139 = 200 bytes of items and separator: an exact fit.
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(config(&temp_dir, max, envelope.clone()))?;
		store.append(item_of_len(60))?;
		store.append(item_of_len(139))?;
		let files = finished_files(&mut store)?;
		assert_eq!(batch_counts(&files), vec![2], "{envelope:?}");
		assert_eq!(files[0].len, max as u64, "{envelope:?}");

		// One byte more rotates.
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(config(&temp_dir, max, envelope.clone()))?;
		store.append(item_of_len(60))?;
		store.append(item_of_len(140))?;
		let files = finished_files(&mut store)?;
		assert_eq!(batch_counts(&files), vec![1, 1], "{envelope:?}");
		assert_ceiling(&files, max);
	}
	Ok(())
}

#[test]
fn test_oversized_boundary_moves_with_the_envelope() -> Result<()> {
	let max = 400;
	let mut largest_seen = Vec::new();
	for envelope in ceiling_envelopes() {
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(config(&temp_dir, max, envelope.clone()))?;
		let largest = max - expected_len(&envelope);
		assert_eq!(store.max_item_size(), largest, "{envelope:?}");
		assert_invalid_input(&store.append(item_of_len(largest + 1)).unwrap_err());
		assert!(files_on_disk(&temp_dir)?.is_empty());
		store.append(item_of_len(largest))?;
		let files = finished_files(&mut store)?;
		assert_eq!(files[0].len, max as u64, "{envelope:?}");
		largest_seen.push(largest);
	}
	// The envelopes really differ in size, so the boundary really moved.
	largest_seen.sort();
	largest_seen.dedup();
	assert_eq!(largest_seen.len(), ceiling_envelopes().len());
	Ok(())
}

#[test]
fn test_validator_sees_the_final_bytes_of_the_configured_envelope() -> Result<()> {
	let envelope = Envelope::new("events")
		.field("source", "sensor-7")
		.timestamp("sentAt");
	let temp_dir = TempDir::new()?;
	let mut config = config(&temp_dir, 4096, envelope.clone());
	config.max_file_items = Some(2);
	let mut store = DirectoryStore::new(config)?;
	let seen = Arc::new(Mutex::new(Vec::new()));
	let log = seen.clone();
	store.set_file_validator(move |path| {
		log.lock().unwrap().push(fs::read(path)?);
		Ok(())
	});
	for i in 0..3 {
		store.append(json!(i))?;
	}
	// A file finished by rotation and one finished by fetch.
	let files = store.fetch(None, None)?.unwrap().data.unwrap();
	let delivered: Vec<Vec<u8>> = files.iter().map(fs::read).collect::<Result<_>>()?;
	let seen = seen.lock().unwrap().clone();
	assert_eq!(seen, delivered);
	for (bytes, items) in seen.iter().zip([json!([0, 1]), json!([2])]) {
		let text = String::from_utf8(bytes.clone()).unwrap();
		let start = format!("{{\"events\":{items}");
		assert!(text.starts_with(&start), "{text}");
		assert!(
			text.contains(r#"],"source":"sensor-7","sentAt":""#),
			"{text}"
		);
		let json: Value = serde_json::from_str(&text)?;
		assert!(is_timestamp(&json["sentAt"]), "{text}");
	}
	Ok(())
}

#[test]
fn test_every_sample_envelope_round_trips_through_a_file() -> Result<()> {
	for envelope in sample_envelopes() {
		let items = [json!({"a": 1}), json!("b")];
		let content = only_file(envelope.clone(), &items)?;
		assert_eq!(
			content.len(),
			expected_len(&envelope) + item_len(&items[0]) + 1 + 3
		);
		let parsed: Value = serde_json::from_str(&content)?;
		assert_eq!(parsed[&envelope.items_key], json!(items), "{envelope:?}");
		for (key, value) in &envelope.fields {
			assert_eq!(&parsed[key], value, "{envelope:?}");
		}
	}
	Ok(())
}

#[test]
#[should_panic(expected = "Envelope items_key is empty? The items need somewhere to live.")]
fn test_new_rejects_an_invalid_envelope() {
	let temp_dir = TempDir::new().unwrap();
	let _ = DirectoryStore::new(config(&temp_dir, 1024, Envelope::new("")));
}

#[test]
fn test_invalid_envelope_creates_no_directory() {
	let temp_dir = TempDir::new().unwrap();
	let location = temp_dir.path().join("never-created");
	for envelope in [
		Envelope::new(""),
		Envelope::new("items").field("", 1),
		Envelope::new("items").timestamp(""),
		Envelope::new("items").field("items", 1),
		Envelope::new("items").field(
			"deep",
			crate::batch::nested(200, crate::batch::Shape::Arrays, json!(0)),
		),
	] {
		let mut config = config(&temp_dir, 1024, envelope);
		config.storage_location = location.clone();
		let result = std::panic::catch_unwind(|| DirectoryStore::new(config));
		assert!(result.is_err());
		assert!(!location.exists());
	}
}
