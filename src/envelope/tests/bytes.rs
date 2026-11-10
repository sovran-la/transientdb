//! The exact bytes of the opening and the closing.

use super::*;
use crate::envelope::samples::{
	every_json_type, expected_closing, expected_opening, sample_envelopes, SAMPLE_TIMESTAMP,
};

#[test]
fn test_default_is_items_and_nothing_else() {
	let envelope = Envelope::default();
	assert_eq!(opening(&envelope), r#"{"items":["#);
	assert_eq!(closing(&envelope), "]}");
	assert_eq!(envelope.byte_len(), 12);
	assert_eq!(
		file_bytes(&envelope, &[json!(1), json!({"a": "b"})]),
		r#"{"items":[1,{"a":"b"}]}"#
	);
	assert_eq!(file_bytes(&envelope, &[]), r#"{"items":[]}"#);
}

#[test]
fn test_renamed_items_key() {
	let envelope = Envelope::new("events");
	assert_eq!(opening(&envelope), r#"{"events":["#);
	assert_eq!(closing(&envelope), "]}");
	assert_eq!(file_bytes(&envelope, &[json!("x")]), r#"{"events":["x"]}"#);
}

#[test]
fn test_one_field() {
	let envelope = Envelope::new("items").field("source", "sensor-7");
	assert_eq!(opening(&envelope), r#"{"items":["#);
	assert_eq!(closing(&envelope), r#"],"source":"sensor-7"}"#);
}

#[test]
fn test_several_fields_keep_their_order() {
	let envelope = Envelope::new("items")
		.field("zeta", 1)
		.field("alpha", 2)
		.field("mid", 3);
	assert_eq!(closing(&envelope), r#"],"zeta":1,"alpha":2,"mid":3}"#);
	let reversed = Envelope::new("items")
		.field("mid", 3)
		.field("alpha", 2)
		.field("zeta", 1);
	assert_eq!(closing(&reversed), r#"],"mid":3,"alpha":2,"zeta":1}"#);
}

#[test]
fn test_timestamp_only() {
	let envelope = Envelope::new("items").timestamp("sentAt");
	assert_eq!(opening(&envelope), r#"{"items":["#);
	assert_eq!(
		closing(&envelope),
		r#"],"sentAt":"2026-11-09T12:34:56.789Z"}"#
	);
}

#[test]
fn test_fields_then_timestamp() {
	let envelope = Envelope::new("events")
		.field("source", "sensor-7")
		.field("v", 2)
		.timestamp("sentAt");
	assert_eq!(opening(&envelope), r#"{"events":["#);
	assert_eq!(
		closing(&envelope),
		r#"],"source":"sensor-7","v":2,"sentAt":"2026-11-09T12:34:56.789Z"}"#
	);
	assert_eq!(opening(&envelope).len(), 11);
	assert_eq!(closing(&envelope).len(), 64);
	assert_eq!(
		file_bytes(&envelope, &[json!({"t": 1}), json!(2)]),
		r#"{"events":[{"t":1},2],"source":"sensor-7","v":2,"sentAt":"2026-11-09T12:34:56.789Z"}"#
	);
}

#[test]
fn test_keys_are_json_escaped() {
	let envelope = Envelope::new("quote\"back\\slash")
		.field("new\nline", "v")
		.field("ключ-🦀", "значение")
		.timestamp("tab\tkey");
	assert_eq!(opening(&envelope), r#"{"quote\"back\\slash":["#);
	assert_eq!(
		closing(&envelope),
		r#"],"new\nline":"v","ключ-🦀":"значение","tab\tkey":"2026-11-09T12:34:56.789Z"}"#
	);
	let control = Envelope::new("c\u{1}");
	assert_eq!(opening(&control), r#"{"c\u0001":["#);
}

#[test]
fn test_values_of_every_json_type_are_written_compactly() {
	let envelope = Envelope {
		items_key: "items".into(),
		fields: every_json_type(),
		timestamp_key: None,
	};
	assert_eq!(
		closing(&envelope),
		concat!(
			r#"],"null":null,"true":true,"false":false,"int":-42,"#,
			r#""uint":18446744073709551615,"float":1.5e-7,"#,
			r#""string":"say \"hi\" \\ bye\n","empty":"","#,
			r#""array":[1,"two",[3,[4]],{"five":5},null],"#,
			r#""object":{"a":{"b":[true,{"c":"d"}]},"e":{}}}"#
		)
	);
}

#[test]
fn test_bytes_match_the_documented_layout_for_every_sample() {
	for envelope in sample_envelopes() {
		assert_eq!(
			opening(&envelope),
			expected_opening(&envelope),
			"{envelope:?}"
		);
		assert_eq!(
			closing(&envelope),
			expected_closing(&envelope, SAMPLE_TIMESTAMP),
			"{envelope:?}"
		);
		assert!(!opening(&envelope).contains(": "), "{envelope:?}");
	}
}

#[test]
fn test_file_parses_to_exactly_the_expected_value() {
	let items = [json!({"a": [1, 2]}), json!("two"), json!(null), json!(3.5)];
	for envelope in sample_envelopes() {
		for count in [1, 2, items.len()] {
			let parsed: Value = serde_json::from_str(&file_bytes(&envelope, &items[..count]))
				.unwrap_or_else(|e| panic!("{envelope:?}: {e}"));
			let mut expected = serde_json::Map::new();
			expected.insert(envelope.items_key.clone(), json!(items[..count]));
			for (key, value) in &envelope.fields {
				expected.insert(key.clone(), value.clone());
			}
			if let Some(key) = &envelope.timestamp_key {
				expected.insert(key.clone(), json!(SAMPLE_TIMESTAMP));
			}
			assert_eq!(parsed, Value::Object(expected), "{envelope:?}");
		}
	}
}

#[test]
fn test_closing_length_does_not_depend_on_the_time() {
	let instants = [
		DateTime::UNIX_EPOCH,
		at(),
		DateTime::from_timestamp(1_762_691_696, 0).unwrap(),
		DateTime::from_timestamp(1_762_691_696, 123_456_789).unwrap(),
		DateTime::from_timestamp(253_402_300_799, 999_999_999).unwrap(),
		Utc::now(),
	];
	for envelope in sample_envelopes() {
		let len = envelope.closing(at()).len();
		for instant in instants {
			assert_eq!(
				envelope.closing(instant).len(),
				len,
				"{envelope:?} {instant}"
			);
		}
		assert_eq!(envelope.byte_len(), envelope.opening().len() + len);
	}
}
