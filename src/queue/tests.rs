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

/// Items under the numbers `seqs`, reserved before, with texts `texts`.
fn reserved(seqs: &[u64], texts: &[&str]) -> Vec<Stored> {
	let pairs = seqs.iter().zip(texts);
	pairs
		.map(|(&seq, text)| Stored {
			seq,
			text: text.to_string(),
		})
		.collect()
}

#[test]
fn test_reserve_takes_the_next_seq_and_queues_nothing() {
	let mut queue = queue_of(&["a"], 10);
	assert_eq!(queue.reserve().unwrap(), 2);
	assert_eq!(queue.reserve().unwrap(), 3);
	assert_eq!(texts(&queue), ["a"]);
	assert!(queue
		.fetch(10, 100)
		.is_some_and(|items| items.values().len() == 1));
	// Pushes go on after the reserved numbers.
	assert_eq!(queue.push("b".into()).unwrap(), (4, vec![]));
}

#[test]
fn test_reserve_stops_at_max_seq() {
	let mut queue = Queue::new(10, 2);
	queue.push("a".into()).unwrap();
	assert_eq!(queue.reserve().unwrap(), 2);
	assert!(matches!(queue.reserve(), Err(Error::Io(_))));
	assert!(matches!(queue.push("c".into()), Err(Error::Io(_))));
	assert_eq!(texts(&queue), ["a"]);
	assert_eq!(queue.next_seq, 3);
}

#[test]
fn test_insert_queues_in_sequence_order_and_drops_nothing() {
	let mut queue = queue_of(&["a", "b"], 2);
	let seqs = [queue.reserve().unwrap(), queue.reserve().unwrap()];
	// Pushed after the reservation: behind the inserted items once they are in.
	assert_eq!(queue.push("c".into()).unwrap(), (5, vec![1]));
	queue.insert(reserved(&seqs, &["x", "y"]));
	// Past max_items, until trimmed.
	assert_eq!(texts(&queue), ["b", "x", "y", "c"]);
	let all: Vec<u64> = queue.items.iter().map(|item| item.seq).collect();
	assert_eq!(all, [2, 3, 4, 5]);
	assert_eq!(queue.drop_oldest_beyond(2), vec![2, 3]);
	// A push trims as usual, inserted items included.
	assert_eq!(queue.push("z".into()).unwrap(), (6, vec![4]));
	assert_eq!(texts(&queue), ["c", "z"]);
}

#[test]
fn test_inserted_items_can_be_fetched_and_removed() {
	let mut queue = queue_of(&["a"], 10);
	let held = queue.fetch(1, 100).unwrap();
	let seq = queue.reserve().unwrap();
	assert!(!queue.has_data(), "a reserved number is not an item");
	queue.insert(reserved(&[seq], &["x"]));
	assert!(queue.has_data());
	let x = queue.fetch(10, 100).unwrap();
	assert_eq!(x.values(), vec![serde_json::json!("x")]);
	assert_eq!(queue.remove(x), vec![2]);
	assert_eq!(queue.remove(held), vec![1]);
	assert!(queue.items.is_empty());
	queue.insert(Vec::new());
	assert!(queue.items.is_empty());
}

#[test]
fn test_push_drops_everything_insert_left_past_the_cap() {
	let mut queue = queue_of(&["a", "b"], 2);
	let seqs: Vec<u64> = (0..3).map(|_| queue.reserve().unwrap()).collect();
	queue.insert(reserved(&seqs, &["x", "y", "z"]));
	let held = queue.fetch(10, 100).unwrap();
	// Every item past the cap is reported, not just one, so each can be deleted.
	assert_eq!(queue.push("new".into()).unwrap(), (6, vec![1, 2, 3, 4]));
	assert_eq!(texts(&queue), ["z", "new"]);
	assert_eq!(queue.remove(held), vec![5]);
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
