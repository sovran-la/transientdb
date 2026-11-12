//! `FileStore::new`: a file that does not match, files of every length, configs that
//! panic, and a file that cannot be opened yet.

use super::*;
use crate::contract::{append_all, available, item_of_len, tagged};
use crate::file::config::FileConfig;
use crate::file::crc::crc32;
use crate::file::faults::Op;
use crate::file::header::{check, encode, Mismatch};
use crate::file::layout::HEADER_LEN;
use crate::memory::{MemoryConfig, MemoryStore};
use crate::{DataStore, Error};
use serde_json::json;
use std::panic::catch_unwind;

/// Changes the header of the file at `path` with `change`, keeping its CRC right.
fn reheader(path: &Path, change: impl FnOnce(&mut [u8])) {
	let mut header = bytes_of(path)[..256].to_vec();
	change(&mut header);
	let crc = crc32(&header[..252]);
	header[252..].copy_from_slice(&crc.to_le_bytes());
	poke(path, 0, &header);
}

/// One way for a file not to match, and the config the next store is made with.
struct Case {
	reason: Mismatch,
	spoil: fn(&Path),
	config: fn(&Path) -> FileConfig,
}

fn same(path: &Path) -> FileConfig {
	config(path, SLOT, 100)
}

const CASES: [Case; 8] = [
	Case {
		reason: Mismatch::Short { len: 100 },
		spoil: |path| std::fs::write(path, &bytes_of(path)[..100]).unwrap(),
		config: same,
	},
	Case {
		reason: Mismatch::Magic,
		spoil: |path| std::fs::write(path, br#"{"batch":[1,2,3]}"#.repeat(40)).unwrap(),
		config: same,
	},
	Case {
		reason: Mismatch::Crc,
		spoil: |path| poke(path, 30, b"x"),
		config: same,
	},
	Case {
		reason: Mismatch::OlderVersion { found: 0 },
		spoil: |path| reheader(path, |h| h[8..12].copy_from_slice(&0u32.to_le_bytes())),
		config: same,
	},
	Case {
		reason: Mismatch::NewerVersion { found: 2 },
		spoil: |path| reheader(path, |h| h[8..12].copy_from_slice(&2u32.to_le_bytes())),
		config: same,
	},
	Case {
		reason: Mismatch::Id,
		spoil: |_| {},
		config: |path| FileConfig {
			id: "another-write-key".to_string(),
			..same(path)
		},
	},
	Case {
		reason: Mismatch::SlotSize {
			found: SLOT as u32,
			expected: SLOT as u32 * 2,
		},
		spoil: |_| {},
		config: |path| config(path, SLOT * 2, 100),
	},
	Case {
		reason: Mismatch::Reserved,
		spoil: |path| reheader(path, |h| h[17] = 1),
		config: same,
	},
];

#[test]
fn test_a_file_that_does_not_match_is_created_again_for_each_reason() {
	for (n, case) in CASES.iter().enumerate() {
		let (_dir, path) = temp_path();
		let mut first = store(&path, 100);
		append_all(&mut first, &tagged(0..3));
		drop(first);
		(case.spoil)(&path);
		let config = (case.config)(&path);
		let mismatch = check(&bytes_of(&path), config.slot_size(), &config.id);
		assert_eq!(mismatch, Err(case.reason.clone()), "{n}");
		let mut store = FileStore::new(config.clone());
		// Discarded with everything in it, and a fresh file in its place.
		assert_eq!(
			bytes_of(&path),
			encode(config.slot_size(), &config.id),
			"{n}"
		);
		assert!(!store.has_data(), "{n}");
		assert!(store.fetch(10, 100).unwrap().is_none(), "{n}");
		store.append(json!({"new": n})).unwrap();
		drop(store);
		assert_eq!(stored(&mut FileStore::new(config)), vec![json!({"new": n})]);
	}
}

#[test]
fn test_a_file_of_every_length_from_0_to_two_whole_slots() {
	let (dir, reference) = temp_path();
	let items = [item_of_len(3, 0), item_of_len(SLOT, 1)];
	let mut store = store(&reference, 100);
	append_all(&mut store, &items);
	drop(store);
	let full = bytes_of(&reference);
	assert_eq!(full.len() as u64, len_of(2));
	for len in 0..=full.len() {
		let path = dir.path().join(format!("len-{len}.tdb"));
		std::fs::write(&path, &full[..len]).unwrap();
		let mut store = super::store(&path, 100);
		let whole = layout().slots_in(len as u64) as usize;
		assert_eq!(stored(&mut store), items[..whole], "length {len}");
		let header_only = (len as u64) < HEADER_LEN || whole == 0;
		let after_open = if header_only { HEADER_LEN } else { len as u64 };
		assert_eq!(file_len(&path), after_open, "length {len}");
		// What a fetch hands out is what was whole; the next item follows it.
		assert_eq!(available(&mut store), items[..whole], "length {len}");
		store.append(json!(7)).unwrap();
		assert_eq!(file_len(&path), len_of(whole as u64 + 1), "length {len}");
		drop(store);
		let mut expected = items[..whole].to_vec();
		expected.push(json!(7));
		assert_eq!(reopened(&path), expected, "length {len}");
	}
}

#[test]
fn test_an_invalid_config_panics_before_touching_anything() {
	let (dir, _) = temp_path();
	let path = dir.path().join("missing").join("store.tdb");
	let configs = [
		config(&path, 0, 10),
		config(&path, SLOT, 0),
		FileConfig {
			id: String::new(),
			..config(&path, SLOT, 10)
		},
		FileConfig {
			id: "x".repeat(225),
			..config(&path, SLOT, 10)
		},
	];
	for config in configs {
		let what = format!("{config:?}");
		assert!(catch_unwind(|| FileStore::new(config)).is_err(), "{what}");
		assert!(!dir.path().join("missing").exists(), "{what}");
	}
	// The largest id is accepted.
	let mut store = FileStore::new(FileConfig {
		id: "x".repeat(224),
		..config(&path, SLOT, 10)
	});
	store.append(json!(1)).unwrap();
}

#[test]
fn test_the_parent_directory_is_created() {
	let (dir, _) = temp_path();
	let path = dir.path().join("a").join("b").join("store.tdb");
	let mut store = store(&path, 10);
	assert_eq!(bytes_of(&path), encode(SLOT as u32, ID));
	store.append(json!(1)).unwrap();
	assert!(store.has_data());
}

#[test]
fn test_items_and_their_order_survive_reopening() {
	let (_dir, path) = temp_path();
	let mut store = store(&path, 100);
	append_all(&mut store, &tagged(0..3));
	let first = store.fetch(1, 100).unwrap().unwrap();
	store.remove(first).unwrap();
	drop(store);
	let mut store = super::store(&path, 100);
	append_all(&mut store, &tagged(3..5));
	drop(store);
	let mut store = super::store(&path, 100);
	assert_eq!(available(&mut store), tagged(1..5));
}

#[test]
fn test_a_file_that_cannot_be_opened_yet_is_tried_again_by_every_call() {
	let (_dir, path) = temp_path();
	let mut first = store(&path, 100);
	append_all(&mut first, &tagged(0..3));
	drop(first);
	let bytes = bytes_of(&path);
	let mut memory = MemoryStore::new(MemoryConfig {
		max_item_size: 64,
		max_items: 10,
	});
	memory.append(json!("m")).unwrap();

	let hook = Faults::shared();
	hook.lock().unwrap().fail(Op::Open);
	let mut store = FileStore::with_hook(config(&path, SLOT, 100), hook.clone());
	assert!(matches!(store.append(json!(9)), Err(Error::Io(_))));
	assert!(matches!(store.fetch(10, 100), Err(Error::Io(_))));
	let foreign = memory.fetch(1, 100).unwrap().unwrap();
	assert!(matches!(store.remove(foreign), Err(Error::Io(_))));
	assert!(memory.has_data(), "the Items went back to its store");
	assert!(!store.has_data());
	store.reset();
	assert_eq!(bytes_of(&path), bytes);
	assert!(drain(&hook).is_empty());

	// Readable again: the next call opens it, `has_data` included.
	hook.lock().unwrap().fail.clear();
	assert!(store.has_data());
	assert_eq!(available(&mut store), tagged(0..3));
}

#[test]
fn test_a_path_that_is_a_directory_fails_every_call_and_changes_nothing() {
	let (dir, _) = temp_path();
	let mut store = FileStore::new(config(dir.path(), SLOT, 10));
	assert!(matches!(store.append(json!(1)), Err(Error::Io(_))));
	assert!(matches!(store.fetch(1, 100), Err(Error::Io(_))));
	assert!(!store.has_data());
	store.reset();
	assert!(dir.path().is_dir());
	// It holds the path all the same: a second store on it is turned away.
	let mut second = FileStore::new(config(dir.path(), SLOT, 10));
	assert!(matches!(second.append(json!(1)), Err(Error::InUse)));
}

#[cfg(unix)]
#[test]
fn test_a_symlink_to_a_missing_directory_fails_every_call_until_it_is_back() {
	let (dir, _) = temp_path();
	let gone = dir.path().join("moved-away");
	let data = dir.path().join("data");
	std::os::unix::fs::symlink("moved-away", &data).unwrap();
	let path = data.join("events.tdb");
	let mut store = store(&path, 100);
	let message = format!(
		"I/O error: {} is a symbolic link to a missing directory",
		data.display()
	);
	for _ in 0..2 {
		match store.append(json!(1)) {
			Err(e @ Error::Io(_)) => assert_eq!(e.to_string(), message),
			other => panic!("{other:?}"),
		}
		assert!(
			matches!(store.fetch(1, 100), Err(Error::Io(ref e)) if e.kind() == std::io::ErrorKind::InvalidInput)
		);
		assert!(!store.has_data());
		store.reset();
	}
	assert!(!gone.exists(), "nothing is created where the link points");
	// The path is not held: another store on it is refused for the same reason.
	let mut second = super::store(&path, 100);
	assert!(matches!(second.append(json!(1)), Err(Error::Io(_))));
	drop(second);

	// Once the directory is back, the next call opens the file there.
	std::fs::create_dir(&gone).unwrap();
	append_all(&mut store, &tagged(0..2));
	assert!(gone.join("events.tdb").is_file());
	drop(store);
	assert_eq!(
		available(&mut super::store(&gone.join("events.tdb"), 100)),
		tagged(0..2)
	);
}
