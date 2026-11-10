//! A custom envelope end to end through TransientDB, for the native stores, and the same
//! batch from `DirectoryStore` and `MemoryStore` for the same envelope and items.

#![cfg(not(target_arch = "wasm32"))]

use serde_json::{json, Value};
use std::fs;
use std::io::Result;
use std::path::PathBuf;
use tempfile::TempDir;
use transientdb::{
	DirectoryConfig, DirectoryStore, Envelope, MemoryConfig, MemoryStore, TransientDB,
};

/// A custom envelope with a renamed items key, fields of several types and a timestamp.
fn envelope() -> Envelope {
	Envelope::new("events")
		.field("source", "sensor-7")
		.field("schema", 3)
		.field("tags", json!(["a", {"b": null}]))
		.timestamp("sentAt")
}

fn directory_db(dir: &TempDir, envelope: Envelope) -> Result<TransientDB<Vec<PathBuf>>> {
	Ok(TransientDB::new(DirectoryStore::new(DirectoryConfig {
		envelope,
		storage_location: dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size: 4096,
		max_file_items: None,
	})?))
}

fn memory_db(envelope: Envelope) -> TransientDB<Value> {
	TransientDB::new(MemoryStore::new(MemoryConfig {
		envelope,
		max_items: None,
		max_fetch_size: 4096,
		max_fetch_items: None,
	}))
}

/// Whether `value` is a timestamp in the documented format.
fn is_timestamp(value: &Value) -> bool {
	value.as_str().is_some_and(|s| {
		s.len() == 24 && chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S.%3fZ").is_ok()
	})
}

/// `batch` with its `sentAt` replaced, after checking it is a timestamp, so two batches
/// finished at different times compare equal.
fn without_timestamp(mut batch: Value) -> Value {
	assert!(is_timestamp(&batch["sentAt"]), "{batch}");
	batch["sentAt"] = json!("<timestamp>");
	batch
}

fn items() -> Vec<Value> {
	vec![
		json!({"event": "a", "n": 1}),
		json!("two"),
		json!([3, {"x": "}"}]),
	]
}

fn expected(items: &[Value]) -> Value {
	json!({
		"events": items,
		"source": "sensor-7",
		"schema": 3,
		"tags": ["a", {"b": null}],
		"sentAt": "<timestamp>",
	})
}

#[test]
fn test_directory_store_custom_envelope_end_to_end() -> Result<()> {
	let dir = TempDir::new()?;
	let db = directory_db(&dir, envelope())?;
	for item in items() {
		db.append(item)?;
	}
	let result = db.fetch(None, None)?.unwrap();
	let files = result.data.unwrap();
	assert_eq!(files.len(), 1);
	let content = fs::read_to_string(&files[0])?;
	let prefix = concat!(
		r#"{"events":[{"event":"a","n":1},"two",[3,{"x":"}"}]],"#,
		r#""source":"sensor-7","schema":3,"tags":["a",{"b":null}],"sentAt":""#
	);
	assert!(content.starts_with(prefix), "{content}");
	assert_eq!(content.len(), prefix.len() + 24 + 2);
	assert!(content.ends_with("\"}"));
	let parsed: Value = serde_json::from_str(&content)?;
	assert_eq!(without_timestamp(parsed), expected(&items()));
	db.remove(&result.removable.unwrap())?;
	assert!(!db.has_data());
	Ok(())
}

#[test]
fn test_memory_store_custom_envelope_end_to_end() -> Result<()> {
	let db = memory_db(envelope());
	for item in items() {
		db.append(item)?;
	}
	let result = db.fetch(None, None)?.unwrap();
	let batch = result.data.unwrap();
	assert_eq!(without_timestamp(batch), expected(&items()));
	db.remove(&result.removable.unwrap())?;
	assert!(!db.has_data());
	Ok(())
}

#[test]
fn test_directory_file_equals_memory_batch_for_the_same_envelope() -> Result<()> {
	let envelopes = [
		Envelope::default(),
		Envelope::new("events"),
		envelope(),
		Envelope::new("項目🦀")
			.field("quote\"key", "back\\slash\nvalue")
			.field("obj", json!({"z": 1, "a": [true, 1.5]}))
			.timestamp("sentAt"),
	];
	for envelope in envelopes {
		let dir = TempDir::new()?;
		let directory = directory_db(&dir, envelope.clone())?;
		let memory = memory_db(envelope.clone());
		for item in items() {
			directory.append(item.clone())?;
			memory.append(item)?;
		}
		let files = directory.fetch(None, None)?.unwrap().data.unwrap();
		assert_eq!(files.len(), 1);
		let file: Value = serde_json::from_str(&fs::read_to_string(&files[0])?)?;
		let batch = memory.fetch(None, None)?.unwrap().data.unwrap();

		let normalize = |value: Value| match &envelope.timestamp_key {
			Some(key) => {
				let mut value = value;
				assert!(is_timestamp(&value[key]), "{envelope:?}: {value}");
				value[key] = json!("<timestamp>");
				value
			}
			None => value,
		};
		assert_eq!(normalize(file), normalize(batch), "{envelope:?}");
	}
	Ok(())
}
