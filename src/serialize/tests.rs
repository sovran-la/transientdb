use super::*;
use crate::item_samples::awkward_items;
use serde_json::json;

#[test]
fn test_item_text_is_compact_serde_json_text() {
	for item in awkward_items() {
		assert_eq!(item_text(item.clone(), 4096).unwrap(), item.to_string());
	}
	assert_eq!(
		item_text(json!({"a": [1, 2], "b": null}), 100).unwrap(),
		r#"{"a":[1,2],"b":null}"#
	);
	assert_eq!(item_text(json!(42.0), 100).unwrap(), "42.0");
	assert_eq!(
		item_text(json!(u64::MAX), 100).unwrap(),
		"18446744073709551615"
	);
}

#[test]
fn test_item_text_at_and_over_max_item_size() {
	// "12345" in quotes is 7 bytes.
	assert_eq!(item_text(json!("12345"), 7).unwrap(), "\"12345\"");
	let err = item_text(json!("12345"), 6).unwrap_err();
	assert!(
		matches!(err, Error::ItemTooLarge { size: 7, max: 6 }),
		"{err:?}"
	);
	assert_eq!(item_text(json!(7), 1).unwrap(), "7");
	assert!(matches!(
		item_text(json!(10), 1),
		Err(Error::ItemTooLarge { size: 2, max: 1 })
	));
}

#[test]
fn test_item_text_counts_bytes_not_characters() {
	// Four characters, each four bytes in UTF-8, plus two quotes.
	let crabs = json!("🦀🦀🦀🦀");
	assert_eq!(item_text(crabs.clone(), 18).unwrap().len(), 18);
	assert!(matches!(
		item_text(crabs, 17),
		Err(Error::ItemTooLarge { size: 18, max: 17 })
	));
}

#[test]
fn test_item_text_checks_depth_before_size() {
	// Far too deep and far too large: the depth check runs first, without recursion.
	let err = item_text(nested(100_000, Shape::Arrays, json!(0)), 1).unwrap_err();
	assert!(matches!(err, Error::ItemTooDeep), "{err:?}");
	let deepest = nested(MAX_ITEM_DEPTH, Shape::Objects, json!(0));
	assert!(item_text(deepest, 1_000_000).is_ok());
	let past = nested(MAX_ITEM_DEPTH + 1, Shape::Objects, json!(0));
	assert!(matches!(
		item_text(past, 1_000_000),
		Err(Error::ItemTooDeep)
	));
}
