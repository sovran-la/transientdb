//! One store per file in a process: a second store on a path the first holds, spelled the
//! same, relative or through a symbolic link, is created but does nothing, until the first
//! is dropped.

use super::*;
use crate::contract::{append_all, available, tagged};
use crate::file::config::FileConfig;
use crate::file::faults::Op;
use crate::memory::{MemoryConfig, MemoryStore};
use crate::{DataStore, Error};
use serde_json::json;
use std::path::PathBuf;

/// A hook that fails every file operation: a store made with it that touches its file in
/// any way reports `Io`, not `InUse`.
fn untouchable() -> Hook {
	let hook = Faults::shared();
	for op in Op::ALL {
		hook.lock().unwrap().fail(op);
	}
	hook
}

/// The configs a second store might be made with: the first's, and two that would not
/// match its file.
fn second_configs(path: &Path) -> [FileConfig; 3] {
	[
		config(path, SLOT, 100),
		FileConfig {
			id: "another-write-key".to_string(),
			..config(path, SLOT, 100)
		},
		config(path, SLOT * 4, 7),
	]
}

/// Both spellings of `path`.
fn spellings(path: &Path) -> [PathBuf; 2] {
	[path.to_path_buf(), relative(path)]
}

#[test]
fn test_a_second_store_on_one_path_does_nothing_and_leaves_the_file_untouched() {
	let (_dir, path) = temp_path();
	let mut first = store(&path, 100);
	append_all(&mut first, &tagged(0..4));
	for spelling in spellings(&path) {
		for second_config in second_configs(&spelling) {
			let what = format!("{second_config:?}");
			let held = first.fetch(2, 100).unwrap().unwrap();
			let bytes = bytes_of(&path);
			let mut second = FileStore::with_hook(second_config, untouchable());
			assert!(
				matches!(second.append(json!(1)), Err(Error::InUse)),
				"{what}"
			);
			assert!(matches!(second.fetch(10, 100), Err(Error::InUse)), "{what}");
			assert!(!second.has_data(), "{what}");
			second.reset();
			// An Items passed to it goes back to the store that issued it.
			assert!(matches!(second.remove(held), Err(Error::InUse)), "{what}");
			assert_eq!(available(&mut first), tagged(0..4), "{what}");
			assert_eq!(bytes_of(&path), bytes, "{what}");
			assert!(second.core().is_none(), "{what}");
		}
	}
	// The first store is unaffected.
	assert_eq!(stored(&mut first), tagged(0..4));
}

/// A call, by name.
type Call = (&'static str, fn(&mut FileStore));

/// Each call that takes the file once the first store is gone.
const FIRST_CALLS: [Call; 5] = [
	("append", |store| store.append(json!({"i": 4})).unwrap()),
	("fetch", |store| {
		let items = store.fetch(1, 100).unwrap().unwrap();
		assert_eq!(items.values(), tagged(0..1));
	}),
	("remove", |store| {
		let mut other = MemoryStore::new(MemoryConfig {
			max_item_size: 64,
			max_items: 10,
		});
		other.append(json!(1)).unwrap();
		store.remove(other.fetch(1, 100).unwrap().unwrap()).unwrap();
		assert!(other.has_data(), "released back to its store");
	}),
	("has_data", |store| assert!(store.has_data())),
	("reset", |store| store.reset()),
];

#[test]
fn test_after_the_first_is_dropped_the_next_call_takes_the_file() {
	for (name, call) in FIRST_CALLS {
		let (_dir, path) = temp_path();
		let mut first = store(&path, 100);
		append_all(&mut first, &tagged(0..4));
		let mut second = store(&relative(&path), 100);
		assert!(matches!(second.fetch(1, 100), Err(Error::InUse)), "{name}");
		drop(first);
		call(&mut second);
		assert!(second.core().is_some(), "{name}");
		// From then on it works normally.
		let expected = match name {
			"append" => tagged(0..5),
			"reset" => Vec::new(),
			_ => tagged(0..4),
		};
		assert_eq!(stored(&mut second), expected, "{name}");
		second.append(json!({"i": 9})).unwrap();
		// And it holds the path now.
		let mut third = store(&path, 100);
		assert!(
			matches!(third.append(json!(1)), Err(Error::InUse)),
			"{name}"
		);
		drop(second);
		assert!(third.has_data(), "{name}");
	}
}

#[test]
fn test_a_second_store_that_would_not_match_discards_the_file_only_once_it_takes_it() {
	let (_dir, path) = temp_path();
	let mut first = store(&path, 100);
	append_all(&mut first, &tagged(0..2));
	let other = FileConfig {
		id: "another-write-key".to_string(),
		..config(&path, SLOT, 100)
	};
	let mut second = FileStore::new(other.clone());
	assert!(!second.has_data());
	assert_eq!(stored(&mut first), tagged(0..2));
	drop(first);
	// Its first call opens the file for its own id: what was queued is discarded.
	assert!(!second.has_data());
	assert_eq!(
		bytes_of(&path),
		crate::file::header::encode(SLOT as u32, &other.id)
	);
	second.append(json!(1)).unwrap();
	assert_eq!(available(&mut second), vec![json!(1)]);
}

#[test]
fn test_dropping_a_store_that_never_took_its_path_frees_nothing() {
	let (_dir, path) = temp_path();
	let mut first = store(&path, 100);
	first.append(json!(1)).unwrap();
	drop(store(&path, 100));
	// The first store's claim is intact.
	let mut third = store(&path, 100);
	assert!(matches!(third.append(json!(2)), Err(Error::InUse)));
	assert!(first.has_data());
}

#[test]
fn test_two_paths_in_one_directory_are_two_stores() {
	let (dir, path) = temp_path();
	let mut a = store(&path, 100);
	let mut b = store(&dir.path().join("other.tdb"), 100);
	a.append(json!("a")).unwrap();
	b.append(json!("b")).unwrap();
	assert_eq!(available(&mut a), vec![json!("a")]);
	assert_eq!(available(&mut b), vec![json!("b")]);
}

#[cfg(unix)]
#[test]
fn test_a_store_on_a_symlink_to_a_missing_file_creates_the_file_at_its_target() {
	let (_dir, real) = temp_path();
	let link = real.with_file_name("link.tdb");
	std::os::unix::fs::symlink("store.tdb", &link).unwrap();
	let mut store = store(&link, 100);
	append_all(&mut store, &tagged(0..2));
	assert!(real.is_file(), "the file is created where the link points");
	assert!(std::fs::symlink_metadata(&link)
		.unwrap()
		.file_type()
		.is_symlink());
	// The target's own path is the same store.
	let mut second = FileStore::with_hook(config(&real, SLOT, 100), untouchable());
	assert!(matches!(second.append(json!(1)), Err(Error::InUse)));
	drop(second);
	drop(store);
	// The next run finds the items through either spelling.
	assert_eq!(reopened(&link), tagged(0..2));
	assert_eq!(available(&mut super::store(&real, 100)), tagged(0..2));
}
