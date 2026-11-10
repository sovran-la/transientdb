use super::*;

fn texts(n: u64) -> Vec<(u64, String)> {
	(1..=n).map(|seq| (seq, format!("{seq}"))).collect()
}

fn issue(ledger: &Ledger, candidates: &[(u64, String)], count: usize, max: usize) -> Option<Items> {
	ledger.issue(candidates.iter().map(|(s, t)| (*s, t.as_str())), count, max)
}

#[test]
fn test_issue_marks_what_it_hands_out() {
	let ledger = Ledger::new();
	let all = texts(5);
	let items = issue(&ledger, &all, 2, 100).unwrap();
	assert_eq!(items.seqs(), vec![1, 2]);
	let claims = ledger.lock();
	assert!(claims.is_pending(1) && claims.is_pending(2) && !claims.is_pending(3));
	assert_eq!(claims.pending_count(), 2);
}

#[test]
fn test_issue_skips_pending_and_stops_at_the_first_misfit() {
	let ledger = Ledger::new();
	let all = vec![
		(1, "a".to_string()),
		(2, "bb".to_string()),
		(3, "cccccc".to_string()),
		(4, "d".to_string()),
	];
	let first = issue(&ledger, &all, 1, 100).unwrap();
	assert_eq!(first.seqs(), vec![1]);
	// 2 fits, 3 does not, and 4 must not jump ahead of it.
	let second = issue(&ledger, &all, 10, 5).unwrap();
	assert_eq!(second.seqs(), vec![2]);
	assert!(issue(&ledger, &all, 10, 5).is_none());
	assert!(issue(&ledger, &all, 0, 100).is_none());
	let third = issue(&ledger, &all, 10, 7).unwrap();
	assert_eq!(third.seqs(), vec![3, 4]);
	assert!(issue(&ledger, &all, 10, 100).is_none());
}

#[test]
fn test_release_only_in_the_epoch_it_was_issued() {
	let ledger = Ledger::new();
	let mut claims = ledger.lock();
	claims.pending.insert(1);
	let old_epoch = claims.epoch;
	claims.reset();
	assert_eq!(claims.pending_count(), 0);
	// The same sequence number claimed again in the new epoch.
	claims.pending.insert(1);
	claims.release(old_epoch, [1]);
	assert!(
		claims.is_pending(1),
		"an old release must not free a new claim"
	);
	let epoch = claims.epoch;
	claims.release(epoch, [1]);
	assert!(!claims.is_pending(1));
}

#[test]
fn test_items_from_before_a_reset_do_not_free_reused_sequence_numbers() {
	let ledger = Ledger::new();
	let all = texts(3);
	let old_released = issue(&ledger, &all, 2, 100).unwrap();
	let old_settled = issue(&ledger, &all, 1, 100).unwrap();
	ledger.lock().reset();
	// A store that gave the same sequence numbers out again after the reset.
	let new = issue(&ledger, &all, 3, 100).unwrap();
	assert_eq!(new.seqs(), vec![1, 2, 3]);
	drop(old_released);
	assert_eq!(ledger.lock().pending_count(), 3);
	assert!(ledger.settle(old_settled).is_empty());
	assert_eq!(ledger.lock().pending_count(), 3);
	assert_eq!(ledger.settle(new), vec![1, 2, 3]);
	assert_eq!(ledger.lock().pending_count(), 0);
}

#[test]
fn test_settle_returns_only_what_is_still_pending() {
	let ledger = Ledger::new();
	let all = texts(4);
	let items = issue(&ledger, &all, 4, 100).unwrap();
	// Item 2 was dropped to make room after the fetch.
	ledger.lock().forget(2);
	assert_eq!(ledger.settle(items), vec![1, 3, 4]);
	assert_eq!(ledger.lock().pending_count(), 0);
}

#[test]
fn test_settle_ignores_and_releases_another_ledgers_items() {
	let (mine, theirs) = (Ledger::new(), Ledger::new());
	let all = texts(2);
	let foreign = issue(&theirs, &all, 2, 100).unwrap();
	let _held = issue(&mine, &all, 1, 100).unwrap();
	assert!(mine.settle(foreign).is_empty());
	assert_eq!(mine.lock().pending_count(), 1, "mine untouched");
	assert_eq!(
		theirs.lock().pending_count(),
		0,
		"released to its own ledger"
	);
}

#[test]
fn test_ledgers_are_the_same_only_when_shared() {
	let a = Ledger::new();
	let b = Ledger::new();
	assert!(a.same(&a.clone()));
	assert!(!a.same(&b));
}

#[test]
fn test_any_available() {
	let ledger = Ledger::new();
	assert!(!ledger.any_available([]));
	assert!(ledger.any_available([1, 2]));
	let held = issue(&ledger, &texts(2), 2, 100).unwrap();
	assert!(!ledger.any_available([1, 2]));
	assert!(ledger.any_available([1, 2, 3]));
	drop(held);
	assert!(ledger.any_available([1]));
}

#[test]
fn test_a_poisoned_ledger_still_works() {
	let ledger = Ledger::new();
	let items = issue(&ledger, &texts(2), 2, 100).unwrap();
	let poisoner = ledger.clone();
	let outcome = std::thread::spawn(move || {
		let _guard = poisoner.lock();
		panic!("poison the ledger");
	})
	.join();
	assert!(outcome.is_err());
	assert!(ledger.0.is_poisoned());
	drop(items);
	assert_eq!(ledger.lock().pending_count(), 0);
	assert!(issue(&ledger, &texts(2), 2, 100).is_some());
}
