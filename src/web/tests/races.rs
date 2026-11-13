//! Adoption against everything else going on: two tabs adopting at once, the store's sync
//! calls between a pass's steps, a reset or a drop in the middle of a pass, and a pass
//! started while another runs.

use super::*;
use contract::{append_all, tagged};
use std::cell::Cell;
use std::rc::Rc;

/// `count` dead tabs on `db_name`, holding `per_tab` items each, all tagged by tab.
async fn orphans(db_name: &str, count: usize, per_tab: usize) -> Vec<Value> {
	let mut all = Vec::new();
	let mut stores = Vec::new();
	for t in 0..count {
		let mut store = WebStore::new(config(db_name, 4096, 1 << 20)).await;
		let items: Vec<Value> = (0..per_tab).map(|i| json!({"t": t, "i": i})).collect();
		append_all(&mut store, &items);
		all.extend(items);
		stores.push(store);
	}
	all
}

/// Starts a pass of `store`'s in the background; the flag is set once it has finished.
fn spawn_pass(store: &WebStore) -> Rc<Cell<bool>> {
	let done = Rc::new(Cell::new(false));
	let (shared, finished) = (store.shared.clone(), done.clone());
	wasm_bindgen_futures::spawn_local(async move {
		adopt::pass(&shared).await;
		finished.set(true);
	});
	done
}

/// Waits, a few milliseconds at a time, until every flag is set.
async fn until_all(flags: &[Rc<Cell<bool>>]) {
	for _ in 0..1000 {
		if flags.iter().all(|f| f.get()) {
			return;
		}
		gloo_timers::future::TimeoutFuture::new(5).await;
	}
	panic!("a pass did not finish");
}

fn sorted(mut values: Vec<Value>) -> Vec<String> {
	let mut texts: Vec<String> = values.drain(..).map(|v| v.to_string()).collect();
	texts.sort();
	texts
}

#[wasm_bindgen_test]
async fn test_racing_adopters_each_get_an_orphan_exactly_once() {
	let db_name = "test-races-adopters";
	let Some(mut b) = persisted(db_name, 4096, 1 << 20).await else {
		return;
	};
	let mut c = WebStore::new(config(db_name, 4096, 1 << 20)).await;
	for round in 0..3 {
		let all = orphans(db_name, 6, 40).await;
		let passes = [spawn_pass(&b), spawn_pass(&c), spawn_pass(&b)];
		until_all(&passes).await;

		let (in_b, in_c) = (queued(&b), queued(&c));
		// Every orphan is in exactly one of them: none twice, none lost.
		assert_eq!(
			sorted([in_b.clone(), in_c.clone()].concat()),
			sorted(all.clone())
		);
		// Each orphan went whole to one adopter, in its order.
		for adopter in [&in_b, &in_c] {
			for t in 0..6 {
				let mine: Vec<&Value> = adopter.iter().filter(|v| v["t"] == t).collect();
				assert!(
					mine.is_empty() || mine.len() == 40,
					"round {round}: tab {t} split"
				);
				let order: Vec<u64> = mine.iter().map(|v| v["i"].as_u64().unwrap()).collect();
				assert!(order.windows(2).all(|w| w[0] < w[1]));
			}
		}
		// IndexedDB holds exactly what the two hold, under their tabs, and nothing else.
		assert_eq!(flushed(&b).await, stored(&b));
		assert_eq!(flushed(&c).await, stored(&c));
		assert_eq!(records(&b).await.len(), all.len());
		b.reset();
		c.reset();
	}
	settle().await;
}

/// The store's sync calls run between the steps of a pass, and see a consistent store
/// throughout: nothing panics, nothing is lost or doubled, and memory and IndexedDB agree
/// once it is done.
#[wasm_bindgen_test]
async fn test_sync_calls_run_freely_during_a_pass() {
	let db_name = "test-races-sync-calls";
	let Some(mut b) = persisted(db_name, 4096, 1 << 20).await else {
		return;
	};
	let orphaned = orphans(db_name, 4, 30).await;
	let pass = spawn_pass(&b);
	let mut appended = Vec::new();
	let mut sent = Vec::new();
	let mut steps = 0;
	while !pass.get() {
		let item = json!({"own": steps});
		b.append(item.clone()).unwrap();
		appended.push(item);
		assert!(b.has_data());
		if let Some(items) = b.fetch(3, 4096).unwrap() {
			sent.extend(items.values());
			b.remove(items).unwrap();
		}
		steps += 1;
		gloo_timers::future::TimeoutFuture::new(0).await;
	}
	assert!(steps > 1, "the pass finished before any call ran during it");
	let left = available(&mut b);
	assert_eq!(
		sorted([sent, left].concat()),
		sorted([orphaned, appended].concat())
	);
	assert_eq!(flushed(&b).await, stored(&b));
	assert_eq!(records(&b).await.len(), stored(&b).len());
	b.reset();
	settle().await;
}

