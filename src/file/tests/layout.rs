//! Where slots are in the file.

use crate::file::layout::{Layout, ENTRY_LEN, HEADER_LEN};

#[test]
fn test_offsets() {
	let layout = Layout { slot_size: 100 };
	assert_eq!(layout.slot_len(), 132);
	assert_eq!(layout.entry_offset(0), 256);
	assert_eq!(layout.payload_offset(0), 288);
	assert_eq!(layout.entry_offset(1), 388);
	assert_eq!(layout.payload_offset(1), 420);
	assert_eq!(layout.entry_offset(10), 256 + 1320);
}

#[test]
fn test_slots_in_rounds_down() {
	let layout = Layout { slot_size: 16 };
	let slot = ENTRY_LEN + 16;
	for len in 0..HEADER_LEN + 3 * slot {
		let expected = if len < HEADER_LEN {
			0
		} else {
			(len - HEADER_LEN) / slot
		};
		assert_eq!(layout.slots_in(len), expected, "length {len}");
	}
	assert_eq!(layout.slots_in(HEADER_LEN + slot - 1), 0);
	assert_eq!(layout.slots_in(HEADER_LEN + slot), 1);
}

#[test]
fn test_the_specs_size_example() {
	let layout = Layout { slot_size: 8192 };
	assert_eq!(layout.entry_offset(2560), 21_053_696);
	assert_eq!(layout.entry_offset(50), 411_456);
}
