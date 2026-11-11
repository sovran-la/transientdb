//! An item `append` must reject is rejected as every store rejects it, even by a store
//! that cannot use its file: one whose path another store holds, or whose file cannot be
//! opened. The item is checked before the file is touched, so the error names the item,
//! and nothing is written.

use super::*;
use crate::contract::{self, append_all, available, tagged};
use crate::{DataStore, Error};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// Why a case's store cannot use its file.
#[derive(Debug, Clone, Copy)]
enum Without {
	/// Another store in this process holds its path.
	InUse,
	/// A directory is where its file should be, so the file cannot be opened.
	Unopenable,
}

/// A store that cannot use its file, for `why`, and what keeps it from it.
struct Case {
	_dir: TempDir,
	path: PathBuf,
	/// The store holding the path, for `Without::InUse`.
	holder: Option<FileStore>,
	store: FileStore,
	/// The file's bytes, or `None` for the directory, before the store was used.
	before: Option<Vec<u8>>,
}

fn case(why: Without) -> Case {
	let (_dir, path) = temp_path();
	let holder = match why {
		Without::InUse => {
			let mut holder = store(&path, 100);
			append_all(&mut holder, &tagged(0..2));
			Some(holder)
		}
		Without::Unopenable => {
			fs::create_dir(&path).unwrap();
			None
		}
	};
	let store = store(&path, 100);
	let before = holder.as_ref().map(|_| bytes_of(&path));
	Case {
		_dir,
		path,
		holder,
		store,
		before,
	}
}

/// What a call on a store without its file returns for `why`.
fn unusable(why: Without, result: crate::Result<()>) -> bool {
	match why {
		Without::InUse => matches!(result, Err(Error::InUse)),
		Without::Unopenable => matches!(result, Err(Error::Io(_))),
	}
}

/// Checks that rejecting left everything as it was, then that the store takes its file
/// once it can, and still rejects the same items as it did without it.
fn assert_untouched_then_usable(mut case: Case, why: Without, rejects: impl Fn(&mut FileStore)) {
	assert!(case.store.core().is_none(), "{why:?}");
	assert!(!case.store.has_data(), "{why:?}");
	assert!(unusable(why, case.store.append(json!(1))), "{why:?}");
	match case.before {
		Some(bytes) => assert_eq!(bytes_of(&case.path), bytes, "{why:?}"),
		None => assert_eq!(fs::read_dir(&case.path).unwrap().count(), 0, "{why:?}"),
	}
	if let Some(mut holder) = case.holder.take() {
		assert_eq!(available(&mut holder), tagged(0..2));
		drop(holder);
	} else {
		fs::remove_dir(&case.path).unwrap();
	}
	rejects(&mut case.store);
	let expected = match why {
		Without::InUse => [tagged(0..2), vec![json!("next")]].concat(),
		Without::Unopenable => vec![json!("next")],
	};
	case.store.append(json!("next")).unwrap();
	assert_eq!(stored(&mut case.store), expected, "{why:?}");
}

#[test]
fn test_a_store_without_its_file_rejects_an_oversized_item_as_any_store_does() {
	for why in [Without::InUse, Without::Unopenable] {
		let mut case = case(why);
		contract::assert_rejects_oversized_items(&mut case.store, SLOT);
		assert_untouched_then_usable(case, why, |store| {
			contract::assert_rejects_oversized_items(store, SLOT)
		});
	}
}

/// Deeper than `append` allows, so that rejecting it is the check under test, and shallow
/// enough that dropping it the ordinary, recursive way is safe: an item nested in objects
/// takes about 1 KiB of stack per level to drop in a debug build, so this one takes about
/// half a MiB of the 2 MiB a test thread gets, and no more on a 32-bit target. A store that
/// took its file before checking the item would fail here with the wrong error rather
/// than abort the test binary. That the check itself does not recurse, however deep the
/// item, is tested on stores that have their file.
const DEEP: usize = 500;

#[test]
fn test_a_store_without_its_file_rejects_a_too_deep_item_before_touching_the_file() {
	for why in [Without::InUse, Without::Unopenable] {
		let mut case = case(why);
		contract::assert_rejects_too_deep_items(&mut case.store, DEEP);
		assert_untouched_then_usable(case, why, |store| {
			contract::assert_rejects_too_deep_items(store, DEEP)
		});
	}
}
