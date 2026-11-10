//! Items for tests of every store that must come back exactly as they were appended:
//! numbers a JavaScript number cannot hold, floats with no fractional part, and every
//! other JSON type.

use serde_json::{json, Value};

/// Items that a store which loses number precision, or drops a float's `.0`, would give
/// back changed. A store must return each equal to the original and serializing to the
/// same bytes.
pub(crate) fn awkward_items() -> Vec<Value> {
	let above_safe = (1u64 << 53) + 1;
	let below_safe = -(1i64 << 53) - 1;
	vec![
		json!(u64::MAX),
		json!(i64::MIN),
		json!(above_safe),
		json!(below_safe),
		json!(42.0),
		json!(-0.0),
		json!(1e300),
		json!(5e-324),
		json!(0.1),
		json!(-1.2345678901234567e250),
		json!(f64::MAX),
		json!([u64::MAX, 42.0, -0.0, above_safe, [below_safe, 1.0]]),
		json!({
			"id": u64::MAX,
			"ts": 1_762_790_400_123_456_789u64,
			"price": 42.0,
			"nested": {"neg": i64::MIN, "list": [0.1, 5e-324, 1e300, -2.0]},
		}),
		json!(null),
		json!(true),
		json!(false),
		json!(0),
		json!("a string \"with\" escapes\n🦀"),
		json!(""),
		json!([]),
		json!({}),
	]
}

/// Asserts that `actual` holds exactly `expected`: equal as values, and serializing to
/// the same text, so `42.0` has not become `42`.
pub(crate) fn assert_exact(actual: &[Value], expected: &[Value]) {
	assert_eq!(actual, expected);
	for (actual, expected) in actual.iter().zip(expected) {
		assert_eq!(
			serde_json::to_string(actual).unwrap(),
			serde_json::to_string(expected).unwrap()
		);
	}
}

#[test]
fn samples_survive_serde_json_text() {
	let items = awkward_items();
	let reparsed: Vec<Value> = items
		.iter()
		.map(|item| serde_json::from_str(&serde_json::to_string(item).unwrap()).unwrap())
		.collect();
	assert_exact(&reparsed, &items);
}

#[test]
fn samples_include_what_a_javascript_number_loses() {
	let text: Vec<String> = awkward_items().iter().map(Value::to_string).collect();
	for expected in ["18446744073709551615", "9007199254740993", "42.0", "-0.0"] {
		assert!(text.iter().any(|t| t == expected), "{expected} missing");
	}
}
