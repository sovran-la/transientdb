//! Dropping an `Items` never waits for the store: not while another thread is inside a
//! store call holding the `TransientDB` mutex, and not from inside a store call on the
//! same thread.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use common::*;
use serde_json::{json, Value};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use transientdb::{DataStore, Items, MemoryStore, Result, TransientDB};

/// A MemoryStore whose calls can be made to stop inside the store, with the
/// `TransientDB` mutex held, until the test lets them go on; and that can drop an
/// `Items` from inside its own calls.
struct Gated {
	inner: MemoryStore,
	/// When set, the next call says it has entered, then waits to be let go on.
	gate: Gate,
	/// When set, the next call drops this `Items` before doing its work.
	to_drop: Slot,
}

impl Gated {
	fn checkpoint(&self) {
		drop(self.to_drop.lock().unwrap().take());
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

type Gate = Arc<Mutex<Option<(Sender<()>, Receiver<()>)>>>;
type Slot = Arc<Mutex<Option<Items>>>;

fn gated_db(n: usize) -> (Arc<TransientDB>, Gate, Slot) {
	let (gate, to_drop) = (Gate::default(), Slot::default());
	let mut inner = memory();
	for item in tagged(0..n) {
		inner.append(item).unwrap();
	}
	let store = Gated {
		inner,
		gate: gate.clone(),
		to_drop: to_drop.clone(),
	};
	(Arc::new(TransientDB::new(store)), gate, to_drop)
}

#[test]
fn test_drop_on_another_thread_while_a_store_call_holds_the_mutex() {
	let (db, gate, _) = gated_db(4);
	let items = db.fetch(2, 4096).unwrap().unwrap();
	assert_eq!(items.values(), tagged(0..2));

	// A thread enters `append` and stops there, holding the TransientDB mutex.
	let (entered_tx, entered) = channel();
	let (proceed, proceed_rx) = channel();
	*gate.lock().unwrap() = Some((entered_tx, proceed_rx));
	let blocked_db = db.clone();
	let blocked = thread::spawn(move || blocked_db.append(json!({"i": 4})));
	entered
		.recv_timeout(DEADLINE)
		.expect("append never entered the store");

	// Dropping the Items on yet another thread finishes while the mutex is held.
	within_deadline("dropping Items while the store is busy", move || {
		drop(items)
	});

	proceed.send(()).unwrap();
	blocked.join().unwrap().unwrap();
	// The dropped items are available again, ahead of the rest.
	assert_eq!(available(&db), tagged(0..5));
}

#[test]
fn test_release_on_another_thread_while_a_store_call_holds_the_mutex() {
	let (db, gate, _) = gated_db(3);
	let items = db.fetch(3, 4096).unwrap().unwrap();
	let (entered_tx, entered) = channel();
	let (proceed, proceed_rx) = channel();
	*gate.lock().unwrap() = Some((entered_tx, proceed_rx));
	let blocked_db = db.clone();
	let blocked = thread::spawn(move || blocked_db.has_data());
	entered.recv_timeout(DEADLINE).unwrap();

	within_deadline("Items::release while the store is busy", move || {
		items.release()
	});

	proceed.send(()).unwrap();
	assert!(blocked.join().unwrap(), "released items count as data");
	assert_eq!(available(&db), tagged(0..3));
}

#[test]
fn test_drop_from_inside_a_store_call_on_the_same_thread() {
	let (db, _, to_drop) = gated_db(3);
	let items = db.fetch(2, 4096).unwrap().unwrap();
	*to_drop.lock().unwrap() = Some(items);
	// `append` drops the Items on this thread with the TransientDB mutex held.
	let appender = db.clone();
	within_deadline("append dropping Items inside the store", move || {
		appender.append(json!({"i": 3})).unwrap()
	});
	assert!(to_drop.lock().unwrap().is_none());
	assert_eq!(available(&db), tagged(0..4));

	// The same from inside `has_data`, which only borrows the store.
	let items = db.fetch(4, 4096).unwrap().unwrap();
	*to_drop.lock().unwrap() = Some(items);
	let checker = db.clone();
	assert!(within_deadline("has_data dropping Items", move || checker.has_data()));
}
