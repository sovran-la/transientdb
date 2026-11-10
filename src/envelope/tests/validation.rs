//! `validate` panics, with its message, for each kind of unusable envelope, and accepts
//! awkward but valid ones.

use super::*;
use crate::batch::{nested, Shape};
use crate::envelope::samples::{every_json_type, sample_envelopes, AWKWARD_STRINGS};

/// The panic message of `validate`, or `None` if it accepts the envelope.
fn rejection(envelope: &Envelope) -> Option<String> {
	let result = std::panic::catch_unwind(|| envelope.validate());
	result.err().map(|payload| {
		payload
			.downcast_ref::<String>()
			.cloned()
			.or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
			.unwrap()
	})
}

#[test]
#[should_panic(expected = "Envelope items_key is empty? The items need somewhere to live.")]
fn test_rejects_empty_items_key() {
	Envelope::new("").validate();
}

#[test]
#[should_panic(
	expected = "An envelope field with an empty key? `\"\":` is legal JSON and a terrible idea."
)]
fn test_rejects_empty_field_key() {
	Envelope::new("items")
		.field("ok", 1)
		.field("", 2)
		.validate();
}

#[test]
#[should_panic(expected = "Envelope timestamp_key is empty? Name it, or leave it None.")]
fn test_rejects_empty_timestamp_key() {
	Envelope::new("items").timestamp("").validate();
}

#[test]
#[should_panic(
	expected = "Envelope key \"items\" is used twice? A JSON object only keeps one of them."
)]
fn test_rejects_field_named_like_the_items() {
	Envelope::new("items").field("items", 1).validate();
}

#[test]
#[should_panic(
	expected = "Envelope key \"source\" is used twice? A JSON object only keeps one of them."
)]
fn test_rejects_two_fields_with_one_key() {
	Envelope::new("items")
		.field("source", 1)
		.field("source", 2)
		.validate();
}

#[test]
#[should_panic(
	expected = "Envelope key \"sentAt\" is used twice? A JSON object only keeps one of them."
)]
fn test_rejects_timestamp_named_like_a_field() {
	Envelope::new("items")
		.field("sentAt", "x")
		.timestamp("sentAt")
		.validate();
}

#[test]
#[should_panic(
	expected = "Envelope key \"items\" is used twice? A JSON object only keeps one of them."
)]
fn test_rejects_timestamp_named_like_the_items() {
	Envelope::new("items").timestamp("items").validate();
}

#[test]
#[should_panic(
	expected = "This envelope doesn't read back as JSON? Then nobody can read the batches either."
)]
fn test_rejects_field_too_deep_to_read_back() {
	Envelope::new("items")
		.field("deep", nested(200, Shape::Arrays, json!(0)))
		.validate();
}

#[test]
fn test_read_back_limit_is_serde_jsons() {
	// serde_json reads at most 127 levels and the batch object is one of them, so a field
	// value may be 126 levels deep and no more.
	for shape in [Shape::Arrays, Shape::Objects, Shape::Mixed] {
		let ok = Envelope::new("items").field("deep", nested(126, shape, json!(0)));
		assert_eq!(rejection(&ok), None, "{shape:?}");
		let deep = Envelope::new("items").field("deep", nested(127, shape, json!(0)));
		assert!(rejection(&deep).is_some(), "{shape:?}");
	}
}

#[test]
fn test_keys_are_compared_exactly() {
	// Same text in another case, with spaces, or escaped differently: distinct keys.
	let envelope = Envelope::new("items")
		.field("Items", 1)
		.field("items ", 2)
		.field(" items", 3)
		.timestamp("ITEMS");
	assert_eq!(rejection(&envelope), None);
}

#[test]
fn test_awkward_but_valid_envelopes_are_accepted() {
	for envelope in sample_envelopes() {
		assert_eq!(rejection(&envelope), None, "{envelope:?}");
	}
	let odd = Envelope {
		items_key: "\u{0}".into(),
		fields: every_json_type()
			.into_iter()
			.chain(AWKWARD_STRINGS.map(|s| (format!("k{s}"), json!({s: [s]}))))
			.chain([(" ".to_string(), json!({"": ""}))])
			.collect(),
		timestamp_key: Some("🕰".into()),
	};
	assert_eq!(rejection(&odd), None);
}
