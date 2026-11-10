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

#[test]
fn test_check_text_accepts_what_item_text_produces() {
	for item in awkward_items() {
		let text = item_text(item, 4096).unwrap();
		assert_eq!(check_text(&text, 4096), Ok(()), "{text}");
		assert_eq!(check_text(&text, text.len()), Ok(()), "{text}");
	}
	let deepest = item_text(nested(MAX_ITEM_DEPTH, Shape::Mixed, json!({})), 4096).unwrap();
	assert_eq!(check_text(&deepest, 4096), Ok(()));
}

#[test]
fn test_check_text_rejects_malformed_text() {
	for text in [
		"",
		" ",
		"not json",
		"{\"a\":1} trailing",
		"[1,",
		"{",
		"'s'",
		"1 2",
	] {
		assert_eq!(check_text(text, 4096), Err(Unfit::Malformed), "{text:?}");
	}
}

#[test]
fn test_check_text_rejects_text_over_max_item_size() {
	assert_eq!(
		check_text("\"12345\"", 6),
		Err(Unfit::TooLarge { size: 7, max: 6 })
	);
	assert_eq!(check_text("\"12345\"", 7), Ok(()));
}

#[test]
fn test_check_text_rejects_text_too_deep_without_overflowing_the_stack() {
	// Deeper than the limit, but shallow enough for serde_json to parse.
	for depth in [MAX_ITEM_DEPTH + 1, MAX_ITEM_DEPTH + 2] {
		let text = nested(depth, Shape::Arrays, json!(0)).to_string();
		assert_eq!(check_text(&text, 4096), Err(Unfit::TooDeep), "{depth}");
	}
	// Far deeper: serde_json refuses it, so it is malformed as far as a store can tell.
	let text = "[".repeat(100_000) + &"]".repeat(100_000);
	assert_eq!(check_text(&text, 1_000_000), Err(Unfit::Malformed));
}

#[test]
fn test_unfit_describes_itself() {
	assert_eq!(Unfit::Malformed.to_string(), "it is not well-formed JSON");
	assert_eq!(
		Unfit::TooLarge { size: 9, max: 8 }.to_string(),
		"it is 9 bytes, over max_item_size = 8 bytes"
	);
	assert_eq!(
		Unfit::TooDeep.to_string(),
		"it is nested more than 125 levels deep"
	);
}
