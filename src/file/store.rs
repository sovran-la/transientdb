//! [`FileStore`]: the public store, built on [`FileCore`] with the registry and a claim
//! ledger.

use super::config::FileConfig;
use super::core::{error, warn, FileCore};
use super::faults::Hook;
use super::index::Target;
use super::registry::{self, Lease};
use crate::claims::Ledger;
use crate::{serialize, DataStore, Error, Items, Result};
use serde_json::Value;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// A queue of items kept in one file, so they survive the process.
///
/// Each item is stored as its compact JSON text in a fixed-size slot of the store's
/// file. A removed item's slot is reused by a later append, and the file grows by one
/// slot only when none is free, so it is as large as the queue has been deep. When the
/// last stored item is removed, the file is cut back to its 256 byte header. Nothing in
/// the file is ever moved or rewritten, so a process killed at any point leaves a file
/// the next `FileStore` opens without a recovery pass. Everything `append` returned
/// `Ok` for survives a kill; nothing is synced, so a power loss may lose recent changes.
///
/// Fetched items are tracked in memory only: after a restart every stored item is
/// fetched again, including any that were fetched but not yet removed.
///
/// A file is used by one store at a time. A second `FileStore` made for a file another
/// one in this process is using is created, but leaves the file alone: its `append`,
/// `fetch` and `remove` return [`Error::InUse`], `has_data` is false and `reset` does
/// nothing, until the first store is dropped; its next call then opens the file. Two
/// processes must not use one file: nothing stops them, and items can be lost or sent
/// twice.
///
/// # Where to put the file
///
/// - Not in a temporary or cache directory: the OS may delete those while the app is
///   suspended, and writes to a deleted file succeed and go nowhere.
/// - Preferably in the app's own container rather than one shared with another app or
///   an extension. No file lock is taken, so a suspended app is not terminated for
///   holding one.
/// - With a data-protection class that allows access whenever the app can run. While
///   the file cannot be read or written, calls return [`Error::Io`] and change nothing;
///   they work again once it can.
///
/// # Examples
/// ```
/// use serde_json::json;
/// use transientdb::{DataStore, FileConfig, FileStore};
///
/// let dir = tempfile::tempdir().unwrap();
/// let config = FileConfig {
///     path: dir.path().join("events.tdb"),
///     id: "write-key".to_string(),
///     max_item_size: 8 * 1024,
///     max_items: 1000,
/// };
///
/// let mut store = FileStore::new(config.clone());
/// store.append(json!({"event": "open"})).unwrap();
/// store.append(json!({"event": "close"})).unwrap();
/// drop(store);
///
/// // A later run finds the items where the last one left them.
/// let mut store = FileStore::new(config);
/// let items = store.fetch(100, 64 * 1024).unwrap().unwrap();
/// assert_eq!(items.json(), r#"[{"event":"open"},{"event":"close"}]"#);
/// // Delivered: remove them, and the file is back to its header.
/// store.remove(items).unwrap();
/// assert!(!store.has_data());
/// ```
pub struct FileStore {
	config: FileConfig,
	ledger: Ledger,
	hook: Hook,
	/// Behind a mutex only so `has_data`, which takes `&self`, can take the file once
	/// another store gives it up. Every other call reaches it through `&mut self`
	/// without locking.
	state: Mutex<State>,
}

/// How far a store has got with its file.
#[derive(Default)]
struct State {
	/// The open file, once it has been opened. Declared before `lease` so it is closed
	/// before the path is given up.
	core: Option<FileCore>,
	/// The store's claim on its path, held from when it is first taken until the store
	/// is dropped.
	lease: Option<Lease>,
}

