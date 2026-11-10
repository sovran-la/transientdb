use super::*;
use crate::item_samples::{assert_exact, awkward_items};
use serde_json::json;

fn items_of(values: &[Value]) -> (Ledger, Items) {
	let ledger = Ledger::new();
	let entries: Vec<(u64, String)> = values
		.iter()
		.enumerate()
		.map(|(i, v)| (i as u64 + 1, v.to_string()))
		.collect();
	{
		let mut claims = ledger.lock();
		for (seq, _) in &entries {
			claims.claim(*seq);
		}
	}
	let epoch = ledger.lock().epoch();
	let items = Items::new(ledger.clone(), epoch, entries);
	(ledger, items)
}

#[test]
fn test_len_byte_len_and_views() {
	let values = vec![json!({"a": 1}), json!("two"), json!([3])];
	let (_, items) = items_of(&values);
	assert_eq!(items.len(), 3);
	assert_eq!(items.byte_len(), 7 + 5 + 3);
	assert_eq!(
		items.iter().collect::<Vec<_>>(),
		[r#"{"a":1}"#, r#""two""#, "[3]"]
	);
	assert_eq!(items.json(), r#"[{"a":1},"two",[3]]"#);
	assert_eq!(items.json().len(), items.byte_len() + 2 + 2);
	assert_eq!(items.values(), values);
}

#[test]
fn test_views_of_awkward_items_are_exact() {
	let values = awkward_items();
	let (_, items) = items_of(&values);
	assert_exact(&items.values(), &values);
	let parsed: Vec<Value> = serde_json::from_str(&items.json()).unwrap();
	assert_exact(&parsed, &values);
	assert_eq!(items.json().len(), items.byte_len() + values.len() - 1 + 2);
}

#[test]
#[should_panic(expected = "an Items is never empty")]
fn test_an_empty_items_cannot_be_made() {
	let _ = Items::new(Ledger::new(), 0, Vec::new());
}

#[test]
fn test_drop_and_release_free_the_claims() {
	let (ledger, items) = items_of(&[json!(1), json!(2)]);
	assert_eq!(ledger.lock().pending_count(), 2);
	drop(items);
	assert_eq!(ledger.lock().pending_count(), 0);

	let (ledger, items) = items_of(&[json!(1)]);
	items.release();
	assert_eq!(ledger.lock().pending_count(), 0);
}

#[test]
fn test_drop_after_the_ledger_was_reset_frees_nothing_new() {
	let (ledger, items) = items_of(&[json!(1), json!(2)]);
	ledger.lock().reset();
	ledger.lock().claim(1);
	drop(items);
	assert!(ledger.lock().is_pending(1));
}

#[test]
fn test_surrender_disarms_only_for_its_own_ledger() {
	let (ledger, items) = items_of(&[json!(1), json!(2)]);
	let (epoch, seqs) = items.surrender(&ledger).unwrap();
	assert_eq!((epoch, seqs), (0, vec![1, 2]));
	// Disarmed: dropping it released nothing.
	assert_eq!(ledger.lock().pending_count(), 2);

	let (ledger, items) = items_of(&[json!(1)]);
	assert!(items.surrender(&Ledger::new()).is_none());
	// Not ours: it went back to its own ledger.
	assert_eq!(ledger.lock().pending_count(), 0);
}

#[test]
fn test_drop_outliving_every_store_reference_is_harmless() {
	let (ledger, items) = items_of(&[json!(1)]);
	drop(ledger);
	drop(items);
}

#[test]
fn test_debug_shows_the_items() {
	let (_, items) = items_of(&[json!({"a": 1}), json!(2)]);
	assert_eq!(
		format!("{items:?}"),
		r#"Items { len: 2, byte_len: 8, items: ["{\"a\":1}", "2"] }"#
	);
}

#[test]
fn test_items_is_send_and_sync() {
	fn assert_send_sync<T: Send + Sync>() {}
	assert_send_sync::<Items>();
}
