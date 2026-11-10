//! Envelopes for tests of every store, and the documented layout built independently of
//! [`Envelope::opening`] and [`Envelope::closing`], to check them against.

use super::Envelope;
use serde_json::{json, Value};

/// Strings that need escaping in JSON, or are multi-byte in UTF-8.
pub(crate) const AWKWARD_STRINGS: [&str; 6] = [
	"quote\"inside",
	"back\\slash",
	"new\nline",
	"tab\there",
	"ключ-🦀-鍵",
	"control\u{1}char",
];

/// A timestamp in the documented format, for building expected bytes.
pub(crate) const SAMPLE_TIMESTAMP: &str = "2026-11-09T12:34:56.789Z";

/// A value of every JSON type, nested ones included.
pub(crate) fn every_json_type() -> Vec<(String, Value)> {
	vec![
		("null".into(), Value::Null),
		("true".into(), json!(true)),
		("false".into(), json!(false)),
		("int".into(), json!(-42)),
		("uint".into(), json!(u64::MAX)),
		("float".into(), json!(1.5e-7)),
		("string".into(), json!("say \"hi\" \\ bye\n")),
		("empty".into(), json!("")),
		(
			"array".into(),
			json!([1, "two", [3, [4]], {"five": 5}, null]),
		),
		(
			"object".into(),
			json!({"a": {"b": [true, {"c": "d"}]}, "e": {}}),
		),
	]
}

/// Envelopes of every kind the stores must handle: the default, a renamed items key,
/// long field values, multi-byte and escaped keys and values, a timestamp on and off,
/// and values of every JSON type.
pub(crate) fn sample_envelopes() -> Vec<Envelope> {
	let mut envelopes = vec![
		Envelope::default(),
		Envelope::new("events"),
		Envelope::new("events").timestamp("sentAt"),
		Envelope::new("events")
			.field("source", "sensor-7")
			.timestamp("sentAt"),
		Envelope::new("items")
			.field("note", "n".repeat(120))
			.field("tags", json!(["a".repeat(30), "b".repeat(30)])),
		Envelope::new("項目🦀")
			.field("ключ", "значение")
			.timestamp("時刻"),
		Envelope {
			items_key: "items".into(),
			fields: every_json_type(),
			timestamp_key: Some("at".into()),
		},
	];
	for (i, s) in AWKWARD_STRINGS.into_iter().enumerate() {
		envelopes.push(Envelope::new(s));
		envelopes.push(Envelope::new("items").field(s, i).field("v", s));
		envelopes.push(Envelope::new("items").field("v", s).timestamp(s));
	}
	envelopes
}

/// The documented opening, built from its description: `{`, the items key as a JSON
/// string, `:[`.
pub(crate) fn expected_opening(envelope: &Envelope) -> String {
	format!(
		"{{{}:[",
		serde_json::to_string(&envelope.items_key).unwrap()
	)
}

/// The documented closing for a batch finished at `timestamp`: `]`, each field as
/// `,"key":value`, the timestamp as `,"key":"timestamp"` if configured, `}`.
pub(crate) fn expected_closing(envelope: &Envelope, timestamp: &str) -> String {
	let mut closing = String::from("]");
	for (key, value) in &envelope.fields {
		closing += &format!(",{}:{}", serde_json::to_string(key).unwrap(), value);
	}
	if let Some(key) = &envelope.timestamp_key {
		closing += &format!(",{}:\"{timestamp}\"", serde_json::to_string(key).unwrap());
	}
	closing + "}"
}

/// Length of the documented envelope with no items.
pub(crate) fn expected_len(envelope: &Envelope) -> usize {
	expected_opening(envelope).len() + expected_closing(envelope, SAMPLE_TIMESTAMP).len()
}

/// Whether `value` is a timestamp in the documented format: 24 bytes,
/// `%Y-%m-%dT%H:%M:%S.%3fZ`.
pub(crate) fn is_timestamp(value: &Value) -> bool {
	value.as_str().is_some_and(|s| {
		s.len() == 24 && chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S.%3fZ").is_ok()
	})
}

/// Asserts that `batch` is exactly what `envelope` describes, and returns its items: an
/// object with the items array under the items key, every field with its configured
/// value, a timestamp under the timestamp key if one is configured, and no other key.
pub(crate) fn items_of(batch: &Value, envelope: &Envelope) -> Vec<Value> {
	let object = batch.as_object().expect("a batch is an object");
	for (key, value) in &envelope.fields {
		assert_eq!(object.get(key), Some(value), "field {key:?} in {batch}");
	}
	if let Some(key) = &envelope.timestamp_key {
		assert!(
			object.get(key).is_some_and(is_timestamp),
			"timestamp {key:?} in {batch}"
		);
	}
	let expected_keys = 1 + envelope.fields.len() + usize::from(envelope.timestamp_key.is_some());
	assert_eq!(object.len(), expected_keys, "unexpected keys in {batch}");
	object
		.get(&envelope.items_key)
		.and_then(Value::as_array)
		.unwrap_or_else(|| panic!("no items array {:?} in {batch}", envelope.items_key))
		.clone()
}