impl FileStore {
	/// Creates a store on the file at `config.path`, creating the file and its parent
	/// directory if they are missing (see [`FileConfig::path`]).
	///
	/// A file written for another `id` or `max_item_size`, an older or newer format, or
	/// one that is damaged or is not a FileStore file, is deleted with everything in it,
	/// logged, and created again. This does not fail: if the file cannot be opened now,
	/// or another store in this process is using it, the store is still created and
	/// every call tries again (see [`FileStore`]).
	///
	/// # Panics
	/// * If `max_item_size` is 0, or larger than `u32::MAX`
	/// * If `max_items` is 0
	/// * If `id` is empty or longer than 224 bytes
	pub fn new(config: FileConfig) -> Self {
		// Outside tests a hook is empty; in them, `new` gets one that injects nothing.
		#[allow(clippy::default_constructed_unit_structs)]
		let hook = Hook::default();
		Self::with_hook(config, hook)
	}

	/// `new`, with faults to inject into every file operation (none outside tests).
	pub(super) fn with_hook(config: FileConfig, hook: Hook) -> Self {
		config.validate();
		let mut store = Self {
			config,
			ledger: Ledger::new(),
			hook,
			state: Mutex::new(State::default()),
		};
		match store.parts() {
			Ok(_) => {}
			Err(Error::InUse) => warn(
				&store.config.path,
				format_args!(
					"another store in this process is using this file; this one waits for it"
				),
			),
			Err(e) => error(
				&store.config.path,
				format_args!("could not open the file ({e}); every call tries again"),
			),
		}
		store
	}

	/// The open core, opening the file first if this store has not yet, and the ledger.
	fn parts(&mut self) -> Result<(&mut FileCore, &Ledger)> {
		let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
		Ok((ready(state, &self.config, &self.hook)?, &self.ledger))
	}

	/// The state, locked, for `has_data`.
	fn state(&self) -> MutexGuard<'_, State> {
		self.state.lock().unwrap_or_else(PoisonError::into_inner)
	}

	#[cfg(test)]
	pub(super) fn core(&mut self) -> Option<&mut FileCore> {
		let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
		state.core.as_mut()
	}

	#[cfg(test)]
	pub(super) fn ledger(&self) -> &Ledger {
		&self.ledger
	}
}

/// Opens the file for `state` if it is not open yet: takes the path, unless another store
/// holds it, then opens the file.
///
/// # Errors
/// [`Error::InUse`] if another store in this process holds the path; [`Error::Io`] if
/// the path cannot be resolved or the file cannot be opened. Either way nothing in the
/// file changed, and the next call tries again. A path once taken is kept.
fn ready<'a>(state: &'a mut State, config: &FileConfig, hook: &Hook) -> Result<&'a mut FileCore> {
	if state.core.is_none() {
		if state.lease.is_none() {
			let path = registry::canonical(&config.path)?;
			state.lease = Some(Lease::acquire(path).ok_or(Error::InUse)?);
		}
		let path = state.lease.as_ref().expect("taken above").path();
		state.core = Some(FileCore::open(path, config, hook.clone())?);
	}
	Ok(state.core.as_mut().expect("opened above"))
}

impl DataStore for FileStore {
	/// Appends an item, dropping the oldest one if the store holds `max_items` already.
	///
	/// # Errors
	/// [`Error::ItemTooDeep`] or [`Error::ItemTooLarge`], writing nothing, as described
	/// on [`DataStore::append`]. The item is checked before the file is touched, so these
	/// are returned for a bad item even while the store cannot use its file, and come
	/// before the errors below. [`Error::InUse`], doing nothing, while another store in
	/// this process is using the file. [`Error::Io`] if the file cannot be written: the
	/// item is not stored, and every other item is as it was, except that a write that
	/// failed while dropping the oldest item may have dropped it. Also [`Error::Io`],
	/// writing nothing, once a damaged file has used up every sequence number: until the
	/// store is reset, or every item is removed and a new store opens the file again.
	fn append(&mut self, data: Value) -> Result<()> {
		// The item first: a bad one is rejected the same way whatever state the file is in,
		// and a deep one is taken apart without recursion before anything can drop it.
		let text = serialize::item_text(data, self.config.max_item_size)?;
		let (core, ledger) = self.parts()?;
		let oldest = match core.index.target(core.max_items) {
			Target::Oldest { sequence, .. } => Some(sequence),
			_ => None,
		};
		let appended = core.append_text(&text);
		// The oldest item's slot now holds (part of) the new one: whoever holds it in an
		// `Items` gets nothing for it back.
		if let Some(sequence) = oldest.filter(|&sequence| core.index.get(sequence).is_none()) {
			ledger.lock().forget(sequence);
		}
		appended?;
		Ok(())
	}

