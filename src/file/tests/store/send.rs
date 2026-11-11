//! A FileStore and its `Items` move between threads: fetch on one, remove or release on
//! another.

use super::*;
use crate::contract::{append_all, available, tagged};
use crate::DataStore;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread;

#[test]
fn test_items_fetched_on_one_thread_are_removed_on_another() {
	let (_dir, path) = temp_path();
	let mut store = store(&path, 100);
	append_all(&mut store, &tagged(0..6));
	let store = Arc::new(Mutex::new(store));
	let (send, receive) = channel();
	let fetcher = {
		let store = Arc::clone(&store);
		thread::spawn(move || {
			for _ in 0..3 {
				let items = store.lock().unwrap().fetch(2, 100).unwrap().unwrap();
				send.send(items).unwrap();
			}
		})
	};
	let remover = {
		let store = Arc::clone(&store);
		thread::spawn(move || {
			for (n, items) in receive.iter().enumerate() {
				if n == 1 {
					items.release();
				} else {
					store.lock().unwrap().remove(items).unwrap();
				}
			}
		})
	};
	fetcher.join().unwrap();
	remover.join().unwrap();
	let mut store = Arc::try_unwrap(store).ok().unwrap().into_inner().unwrap();
	assert_eq!(stored(&mut store), tagged(2..4));
	assert_eq!(available(&mut store), tagged(2..4));
}

#[test]
fn test_a_store_moves_to_another_thread_and_keeps_its_file() {
	let (_dir, path) = temp_path();
	let mut store = store(&path, 100);
	append_all(&mut store, &tagged(0..2));
	let items = store.fetch(1, 100).unwrap().unwrap();
	let mut store = thread::spawn(move || {
		store.remove(items).unwrap();
		store
	})
	.join()
	.unwrap();
	assert_eq!(available(&mut store), tagged(1..2));
	// Still the path's holder.
	let mut second = super::store(&path, 100);
	assert!(matches!(second.fetch(1, 100), Err(crate::Error::InUse)));
}
