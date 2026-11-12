//! The config's limits, checked before any file is touched.

use super::support::config;
use crate::file::header::MAX_ID_LEN;
use std::path::Path;

fn validated(max_item_size: usize, max_items: usize, id: &str) {
	let mut config = config(Path::new("unused.tdb"), max_item_size, max_items);
	config.id = id.to_string();
	config.validate();
}

#[test]
fn test_smallest_and_largest_allowed_values() {
	validated(1, 1, "k");
	validated(8192, 2560, &"k".repeat(MAX_ID_LEN));
	#[cfg(target_pointer_width = "64")]
	validated(u32::MAX as usize, usize::MAX, "k");
	let config = config(Path::new("x"), 8192, 1);
	assert_eq!(config.slot_size(), 8192);
}

#[test]
#[should_panic(expected = "max_item_size = 0")]
fn test_zero_max_item_size_panics() {
	validated(0, 1, "k");
}

#[test]
#[should_panic(expected = "max_items = 0")]
fn test_zero_max_items_panics() {
	validated(1, 0, "k");
}

#[test]
#[should_panic(expected = "id is empty")]
fn test_empty_id_panics() {
	validated(1, 1, "");
}

#[test]
#[should_panic(expected = "id is 225 bytes")]
fn test_long_id_panics() {
	validated(1, 1, &"k".repeat(MAX_ID_LEN + 1));
}

#[test]
#[should_panic(expected = "id is 226 bytes")]
fn test_id_length_is_counted_in_bytes() {
	// 113 two-byte characters: 113 characters, 226 bytes.
	validated(1, 1, &"é".repeat(113));
}

#[cfg(target_pointer_width = "64")]
#[test]
#[should_panic(expected = "max_item_size over 4294967295 bytes")]
fn test_max_item_size_past_32_bits_panics() {
	validated(u32::MAX as usize + 1, 1, "k");
}
