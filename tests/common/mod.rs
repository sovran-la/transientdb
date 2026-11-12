//! Helpers shared by the integration tests.

#![allow(dead_code)] // each test crate uses only some of them

use serde_json::{json, Value};
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;
use transientdb::{MemoryConfig, MemoryStore, TransientDB};

/// Long enough for any of these steps on a loaded machine; a deadlock fails the test
/// after this instead of hanging it.
pub const DEADLINE: Duration = Duration::from_secs(10);

pub fn tagged(range: std::ops::Range<usize>) -> Vec<Value> {
	range.map(|i| json!({ "i": i })).collect()
}

pub fn memory() -> MemoryStore {
	MemoryStore::new(MemoryConfig {
		max_item_size: 1024,
		max_items: 1000,
	})
}

pub fn db_holding(n: usize) -> TransientDB {
	let db = TransientDB::new(memory());
	for item in tagged(0..n) {
		db.append(item).unwrap();
	}
	db
}

pub fn available(db: &TransientDB) -> Vec<Value> {
	db.fetch(usize::MAX, usize::MAX)
		.unwrap()
		.map_or_else(Vec::new, |items| items.values())
}

/// Runs `f` on a new thread and returns its result, failing the test if it takes longer
/// than [`DEADLINE`].
pub fn within_deadline<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
	let (done, finished) = channel();
	let handle = thread::spawn(move || {
		let _ = done.send(f());
	});
	let value = finished
		.recv_timeout(DEADLINE)
		.unwrap_or_else(|_| panic!("{what} did not finish: deadlocked or panicked"));
	handle.join().unwrap();
	value
}

/// Lets fire-and-forget IndexedDB writes complete: waits 100 ms on a browser timer.
///
/// Not `gloo_timers`: with wasm-bindgen 0.2.95 (as locked) and a rustc that uses v0
/// symbol names (1.99 does), a test binary in which both `gloo_timers` and this crate
/// create a `Closure<dyn FnMut()>` gets two closure shims exported under one demangled
/// name, and the browser rejects the module ("duplicate export"). Do not reintroduce
/// it here. (The library's unit tests use `gloo_timers` safely: there it is a dependency
/// of the crate under test, which reuses its shim instead of making a second one.)
#[cfg(target_arch = "wasm32")]
pub async fn settle() {
	let timer = js_sys::Promise::new(&mut |resolve, _reject| {
		web_sys::window()
			.expect("a browser window")
			.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 100)
			.expect("setTimeout");
	});
	wasm_bindgen_futures::JsFuture::from(timer)
		.await
		.expect("timer");
}
