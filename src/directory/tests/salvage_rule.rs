//! The salvage rule itself, applied to bytes directly and to top-level scalars, whose end
//! a cut can hide.

use super::salvage_support::*;
use super::*;
use crate::directory::recovery::{salvage, Salvage};

#[test]
fn test_top_level_scalars() -> Result<()> {
	let cases: Vec<(&str, Vec<Value>)> = vec![
		// A number at the very end may have been cut short: dropped.
		("1,22,333", vec![json!(1), json!(22)]),
		("42", vec![]),
		("1,-0.5e10", vec![json!(1)]),
		// A comma, or any delimiter, after it shows it was complete.
		("1,22,333,", vec![json!(1), json!(22), json!(333)]),
		("1,22,333 ", vec![json!(1), json!(22), json!(333)]),
		("1,22,333]", vec![json!(1), json!(22), json!(333)]),
		// A number followed by anything that cannot end a number is not one.
		("1,22x", vec![json!(1)]),
		("1,22\0", vec![json!(1)]),
		// Incomplete numbers.
		("1,-", vec![json!(1)]),
		("1,2.", vec![json!(1)]),
		("1,2e", vec![json!(1)]),
		("1,2e+", vec![json!(1)]),
		// Strings and literals show their own end.
		("\"a\",\"b\"", vec![json!("a"), json!("b")]),
		(
			"true,false,null",
			vec![json!(true), json!(false), json!(null)],
		),
		("true,nul", vec![json!(true)]),
		("\"a\",\"b", vec![json!("a")]),
		("7,\"seven\"", vec![json!(7), json!("seven")]),
	];
	for (items, expected) in cases {
		let bytes = [OPENING_BYTES, items.as_bytes()].concat();
		assert_recovers_to(&bytes, &expected, 1024)
			.map_err(|e| io::Error::other(format!("{items:?}: {e}")))?;
	}
	Ok(())
}

#[test]
fn test_salvage_rule_directly() {
	let max = usize::MAX;
	assert_eq!(
		salvage(OPENING_BYTES, b"{\"items", max),
		Salvage::Unreadable
	);
	assert_eq!(salvage(OPENING_BYTES, b"", max), Salvage::Unreadable);
	assert_eq!(
		salvage(OPENING_BYTES, b"{\"events\":[1,", max),
		Salvage::Unreadable
	);
	assert_eq!(salvage(OPENING_BYTES, b"{\"items\":[", max), Salvage::Empty);
	assert_eq!(
		salvage(OPENING_BYTES, b"{\"items\":[1", max),
		Salvage::Empty
	);
	assert_eq!(
		salvage(OPENING_BYTES, b"{\"items\":[1,", max),
		Salvage::Keep(11)
	);
	assert_eq!(
		salvage(OPENING_BYTES, b"{\"items\":[ 1 , \"x\" ,", max),
		Salvage::Keep(18)
	);
	assert_eq!(
		salvage(OPENING_BYTES, b"{\"items\":[\"a\",\"bc\"", max),
		Salvage::Keep(18)
	);
	// max_len bounds the end of the last kept item.
	let bytes = b"{\"items\":[\"a\",\"bc\"";
	assert_eq!(salvage(OPENING_BYTES, bytes, 17), Salvage::Keep(13));
	assert_eq!(salvage(OPENING_BYTES, bytes, 13), Salvage::Keep(13));
	assert_eq!(salvage(OPENING_BYTES, bytes, 12), Salvage::Empty);
	// Any opening: the rule follows the one it is given.
	let renamed = Envelope::new("events").opening();
	assert_eq!(
		salvage(&renamed, b"{\"items\":[1,", max),
		Salvage::Unreadable
	);
	assert_eq!(
		salvage(&renamed, b"{\"events\":[1,", max),
		Salvage::Keep(12)
	);
}
