//! Sequence numbers: from 1, increasing, never reused, never wrapping.

use super::*;
use crate::Error;
use contract::{append_all, drain, tagged};

fn seqs(store: &MemoryStore) -> Vec<u64> {
	store.queue.items.iter().map(|item| item.seq).collect()
}

#[test]
fn test_sequence_numbers_start_at_one_and_survive_removal_and_reset() {
	let mut store = store(1024, 100);
	append_all(&mut store, &tagged(0..3));
	assert_eq!(seqs(&store), vec![1, 2, 3]);
	let items = store.fetch(2, 1024).unwrap().unwrap();
	store.remove(items).unwrap();
	append_all(&mut store, &tagged(3..4));
	assert_eq!(seqs(&store), vec![3, 4]);
	store.reset();
	append_all(&mut store, &tagged(4..5));
	assert_eq!(seqs(&store), vec![5]);
}

#[test]
fn test_rejected_items_use_no_sequence_number() {
	let mut store = store(8, 100);
	store.append(json!(1)).unwrap();
	assert!(store.append(json!("far too long")).is_err());
	store.append(json!(2)).unwrap();
	assert_eq!(seqs(&store), vec![1, 2]);
}

#[test]
fn test_sequence_numbers_stop_before_wrapping() {
	let mut store = store(1024, 100);
	store.append(json!("first")).unwrap();
	store.queue.next_seq = u64::MAX - 1;
	store.append(json!("last")).unwrap();
	assert_eq!(seqs(&store), vec![1, u64::MAX - 1]);
	// The next item would need a sequence number some item could end up sharing.
	for _ in 0..3 {
		let err = store.append(json!("one too many")).unwrap_err();
		assert!(matches!(&err, Error::Io(e) if e.to_string() == "sequence numbers exhausted"));
	}
	assert_eq!(queued(&store), vec![json!("first"), json!("last")]);

	// What is queued is still handed out and removed exactly.
	let items = store.fetch(1, 1024).unwrap().unwrap();
	assert_eq!(items.values(), vec![json!("first")]);
	store.remove(items).unwrap();
	assert_eq!(drain(&mut store, 10, 1024).concat(), vec![json!("last")]);
}
