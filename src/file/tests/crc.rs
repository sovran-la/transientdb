//! CRC-32 against the standard check value and other known values.

use crate::file::crc::{crc32, Crc32};

#[test]
fn test_check_value() {
	assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}

#[test]
fn test_known_values() {
	assert_eq!(crc32(b""), 0);
	assert_eq!(crc32(b"a"), 0xE8B7_BE43);
	assert_eq!(
		crc32(b"The quick brown fox jumps over the lazy dog"),
		0x414F_A339
	);
	assert_eq!(crc32(&[0u8; 32]), 0x190A_55AD);
	assert_eq!(crc32(&[0xFFu8; 32]), 0xFF6C_AB0B);
}

#[test]
fn test_pieces_give_the_crc_of_the_whole() {
	let data = b"123456789 and then some more bytes";
	for split in 0..=data.len() {
		let (a, b) = data.split_at(split);
		assert_eq!(Crc32::new().update(a).update(b).finish(), crc32(data));
	}
}

#[test]
fn test_every_single_bit_change_is_caught() {
	let data = *b"123456789";
	for byte in 0..data.len() {
		for bit in 0..8 {
			let mut flipped = data;
			flipped[byte] ^= 1 << bit;
			assert_ne!(crc32(&flipped), crc32(&data), "byte {byte} bit {bit}");
		}
	}
}
