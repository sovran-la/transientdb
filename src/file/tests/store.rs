//! FileStore as a caller sees it, through `DataStore`: the spec's testing requirements at
//! the store level, and the contract every store meets.

use super::support::*;
use crate::contract;
use crate::file::faults::{Faults, Hook};
use crate::file::FileStore;
use serde_json::Value;
use std::path::Path;

mod damaged;
mod full;
mod in_use;
mod io;
mod kill;
mod open;
mod pending;
mod rejected;
mod remove;
mod reuse;
mod send;
mod sequence;
mod shrink;
mod stored_text;
mod two_stores;

/// A store on `path` with `SLOT` byte items and room for `max_items`.
pub(super) fn store(path: &Path, max_items: usize) -> FileStore {
	FileStore::new(config(path, SLOT, max_items))
}

/// A store on `path` with `SLOT` byte items and room for `max_items`, and the faults its
/// file operations obey.
pub(super) fn store_with(path: &Path, max_items: usize) -> (FileStore, Hook) {
	let hook = Faults::shared();
	let store = FileStore::with_hook(config(path, SLOT, max_items), hook.clone());
	(store, hook)
}

/// Every item the store holds, fetched or not, oldest first, read straight from its file.
pub(super) fn stored(store: &mut FileStore) -> Vec<Value> {
	let core = store.core().expect("the store has its file open");
	let texts = texts(core);
	texts
		.iter()
		.map(|t| serde_json::from_str(t).unwrap())
		.collect()
}

/// Every item a fresh store on `path` finds there, as the next run would.
pub(super) fn reopened(path: &Path) -> Vec<Value> {
	stored(&mut store(path, 1000))
}

macro_rules! file_check {
	($name:ident, $max_item_size:expr, $max_items:expr) => {
		#[test]
		fn $name() {
			let (_dir, path) = temp_path();
			let config = config(&path, $max_item_size, $max_items);
			let mut store = FileStore::new(config.clone());
			contract::$name(&mut store);
			// The file holds exactly what the store does.
			let held = stored(&mut store);
			drop(store);
			assert_eq!(stored(&mut FileStore::new(config)), held);
		}
	};
}

mod contract_checks {
	use super::*;
	contract::for_each_check!(file_check);
}