/// A reset while a pass is moving records adopts none of them: what the reset cleared
/// stays cleared, in memory and in IndexedDB.
#[wasm_bindgen_test]
async fn test_a_reset_during_a_pass_wins() {
	let db_name = "test-races-reset";
	let Some(mut b) = persisted(db_name, 4096, 1 << 20).await else {
		return;
	};
	// Every point in the pass, from before it reads the tabs to after it is done.
	for delay in 0..8 {
		orphans(db_name, 3, 20).await;
		let pass = spawn_pass(&b);
		for _ in 0..delay {
			gloo_timers::future::TimeoutFuture::new(0).await;
		}
		b.reset();
		until_all(&[pass]).await;
		settle().await;
		// Whatever the pass took before the reset is gone with it, and nothing after it.
		assert!(!b.has_data(), "delay {delay}: {:?}", queued(&b));
		assert!(records(&b).await.is_empty(), "delay {delay}");
	}
}

/// Dropping a store in the middle of a pass stops it: whatever it had moved under its tab
/// is an orphan again, as its lock is released, and the next store adopts all of it.
#[wasm_bindgen_test]
async fn test_dropping_a_store_during_a_pass_loses_nothing() {
	let db_name = "test-races-drop";
	let Some(cleaner) = persisted(db_name, 4096, 1 << 20).await else {
		return;
	};
	drop(cleaner);
	for delay in 0..10 {
		// Open before the orphans die, so that only the pass adopts them.
		let mut b = WebStore::new(config(db_name, 4096, 1 << 20)).await;
		append_all(&mut b, &[json!({"b": delay})]);
		let all = orphans(db_name, 3, 20).await;
		let pass = spawn_pass(&b);
		for _ in 0..delay {
			gloo_timers::future::TimeoutFuture::new(0).await;
		}
		drop(b);
		until_all(&[pass]).await;
		let mut c = WebStore::new(config(db_name, 4096, 1 << 20)).await;
		let expected = [vec![json!({"b": delay})], all].concat();
		assert_eq!(sorted(queued(&c)), sorted(expected), "delay {delay}");
		assert_eq!(flushed(&c).await, stored(&c), "delay {delay}");
		assert_eq!(records(&c).await.len(), stored(&c).len(), "delay {delay}");
		c.reset();
		drop(c);
		settle().await;
	}
}

/// A pass started while another runs does nothing; the interval's ticks rely on it.
#[wasm_bindgen_test]
async fn test_a_pass_never_runs_alongside_another() {
	let db_name = "test-races-overlap";
	let Some(b) = persisted(db_name, 4096, 1 << 20).await else {
		return;
	};
	let all = orphans(db_name, 3, 20).await;
	let before = b.shared.passes.get();
	let first = spawn_pass(&b);
	// Let it start: it runs up to its first IndexedDB request, which cannot have been
	// answered within the same round of microtasks.
	let resolved = js_sys::Promise::resolve(&JsValue::NULL);
	wasm_bindgen_futures::JsFuture::from(resolved)
		.await
		.unwrap();
	assert!(b.shared.passing.get());
	b.adopt_now().await;
	let others = [spawn_pass(&b), spawn_pass(&b)];
	until_all(&[first]).await;
	until_all(&others).await;
	assert_eq!(b.shared.passes.get(), before + 1);
	assert!(!b.shared.passing.get());
	assert_eq!(sorted(queued(&b)), sorted(all));
	// Once it is over, the next one runs.
	b.adopt_now().await;
	assert_eq!(b.shared.passes.get(), before + 2);
	let mut b = b;
	b.reset();
	settle().await;
}

/// Appending past `max_items` while a pass has the queue over the cap (adopted, not yet
/// trimmed) deletes every item it drops from IndexedDB, not just one.
#[wasm_bindgen_test]
async fn test_appends_during_a_pass_trim_everything_they_drop() {
	let db_name = "test-races-trim";
	let Some(mut b) = persisted(db_name, 4096, 5).await else {
		return;
	};
	for delay in 0..10 {
		orphans(db_name, 3, 4).await;
		let pass = spawn_pass(&b);
		for _ in 0..delay {
			gloo_timers::future::TimeoutFuture::new(0).await;
		}
		append_all(&mut b, &tagged(0..2));
		until_all(&[pass]).await;
		assert_eq!(queued(&b).len(), 5, "delay {delay}");
		assert_eq!(flushed(&b).await, stored(&b), "delay {delay}");
		assert_eq!(records(&b).await.len(), 5, "delay {delay}");
		b.reset();
	}
	settle().await;
}
