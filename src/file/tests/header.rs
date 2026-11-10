//! The header: what this build writes, and every way a header can fail to match it.

use crate::file::crc::crc32;
use crate::file::header::{check, encode, Mismatch, MAGIC, MAX_ID_LEN, VERSION};

/// `header` with its CRC made right again after a change.
fn resealed(mut header: [u8; 256]) -> [u8; 256] {
	let crc = crc32(&header[..252]);
	header[252..].copy_from_slice(&crc.to_le_bytes());
	header
}

#[test]
fn test_encode_writes_every_field_in_place() {
	let header = encode(8192, "key");
	assert_eq!(&header[0..8], b"TDBFILE\0");
	assert_eq!(&header[0..8], MAGIC);
	assert_eq!(&header[8..12], &1u32.to_le_bytes());
	assert_eq!(VERSION, 1);
	assert_eq!(&header[12..16], &8192u32.to_le_bytes());
	assert_eq!(&header[16..20], &[0; 4]);
	assert_eq!(&header[20..24], &3u32.to_le_bytes());
	assert_eq!(&header[24..27], b"key");
	assert!(header[27..248].iter().all(|&b| b == 0));
	assert_eq!(&header[248..252], &[0; 4]);
	assert_eq!(&header[252..256], &crc32(&header[..252]).to_le_bytes());
}

#[test]
fn test_ids_of_every_allowed_length() {
	for len in 1..=MAX_ID_LEN {
		let id = "x".repeat(len);
		let header = encode(16, &id);
		assert_eq!(&header[20..24], &(len as u32).to_le_bytes());
		assert_eq!(&header[24..24 + len], id.as_bytes());
		assert_eq!(check(&header, 16, &id), Ok(()));
	}
}

#[test]
fn test_id_is_stored_as_utf8_not_hashed() {
	let header = encode(16, "clé");
	assert_eq!(&header[24..28], "clé".as_bytes());
	assert_eq!(check(&header, 16, "clé"), Ok(()));
	assert_eq!(check(&header, 16, "cle"), Err(Mismatch::Id));
}

#[test]
#[should_panic(expected = "id must be 1 to 224 bytes")]
fn test_encode_refuses_an_empty_id() {
	encode(16, "");
}

#[test]
#[should_panic(expected = "id must be 1 to 224 bytes")]
fn test_encode_refuses_a_long_id() {
	encode(16, &"x".repeat(225));
}

#[test]
fn test_the_written_header_matches() {
	assert_eq!(check(&encode(16, "key"), 16, "key"), Ok(()));
	// Bytes after the header are not the header's business.
	let mut longer = encode(16, "key").to_vec();
	longer.extend_from_slice(&[0xAB; 40]);
	assert_eq!(check(&longer, 16, "key"), Ok(()));
}

#[test]
fn test_short_files_of_every_length() {
	let header = encode(16, "key");
	for len in 0..256 {
		assert_eq!(
			check(&header[..len], 16, "key"),
			Err(Mismatch::Short { len: len as u64 }),
			"length {len}"
		);
	}
}

#[test]
fn test_bad_magic() {
	let mut header = encode(16, "key");
	header[0] = b'X';
	assert_eq!(check(&resealed(header), 16, "key"), Err(Mismatch::Magic));
	assert_eq!(check(&[0u8; 256], 16, "key"), Err(Mismatch::Magic));
	assert_eq!(check(&[b'{'; 300], 16, "key"), Err(Mismatch::Magic));
}

#[test]
fn test_bad_crc_for_any_damaged_byte() {
	let good = encode(16, "key");
	for at in 8..256 {
		let mut header = good;
		header[at] ^= 0x40;
		assert_eq!(check(&header, 16, "key"), Err(Mismatch::Crc), "byte {at}");
	}
}

#[test]
fn test_versions_older_and_newer() {
	for (version, expected) in [
		(0, Mismatch::OlderVersion { found: 0 }),
		(2, Mismatch::NewerVersion { found: 2 }),
		(u32::MAX, Mismatch::NewerVersion { found: u32::MAX }),
	] {
		let mut header = encode(16, "key");
		header[8..12].copy_from_slice(&u32::to_le_bytes(version));
		assert_eq!(check(&resealed(header), 16, "key"), Err(expected));
	}
}

#[test]
fn test_another_id() {
	let header = encode(16, "key");
	assert_eq!(check(&header, 16, "kez"), Err(Mismatch::Id));
	assert_eq!(check(&header, 16, "ke"), Err(Mismatch::Id));
	assert_eq!(check(&header, 16, "keys"), Err(Mismatch::Id));
	// An id length the header cannot hold.
	for len in [0u32, 225, u32::MAX] {
		let mut header = encode(16, "key");
		header[20..24].copy_from_slice(&len.to_le_bytes());
		assert_eq!(check(&resealed(header), 16, "key"), Err(Mismatch::Id));
	}
}

#[test]
fn test_another_slot_size() {
	let header = encode(16, "key");
	assert_eq!(
		check(&header, 17, "key"),
		Err(Mismatch::SlotSize {
			found: 16,
			expected: 17
		})
	);
}

#[test]
fn test_a_reserved_or_padding_byte_that_is_not_zero() {
	for at in (16..20).chain(27..248).chain(248..252) {
		let mut header = encode(16, "key");
		header[at] = 1;
		assert_eq!(
			check(&resealed(header), 16, "key"),
			Err(Mismatch::Reserved),
			"byte {at}"
		);
	}
}

#[test]
fn test_every_reason_says_what_is_wrong() {
	let cases = [
		(
			Mismatch::Short { len: 12 },
			"it is 12 bytes, shorter than a header",
		),
		(Mismatch::Magic, "it is not a FileStore file"),
		(Mismatch::Crc, "its header is damaged"),
		(
			Mismatch::OlderVersion { found: 0 },
			"it is format version 0, older than 1",
		),
		(
			Mismatch::NewerVersion { found: 3 },
			"it is format version 3, newer than 1",
		),
		(Mismatch::Id, "it is held for another id"),
		(
			Mismatch::SlotSize {
				found: 16,
				expected: 32,
			},
			"its slots hold 16 bytes, not the 32 configured",
		),
		(
			Mismatch::Reserved,
			"its header has unexpected reserved bytes",
		),
	];
	for (mismatch, text) in cases {
		assert_eq!(mismatch.to_string(), text);
		assert!(!text.contains('\n'));
	}
}
