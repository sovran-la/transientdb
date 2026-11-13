use super::*;

fn queue_of(texts: &[&str], max_items: usize) -> Queue {
	let mut queue = Queue::new(max_items, 1000);
	for text in texts {
		queue.push(text.to_string()).unwrap();
	}
	queue
}

fn texts(queue: &Queue) -> Vec<&str> {
	queue.items.iter().map(|item| item.text.as_str()).collect()
}

#[test]
fn test_push_reports_the_sequence_and_what_it_dropped() {
	let mut queue = Queue::new(2, 1000);
	assert_eq!(queue.push("a".into()).unwrap(), (1, vec![]));
	assert_eq!(queue.push("b".into()).unwrap(), (2, vec![]));
	assert_eq!(queue.push("c".into()).unwrap(), (3, vec![1]));
	assert_eq!(texts(&queue), ["b", "c"]);
}

#[test]
fn test_push_stops_at_max_seq() {
	let mut queue = Queue::new(10, 2);
	queue.push("a".into()).unwrap();
	queue.push("b".into()).unwrap();
	let err = queue.push("c".into()).unwrap_err();
	assert!(matches!(err, Error::Io(_)));
	assert_eq!(texts(&queue), ["a", "b"]);
	assert_eq!(queue.next_seq, 3);
}

#[test]
fn test_adopt_takes_the_next_seq_and_drops_nothing() {
	let mut queue = queue_of(&["a", "b"], 2);
	assert_eq!(queue.adopt("x".into()).unwrap(), 3);
	assert_eq!(queue.adopt("y".into()).unwrap(), 4);
	// Past max_items, until trimmed.
	assert_eq!(texts(&queue), ["a", "b", "x", "y"]);
	assert_eq!(queue.drop_oldest_beyond(2), vec![1, 2]);
	// A push trims as usual, adopted items included.
	assert_eq!(queue.push("z".into()).unwrap(), (5, vec![3]));
	assert_eq!(texts(&queue), ["y", "z"]);
}

#[test]
fn test_push_drops_everything_adopt_left_past_the_cap() {
	let mut queue = queue_of(&["a", "b"], 2);
	for text in ["x", "y", "z"] {
		queue.adopt(text.into()).unwrap();
	}
	let held = queue.fetch(10, 100).unwrap();
	// Every item past the cap is reported, not just one, so each can be deleted.
	assert_eq!(queue.push("new".into()).unwrap(), (6, vec![1, 2, 3, 4]));
	assert_eq!(texts(&queue), ["z", "new"]);
	assert_eq!(queue.remove(held), vec![5]);
}

#[test]
fn test_adopt_stops_at_max_seq() {
	let mut queue = Queue::new(10, 2);
	queue.adopt("a".into()).unwrap();
	queue.adopt("b".into()).unwrap();
	assert!(matches!(queue.adopt("c".into()), Err(Error::Io(_))));
	assert!(matches!(queue.push("c".into()), Err(Error::Io(_))));
	assert_eq!(texts(&queue), ["a", "b"]);
	assert_eq!(queue.next_seq, 3);
}

#[test]
fn test_restore_keeps_order_and_moves_next_seq_past_it() {
	let mut queue = Queue::new(10, 1000);
	queue.restore(4, "x".into());
	queue.restore(9, "y".into());
	assert_eq!(queue.next_seq, 10);
	assert_eq!(queue.push("z".into()).unwrap(), (10, vec![]));
	queue.skip_past(20);
	assert_eq!(queue.push("w".into()).unwrap(), (21, vec![]));
	queue.skip_past(5);
	assert_eq!(queue.next_seq, 22);
}

#[test]
fn test_drop_oldest_beyond_trims_and_forgets() {
	let mut queue = queue_of(&["a", "b", "c", "d"], 10);
	let held = queue.fetch(2, 100).unwrap();
	assert_eq!(queue.drop_oldest_beyond(3), vec![1]);
	assert_eq!(queue.drop_oldest_beyond(3), Vec::<u64>::new());
	assert_eq!(queue.drop_oldest_beyond(1), vec![2, 3]);
	assert_eq!(texts(&queue), ["d"]);
	assert_eq!(queue.ledger.lock().pending_count(), 0);
	assert!(queue.remove(held).is_empty());
	assert_eq!(texts(&queue), ["d"]);
}

#[test]
fn test_remove_returns_what_it_deleted() {
	let mut queue = queue_of(&["a", "b", "c", "d"], 10);
	let first = queue.fetch(2, 100).unwrap();
	let second = queue.fetch(2, 100).unwrap();
	assert_eq!(queue.remove(second), vec![3, 4]);
	assert_eq!(texts(&queue), ["a", "b"]);
	assert!(!queue.has_data());
	drop(first);
	assert!(queue.has_data());
}

#[test]
fn test_reset_keeps_counting() {
	let mut queue = queue_of(&["a", "b"], 10);
	queue.reset();
	assert!(queue.items.is_empty() && !queue.has_data());
	assert_eq!(queue.push("c".into()).unwrap(), (3, vec![]));
}
