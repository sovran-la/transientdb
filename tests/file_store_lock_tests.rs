//! A FileStore's `Items` crosses threads, and dropping one never waits for the store: not
//! while another thread is inside a call holding the `TransientDB` mutex, not after the
//! store is gone.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use transientdb::{DataStore, FileConfig, FileStore, Items, Result, TransientDB};

fn file_store(path: &Path, n: usize) -> FileStore {
	let mut store = FileStore::new(FileConfig {
		path: path.to_path_buf(),
		id: "write-key".to_string(),
		max_item_size: 1024,
		max_items: 1000,
	});
	for item in tagged(0..n) {
		store.append(item).unwrap();
	}
	store
}

#[test]
fn test_fetch_on_one_thread_remove_on_another_through_transient_db() {
	let dir = tempfile::tempdir().unwrap();
	let db = Arc::new(TransientDB::new(file_store(&dir.path().join("e.tdb"), 9)));
	let (send, receive) = channel::<Items>();
	let fetcher = {
		let db = Arc::clone(&db);
		thread::spawn(move || {
			while let Some(items) = db.fetch(3, 4096).unwrap() {
				send.send(items).unwrap();
			}
		})
	};
	let remover = {
		let db = Arc::clone(&db);
		within_deadline("removing on another thread", move || {
			let mut removed = Vec::new();
			for items in receive.iter() {
				removed.extend(items.values());
				db.remove(items).unwrap();
			}
			removed
		})
	};
	fetcher.join().unwrap();
	assert_eq!(remover, tagged(0..9));
	assert!(!db.has_data());
}

/// A FileStore whose next call can be made to stop inside the store, with the
/// `TransientDB` mutex held, until the test lets it go on.
struct Gated {
	inner: FileStore,
	gate: Gate,
}

type Gate = Arc<Mutex<Option<(Sender<()>, Receiver<()>)>>>;

impl Gated {
	fn checkpoint(&self) {
		if let Some((entered, proceed)) = self.gate.lock().unwrap().take() {
			entered.send(()).unwrap();
			proceed.recv_timeout(DEADLINE).unwrap();
		}
	}
}

impl DataStore for Gated {
	fn append(&mut self, data: Value) -> Result<()> {
		self.checkpoint();
		self.inner.append(data)
	}

	fn fetch(&mut self, count: usize, max_bytes: usize) -> Result<Option<Items>> {
		self.checkpoint();
		self.inner.fetch(count, max_bytes)
	}

	fn remove(&mut self, items: Items) -> Result<()> {
		self.checkpoint();
		self.inner.remove(items)
	}

	fn reset(&mut self) {
		self.checkpoint();
		self.inner.reset();
	}

	fn has_data(&self) -> bool {
		self.checkpoint();
		self.inner.has_data()
	}
}

/// Stops the next call on `db` inside the store, on a new thread running `call`, and
/// returns once it has stopped there: the sender lets it go on, the handle joins it.
fn stop_inside<T: Send + 'static>(
	db: &Arc<TransientDB>,
	gate: &Gate,
	call: impl FnOnce(&TransientDB) -> T + Send + 'static,
) -> (Sender<()>, thread::JoinHandle<T>) {
	let (entered_tx, entered) = channel();
	let (proceed, proceed_rx) = channel();
	*gate.lock().unwrap() = Some((entered_tx, proceed_rx));
	let blocked_db = Arc::clone(db);
	let blocked = thread::spawn(move || call(&blocked_db));
	entered
		.recv_timeout(DEADLINE)
		.expect("the call never entered the store");
	(proceed, blocked)
}

#[test]
fn test_dropping_items_while_a_call_holds_the_mutex_does_not_deadlock() {
	let dir = tempfile::tempdir().unwrap();
	let gate = Gate::default();
	let store = Gated {
		inner: file_store(&dir.path().join("e.tdb"), 4),
		gate: gate.clone(),
	};
	let db = Arc::new(TransientDB::new(store));
	let dropped = db.fetch(2, 4096).unwrap().unwrap();
	let released = db.fetch(1, 4096).unwrap().unwrap();

	let (proceed, blocked) = stop_inside(&db, &gate, |db| db.append(json!({"i": 4})));
	within_deadline("dropping Items while the store is busy", move || {
		drop(dropped);
		released.release();
	});
	proceed.send(()).unwrap();
	blocked.join().unwrap().unwrap();
	assert_eq!(available(&db), tagged(0..5));

	// The same while `has_data`, which only borrows the store, is inside it.
	let held = db.fetch(5, 4096).unwrap().unwrap();
	let (proceed, blocked) = stop_inside(&db, &gate, |db| db.has_data());
	within_deadline("dropping Items during has_data", move || drop(held));
	proceed.send(()).unwrap();
	// Either answer is right for a has_data racing the release; what matters is that
	// both finish.
	let _ = blocked.join().unwrap();
	assert!(db.has_data());
}

#[test]
fn test_items_outliving_the_db_and_its_store_release_harmlessly() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("e.tdb");
	let db = TransientDB::new(file_store(&path, 3));
	let items = db.fetch(3, 4096).unwrap().unwrap();
	drop(db);
	within_deadline("releasing after the store is gone", move || {
		assert_eq!(items.values(), tagged(0..3));
		drop(items);
	});
	// And the file still holds them for the next store.
	let db = TransientDB::new(file_store(&path, 0));
	assert_eq!(available(&db), tagged(0..3));
}
