//! The store adopts every `orphan_interval` on its own, for as long as it lives.

use super::*;
use contract::{append_all, tagged};
use std::time::Duration;

fn every(db_name: &str, ms: u64) -> WebConfig {
	WebConfig {
		orphan_interval: Duration::from_millis(ms),
		..config(db_name, 4096, 1000)
	}
}

async fn wait_ms(ms: u32) {
	gloo_timers::future::TimeoutFuture::new(ms).await;
}

#[wasm_bindgen_test]
async fn test_adoption_runs_again_after_each_interval() {
	let db_name = "test-interval-runs";
	let Some(cleaner) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	drop(cleaner);
	let b = WebStore::new(every(db_name, 150)).await;
	assert_eq!(b.shared.passes.get(), 1);

	// An orphan that appears after B opened is adopted by a later pass, unprompted.
	let mut a = WebStore::new(config(db_name, 4096, 1000)).await;
	append_all(&mut a, &tagged(0..3));
	drop(a);
	assert!(!b.has_data());
	until("the first orphan adopted", || queued(&b).len() >= 3).await;
	assert_eq!(queued(&b), tagged(0..3));
	let passes = b.shared.passes.get();
	assert!(passes > 1, "adopted with {passes} passes");

	// And again for the next one, by a later pass still.
	let mut a = WebStore::new(config(db_name, 4096, 1000)).await;
	append_all(&mut a, &tagged(3..5));
	drop(a);
	until("the second orphan adopted", || queued(&b).len() >= 5).await;
	assert_eq!(queued(&b), tagged(0..5));
	assert!(b.shared.passes.get() > passes);
	assert_eq!(flushed(&b).await, stored(&b));
	let mut b = b;
	b.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_adoption_stops_when_the_store_is_dropped() {
	let db_name = "test-interval-stops";
	let Some(cleaner) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	drop(cleaner);
	let b = WebStore::new(every(db_name, 50)).await;
	until("a pass on the interval", || b.shared.passes.get() > 1).await;
	// Kept past the drop, to see whether passes go on starting.
	let shared = b.shared.clone();
	drop(b);
	let passes = shared.passes.get();
	let mut a = WebStore::new(config(db_name, 4096, 1000)).await;
	append_all(&mut a, &tagged(0..2));
	drop(a);
	wait_ms(300).await;
	assert_eq!(shared.passes.get(), passes, "a pass started after the drop");
	assert!(shared.queue.borrow().items.is_empty());
	// The orphan is still there for the next store.
	assert_eq!(adopted(db_name).await, tagged(0..2));
	fresh_store(db_name, 4096, 1000).await;
	settle().await;
}

#[wasm_bindgen_test]
async fn test_memory_only_stores_never_adopt() {
	let db_name = "test-interval-memory-only";
	let Some(mut a) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	append_all(&mut a, &tagged(0..2));
	drop(a);
	let mut b = WebStore::new(every(db_name, 50)).await;
	assert_eq!(queued(&b), tagged(0..2));
	// Its connection closes, as for another page's upgrade: no pass does anything more.
	b.shared.closed.set(true);
	let mut c = WebStore::new(config(db_name, 4096, 1000)).await;
	append_all(&mut c, &tagged(5..6));
	drop(c);
	wait_ms(200).await;
	assert_eq!(queued(&b), tagged(0..2));
	b.reset();
	assert!(adopted(db_name).await.contains(&json!({"i": 5})));
	fresh_store(db_name, 4096, 1000).await;
	settle().await;
}

#[wasm_bindgen_test]
fn test_interval_delays_are_whole_milliseconds_within_timer_range() {
	use super::super::interval::millis;
	assert_eq!(millis(Duration::from_secs(30)), 30_000);
	assert_eq!(millis(Duration::from_micros(1)), 1);
	assert_eq!(millis(Duration::from_micros(1500)), 1);
	assert_eq!(millis(Duration::from_millis(2)), 2);
	assert_eq!(millis(Duration::from_secs(u64::MAX)), i32::MAX);
}
