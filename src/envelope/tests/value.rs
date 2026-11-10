//! The `Value` form, its length against the bytes, the timestamp and the builder.

use super::*;
use crate::envelope::samples::{is_timestamp, sample_envelopes, AWKWARD_STRINGS};

#[test]
fn test_value_form_has_the_configured_keys() {
	let envelope = Envelope::new("events")
		.field("source", "sensor-7")
		.field("n", json!([1, {"x": null}]))
		.timestamp("sentAt");
	let batch = envelope.batch(vec![json!(1), json!("b")], at());
	assert_eq!(
		batch,
		json!({
			"events": [1, "b"],
			"source": "sensor-7",
			"n": [1, {"x": null}],
			"sentAt": "2026-11-09T12:34:56.789Z"
		})
	);
	assert_eq!(
		Envelope::default().batch(vec![json!(1)], at()).to_string(),
		r#"{"items":[1]}"#
	);
}

#[test]
fn test_value_length_equals_opening_plus_closing() {
	let mut envelopes = sample_envelopes();
	// Keys that sort differently from the order given, so the Value's order differs from
	// the bytes'.
	for s in AWKWARD_STRINGS {
		envelopes.push(
			Envelope::new(format!("z{s}"))
				.field(format!("a{s}"), s)
				.field("0", json!({"z": s, "a": [s]}))
				.timestamp(format!("m{s}")),
		);
	}
	let items = [json!({"b": 1, "a": 2}), json!("x"), json!([null])];
	for envelope in envelopes {
		let empty = serde_json::to_string(&envelope.batch(Vec::new(), at())).unwrap();
		assert_eq!(empty.len(), envelope.byte_len(), "{envelope:?}: {empty}");
		assert_eq!(
			empty.len(),
			envelope.opening().len() + envelope.closing(at()).len()
		);
		for count in 1..=items.len() {
			let batch = envelope.batch(items[..count].to_vec(), at());
			let text = serde_json::to_string(&batch).unwrap();
			assert_eq!(text.len(), file_bytes(&envelope, &items[..count]).len());
			let parsed: Value =
				serde_json::from_str(&file_bytes(&envelope, &items[..count])).unwrap();
			assert_eq!(parsed, batch, "{envelope:?}");
		}
	}
}

#[test]
fn test_timestamp_is_24_bytes_in_the_documented_format() {
	let instants = [
		DateTime::UNIX_EPOCH,
		at(),
		DateTime::from_timestamp(1_762_691_696, 0).unwrap(),
		DateTime::from_timestamp(1_762_691_696, 5_000_000).unwrap(),
		DateTime::from_timestamp(1_762_691_696, 999_999_999).unwrap(),
		DateTime::from_timestamp(-62_167_219_200, 0).unwrap(),
		DateTime::from_timestamp(253_402_300_799, 999_999_999).unwrap(),
		Utc::now(),
	];
	for instant in instants {
		let text = timestamp(instant);
		assert_eq!(text.len(), 24, "{text}");
		assert!(is_timestamp(&json!(text)), "{text}");
		let parsed = chrono::NaiveDateTime::parse_from_str(&text, "%Y-%m-%dT%H:%M:%S.%3fZ")
			.unwrap()
			.and_utc();
		// Millisecond precision, truncated.
		assert_eq!(
			parsed.timestamp_millis(),
			instant.timestamp_millis(),
			"{text}"
		);
	}
	assert_eq!(timestamp(at()), "2026-11-09T12:34:56.789Z");
	assert_eq!(timestamp(DateTime::UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
}

#[test]
fn test_builder_matches_literal() {
	assert_eq!(
		Envelope::default(),
		Envelope {
			items_key: "items".into(),
			fields: Vec::new(),
			timestamp_key: None,
		}
	);
	assert_eq!(
		Envelope::new("events"),
		Envelope {
			items_key: "events".into(),
			fields: Vec::new(),
			timestamp_key: None,
		}
	);
	assert_eq!(
		Envelope::new("events")
			.field("source", "sensor-7")
			.field("n", 7)
			.field("tags", json!(["a"]))
			.field("none", Value::Null)
			.timestamp("sentAt"),
		Envelope {
			items_key: "events".into(),
			fields: vec![
				("source".into(), json!("sensor-7")),
				("n".into(), json!(7)),
				("tags".into(), json!(["a"])),
				("none".into(), Value::Null),
			],
			timestamp_key: Some("sentAt".into()),
		}
	);
	// The last timestamp key set wins.
	assert_eq!(
		Envelope::new("a")
			.timestamp("x")
			.timestamp("y")
			.timestamp_key,
		Some("y".into())
	);
}
