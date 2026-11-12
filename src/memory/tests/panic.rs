//! A panic while holding an `Items` gives its items back as the stack unwinds.

use super::*;
use contract::{append_all, available, tagged};
use std::panic::{catch_unwind, AssertUnwindSafe};

#[test]
fn test_panic_while_holding_items_makes_them_available_again() {
	let mut store = store(1024, 100);
	append_all(&mut store, &tagged(0..5));
	let outcome = catch_unwind(AssertUnwindSafe(|| {
		let items = store.fetch(3, 1024).unwrap().unwrap();
		assert_eq!(items.values(), tagged(0..3));
		panic!("the send failed");
	}));
	assert!(outcome.is_err());
	// Released during unwinding, in their original order, ahead of the rest.
	assert!(store.has_data());
	assert_eq!(available(&mut store), tagged(0..5));
	let items = store.fetch(3, 1024).unwrap().unwrap();
	assert_eq!(items.values(), tagged(0..3));
	store.remove(items).unwrap();
	assert_eq!(queued(&store), tagged(3..5));
}

#[test]
fn test_panic_with_several_items_held_releases_them_all() {
	let mut store = store(1024, 100);
	append_all(&mut store, &tagged(0..6));
	let kept = store.fetch(2, 1024).unwrap().unwrap();
	let outcome = catch_unwind(AssertUnwindSafe(|| {
		let _a = store.fetch(2, 1024).unwrap().unwrap();
		let _b = store.fetch(2, 1024).unwrap().unwrap();
		panic!("boom");
	}));
	assert!(outcome.is_err());
	assert_eq!(available(&mut store), tagged(2..6));
	store.remove(kept).unwrap();
	assert_eq!(queued(&store), tagged(2..6));
}
