//! Slot entries: encoding, decoding and what their CRC covers.

use crate::file::crc::crc32;
use crate::file::entry::{crc, encode_live, Entry, FREE, LEN, LIVE};

#[test]
fn test_encode_writes_every_field_in_place() {
	let bytes = encode_live(0x0102_0304_0506_0708, b"{\"a\":1}");
	assert_eq!(bytes.len(), 32);
	assert_eq!(bytes[0], LIVE);
	assert_eq!(&bytes[1..4], &[0, 0, 0]);
	assert_eq!(&bytes[4..8], &7u32.to_le_bytes());
	assert_eq!(&bytes[8..16], &0x0102_0304_0506_0708u64.to_le_bytes());
	let expected = crc(0x0102_0304_0506_0708, b"{\"a\":1}");
	assert_eq!(&bytes[16..20], &expected.to_le_bytes());
	assert_eq!(&bytes[20..32], &[0u8; 12]);
}

#[test]
fn test_crc_covers_sequence_then_length_then_payload() {
	let mut covered = Vec::new();
	covered.extend_from_slice(&42u64.to_le_bytes());
	covered.extend_from_slice(&3u32.to_le_bytes());
	covered.extend_from_slice(b"[1]");
	assert_eq!(crc(42, b"[1]"), crc32(&covered));
}

#[test]
fn test_decode_reads_back_what_encode_wrote() {
	let bytes = encode_live(7, b"\"hello\"");
	let entry = Entry::decode(&bytes);
	assert_eq!(
		entry,
		Entry {
			state: LIVE,
			length: 7,
			sequence: 7,
			crc: crc(7, b"\"hello\""),
		}
	);
	assert!(entry.matches(b"\"hello\""));
}

#[test]
fn test_matches_rejects_a_payload_that_does_not_belong() {
	let entry = Entry::decode(&encode_live(7, b"\"hello\""));
	assert!(!entry.matches(b"\"hellp\""));
	assert!(!entry.matches(b"\"hello\" "));
	assert!(!entry.matches(b"\"hell\""));
	assert!(!entry.matches(b""));
	// The same payload under another item's entry.
	let other = Entry::decode(&encode_live(8, b"\"hello\""));
	assert_ne!(other.crc, entry.crc);
	let mut forged = other;
	forged.crc = entry.crc;
	assert!(!forged.matches(b"\"hello\""));
}

#[test]
fn test_state_byte_is_outside_the_crc() {
	let mut bytes = encode_live(3, b"1");
	bytes[0] = FREE;
	let entry = Entry::decode(&bytes);
	assert_eq!(entry.state, FREE);
	assert!(entry.matches(b"1"));
}

#[test]
fn test_valid_live_needs_state_one_and_a_length_the_slot_holds() {
	let live = |state: u8, length: u32| {
		let mut bytes = [0u8; LEN];
		bytes[0] = state;
		bytes[4..8].copy_from_slice(&length.to_le_bytes());
		Entry::decode(&bytes).is_valid_live(16)
	};
	assert!(live(LIVE, 1));
	assert!(live(LIVE, 16));
	assert!(!live(LIVE, 0));
	assert!(!live(LIVE, 17));
	assert!(!live(LIVE, u32::MAX));
	assert!(!live(FREE, 5));
	for state in 2..=255u8 {
		assert!(!live(state, 5), "state {state}");
	}
}

#[test]
fn test_reserved_bytes_are_ignored_when_decoding() {
	let mut bytes = encode_live(9, b"true");
	bytes[1..4].copy_from_slice(&[1, 2, 3]);
	bytes[20..32].copy_from_slice(&[0xAB; 12]);
	let entry = Entry::decode(&bytes);
	assert!(entry.is_valid_live(16));
	assert!(entry.matches(b"true"));
}