	/// Hands out the oldest items, as described on [`DataStore::fetch`], reading each one
	/// from the file and checking it. A damaged item is deleted, logged and skipped, and
	/// does not count toward `count` or `max_bytes`.
	///
	/// # Errors
	/// [`Error::InUse`], doing nothing, while another store in this process is using the
	/// file. [`Error::Io`] if the file cannot be read: nothing is handed out, and a later
	/// fetch returns the same items.
	fn fetch(&mut self, count: usize, max_bytes: usize) -> Result<Option<Items>> {
		let (core, ledger) = self.parts()?;
		if count == 0 {
			return Ok(None);
		}
		// Only this store hands its items out, and it is busy here, so what is available
		// now can only grow until the items read are handed out: anything released
		// meanwhile waits for the next fetch, and nothing read is taken by another.
		let available: Vec<u64> = {
			let claims = ledger.lock();
			let live = core.index.live().map(|(sequence, _)| sequence);
			live.filter(|&sequence| !claims.is_pending(sequence))
				.collect()
		};
		let read = core.read_items(count, max_bytes, |sequence| {
			available.binary_search(&sequence).is_err()
		})?;
		let candidates = read
			.iter()
			.map(|(sequence, text)| (*sequence, text.as_str()));
		Ok(ledger.issue(candidates, count, max_bytes))
	}

	/// Deletes the items in an `Items` this store handed out, as described on
	/// [`DataStore::remove`]. When that leaves nothing stored, the file is cut back to its
	/// header.
	///
	/// # Errors
	/// [`Error::InUse`], doing nothing, while another store in this process is using the
	/// file; `items` is released. [`Error::Io`] with the first failure if an item cannot
	/// be deleted: every item is tried, and those that failed stay stored and are fetched
	/// again.
	fn remove(&mut self, items: Items) -> Result<()> {
		let (core, ledger) = match self.parts() {
			Ok(parts) => parts,
			Err(e) => {
				// Nothing this store has not opened can be its own: back to its issuer.
				drop(items);
				return Err(e);
			}
		};
		let mut first_error = None;
		for sequence in ledger.settle(items) {
			if let Err(e) = core.free(sequence) {
				first_error.get_or_insert(e);
			}
		}
		match first_error {
			Some(e) => Err(Error::Io(e)),
			None => Ok(()),
		}
	}

	/// Deletes every item by cutting the file back to its header, as described on
	/// [`DataStore::reset`]. Does nothing while another store in this process is using
	/// the file, or if the file cannot be opened or cut; that is logged.
	fn reset(&mut self) {
		let (core, ledger) = match self.parts() {
			Ok(parts) => parts,
			Err(Error::InUse) => return,
			Err(e) => {
				error(&self.config.path, format_args!("reset did nothing: {e}"));
				return;
			}
		};
		if core.reset() {
			ledger.lock().reset();
		}
	}

	/// Whether a fetch could return something. False while another store in this
	/// process is using the file, or while the file cannot be opened.
	///
	/// No item is read for this, so a damaged item counts until a fetch finds it and
	/// frees it: a store whose only items are damaged has data until its next fetch,
	/// which returns `None`.
	fn has_data(&self) -> bool {
		let mut state = self.state();
		let Ok(core) = ready(&mut state, &self.config, &self.hook) else {
			return false;
		};
		let live = core.index.live().map(|(sequence, _)| sequence);
		self.ledger.any_available(live)
	}
}

// Moved to another thread, as `TransientDB` does with its store.
const _: () = {
	const fn assert_send<T: Send>() {}
	assert_send::<FileStore>();
};
