//! A file that does not match the config is deleted and created again, once for each
//! reason listed under Open.

use super::support::*;
use crate::file::config::FileConfig;
use crate::file::core::FileCore;
use crate::file::crc::crc32;
use crate::file::faults::{Faults, Op};
use crate::file::header::{check, encode, Mismatch};
use std::path::Path;

/// A store file at `path` holding two items, made with `config`.
fn stored(config: &FileConfig) {
	let (mut core, _) = open_config(config);
	core.append_text("[1]").unwrap();
	core.append_text("[2]").unwrap();
}

/// Changes the header of the file at `path` with `change`, keeping its CRC right.
fn reheader(path: &Path, change: impl FnOnce(&mut [u8])) {
	let mut header = bytes_of(path)[..256].to_vec();
	change(&mut header);
	let crc = crc32(&header[..252]);
	header[252..].copy_from_slice(&crc.to_le_bytes());
	poke(path, 0, &header);
}

/// Opens `path` with `config` and checks the old file was discarded for `reason`.
fn assert_discarded(path: &Path, config: &FileConfig, reason: Mismatch) {
	let header = bytes_of(path);
	assert_eq!(
		check(&header, config.slot_size(), &config.id),
		Err(reason.clone())
	);
	let hook = Faults::shared();
	let mut core = FileCore::open(path, config, hook.clone()).unwrap();
	assert_eq!(
		drain(&hook),
		vec![Op::Delete, Op::Create, Op::Header],
		"{reason:?}"
	);
	assert_eq!(
		bytes_of(path),
		encode(config.slot_size(), &config.id),
		"{reason:?}"
	);
	assert!(texts(&mut core).is_empty(), "{reason:?}");
	assert_eq!(core.append_text("[3]").unwrap(), 1, "{reason:?}");
}

#[test]
fn test_cut_short() {
	for len in [0usize, 1, 8, 100, 255] {
		let (_dir, path) = temp_path();
		let config = config(&path, SLOT, 10);
		stored(&config);
		let bytes = bytes_of(&path);
		std::fs::write(&path, &bytes[..len]).unwrap();
		assert_discarded(&path, &config, Mismatch::Short { len: len as u64 });
	}
}

#[test]
fn test_not_a_filestore_file() {
	let (_dir, path) = temp_path();
	let config = config(&path, SLOT, 10);
	std::fs::write(&path, br#"{"batch":[1,2,3]}"#.repeat(40)).unwrap();
	assert_discarded(&path, &config, Mismatch::Magic);
}

#[test]
fn test_damaged_header() {
	let (_dir, path) = temp_path();
	let config = config(&path, SLOT, 10);
	stored(&config);
	poke(&path, 30, b"x");
	assert_discarded(&path, &config, Mismatch::Crc);
}

#[test]
fn test_older_version() {
	let (_dir, path) = temp_path();
	let config = config(&path, SLOT, 10);
	stored(&config);
	reheader(&path, |h| h[8..12].copy_from_slice(&0u32.to_le_bytes()));
	assert_discarded(&path, &config, Mismatch::OlderVersion { found: 0 });
}

#[test]
fn test_newer_version() {
	let (_dir, path) = temp_path();
	let config = config(&path, SLOT, 10);
	stored(&config);
	reheader(&path, |h| h[8..12].copy_from_slice(&2u32.to_le_bytes()));
	assert_discarded(&path, &config, Mismatch::NewerVersion { found: 2 });
}

#[test]
fn test_another_id() {
	let (_dir, path) = temp_path();
	stored(&config(&path, SLOT, 10));
	let mut other = config(&path, SLOT, 10);
	other.id = "another-write-key".to_string();
	assert_discarded(&path, &other, Mismatch::Id);
	// And back: the first id's items were never in the new file.
	assert_discarded(&path, &config(&path, SLOT, 10), Mismatch::Id);
}

#[test]
fn test_another_slot_size() {
	let (_dir, path) = temp_path();
	stored(&config(&path, SLOT, 10));
	assert_discarded(
		&path,
		&config(&path, SLOT * 2, 10),
		Mismatch::SlotSize {
			found: SLOT as u32,
			expected: SLOT as u32 * 2,
		},
	);
}

#[test]
fn test_a_reserved_byte_set() {
	let (_dir, path) = temp_path();
	let config = config(&path, SLOT, 10);
	stored(&config);
	reheader(&path, |h| h[17] = 1);
	assert_discarded(&path, &config, Mismatch::Reserved);
}

#[test]
fn test_a_failed_delete_or_create_is_an_error_and_the_next_open_recovers() {
	for op in [Op::Delete, Op::Create, Op::Header] {
		let (_dir, path) = temp_path();
		stored(&config(&path, SLOT, 10));
		let other = config(&path, SLOT * 2, 10);
		let hook = Faults::shared();
		hook.lock().unwrap().fail(op);
		assert!(FileCore::open(&path, &other, hook).is_err(), "{op:?}");
		let (mut core, _) = open_config(&other);
		assert!(texts(&mut core).is_empty(), "{op:?}");
		assert_eq!(bytes_of(&path), encode(other.slot_size(), &other.id));
	}
}

#[test]
fn test_a_kill_while_writing_the_header_is_created_again_at_the_next_open() {
	for budget in 0..256 {
		let (_dir, path) = temp_path();
		let config = config(&path, SLOT, 10);
		let hook = Faults::shared();
		hook.lock().unwrap().write_budget = Some(budget);
		assert!(FileCore::open(&path, &config, hook).is_err());
		assert_eq!(file_len(&path), budget as u64);
		let (mut core, hook) = open_config(&config);
		assert_eq!(drain(&hook), vec![Op::Delete, Op::Create, Op::Header]);
		assert!(texts(&mut core).is_empty());
		assert_eq!(bytes_of(&path), encode(config.slot_size(), &config.id));
	}
}
