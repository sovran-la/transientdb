//! Opening a store over what an earlier one stored: records that are not items, and items
//! larger than `max_item_size`, are discarded, deleted from IndexedDB and logged, and if
//! more items are stored than `max_items` allows, the oldest are dropped. Nothing else is
//! touched: an item's text is not parsed again.

use super::schema::add_raw;
use super::*;
use crate::item_samples::assert_exact;
use crate::serialize::{nested, Shape, MAX_ITEM_DEPTH};
use contract::{append_all, item_of_len, tagged};

/// Writes `items` through a roomy store on a fresh `db_name`, then waits for IndexedDB.
/// `None`, after logging, when IndexedDB is unavailable.
async fn written(db_name: &str, items: &[Value]) -> Option<()> {
	let mut store = persisted(db_name, 1 << 20, 1 << 20).await?;
	append_all(&mut store, items);
	settle().await;
	Some(())
}

/// Every record IndexedDB holds for `db_name`, read through a roomy store's connection.
async fn records(db_name: &str) -> Vec<Value> {
	let store = WebStore::new(config(db_name, 1 << 20, 1 << 20)).await;
	idb::read_all(store.db.as_ref().unwrap()).await.unwrap()
}

#[wasm_bindgen_test]
async fn test_lowered_max_items_drops_the_oldest_at_open() {
	let db_name = "test-hydrate-lowered-max-items";
	if written(db_name, &tagged(0..10)).await.is_none() {
		return;
	}
	let mut store = WebStore::new(config(db_name, 4096, 3)).await;
	assert_eq!(queued(&store), tagged(7..10));
	// The drop reached IndexedDB.
	assert_eq!(flushed(&store).await, stored(&store));
	assert_eq!(hydrated(db_name).await, tagged(7..10));
	// From then on the store holds at most 3, as usual.
	store.append(json!({"i": 10})).unwrap();
	assert_eq!(queued(&store), tagged(8..11));
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_raised_max_items_keeps_everything() {
	let db_name = "test-hydrate-raised-max-items";
	if written(db_name, &tagged(0..10)).await.is_none() {
		return;
	}
	let mut store = WebStore::new(config(db_name, 4096, 1000)).await;
	assert_eq!(queued(&store), tagged(0..10));
	append_all(&mut store, &tagged(10..20));
	assert_eq!(hydrated(db_name).await, tagged(0..20));
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_items_over_a_smaller_max_item_size_are_discarded_at_open() {
	let db_name = "test-hydrate-smaller-max-item-size";
	let items = [
		item_of_len(100, 0),
		json!("a"),
		item_of_len(51, 1),
		item_of_len(50, 2),
		item_of_len(500, 3),
		json!("b"),
	];
	if written(db_name, &items).await.is_none() {
		return;
	}
	let mut store = WebStore::new(config(db_name, 50, 1000)).await;
	let kept = vec![json!("a"), item_of_len(50, 2), json!("b")];
	assert_eq!(queued(&store), kept);
	assert_eq!(flushed(&store).await, stored(&store));
	assert_eq!(hydrated(db_name).await, kept);
	// Every item left can be fetched under the new limit.
	let all = store.fetch(10, 1000).unwrap().unwrap();
	assert_eq!(all.values(), kept);
	drop(all);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_both_limits_lowered_at_once() {
	let db_name = "test-hydrate-both-lowered";
	let items: Vec<Value> = (0..8)
		.map(|i| item_of_len(if i % 2 == 0 { 40 } else { 10 }, i))
		.collect();
	if written(db_name, &items).await.is_none() {
		return;
	}
	// The oversized ones go first, then the oldest of the rest beyond max_items.
	let store = WebStore::new(config(db_name, 20, 2)).await;
	assert_eq!(queued(&store), vec![items[5].clone(), items[7].clone()]);
	assert_eq!(
		hydrated(db_name).await,
		vec![items[5].clone(), items[7].clone()]
	);
	let mut store = store;
	store.reset();
	settle().await;
}

/// Records without the record shape are discarded; records with it are kept, whatever
/// their text, as an item's text was checked when it was appended.
#[wasm_bindgen_test]
async fn test_records_that_are_not_items_are_discarded_and_their_seqs_never_reused() {
	let db_name = "test-hydrate-not-items";
	let Some(store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	settle().await;
	let db = store.db.clone().unwrap();
	let too_deep = nested(MAX_ITEM_DEPTH + 1, Shape::Arrays, json!(0)).to_string();
	add_raw(
		&db,
		&[
			json!({"seq": 1, "text": json!({"n": u64::MAX}).to_string()}),
			json!({"seq": 2, "text": 42}),
			json!({"seq": 3, "text": {"event": "a JavaScript value"}}),
			json!({"seq": 4, "text": null}),
			json!({"seq": 5, "text": "not json"}),
			json!({"seq": 6, "text": ""}),
			json!({"seq": 7, "text": "{\"a\":1} trailing"}),
			json!({"seq": 8, "text": "42.0"}),
			json!({"seq": 9, "text": too_deep}),
			json!({"seq": 10}),
			json!({"seq": 0, "text": "\"zero is not a sequence number\""}),
			json!({"seq": 12.5, "text": "\"fractional\""}),
			json!({"seq": 9_007_199_254_740_992u64, "text": "\"past MAX_SEQ\""}),
			json!({"seq": "text key", "text": "\"string key\""}),
			json!({"seq": 30, "text": "[1,"}),
		],
	)
	.await;

	let mut reopened = WebStore::new(config(db_name, 4096, 1000)).await;
	assert!(reopened.is_persisted());
	let kept: Vec<(u64, String)> = [
		(1, json!({"n": u64::MAX}).to_string()),
		(5, "not json".to_string()),
		(6, String::new()),
		(7, "{\"a\":1} trailing".to_string()),
		(8, "42.0".to_string()),
		(9, too_deep),
		(30, "[1,".to_string()),
	]
	.into();
	assert_eq!(stored(&reopened), kept);
	// The JSON ones read back exactly.
	let fetched = reopened.fetch(1, 4096).unwrap().unwrap();
	assert_exact(&fetched.values(), &[json!({"n": u64::MAX})]);
	drop(fetched);
	// A discarded record's sequence number still counts: 30 is the highest kept, and 31
	// on is never given out to an existing record.
	assert_eq!(reopened.queue.next_seq, 31);
	// Everything discarded was deleted.
	assert_eq!(flushed(&reopened).await, stored(&reopened));
	assert_eq!(records(db_name).await.len(), kept.len());

	reopened.append(json!("new")).unwrap();
	assert_eq!(reopened.queue.items.back().unwrap().seq, 31);
	settle().await;
	let again = WebStore::new(config(db_name, 4096, 1000)).await;
	let new = (31, json!("new").to_string());
	assert_eq!(stored(&again), [kept, vec![new]].concat());
	reopened.reset();
	settle().await;
	assert!(idb::read_all(&db).await.unwrap().is_empty());
}

/// Adds `records`, JavaScript values as another writer could store them, to `db`.
async fn add_js(db: &IdbDatabase, records: &[js_sys::Object]) {
	let transaction = db
		.transaction_with_str_and_mode(idb::STORE_NAME, web_sys::IdbTransactionMode::Readwrite)
		.unwrap();
	let store = transaction.object_store(idb::STORE_NAME).unwrap();
	for record in records {
		store.add(record).unwrap();
	}
	idb::completion(&transaction).await.unwrap();
}

/// A record `{seq, text}` whose `text` is `text`.
fn js_record(seq: f64, text: &JsValue) -> js_sys::Object {
	let record = js_sys::Object::new();
	js_sys::Reflect::set(&record, &"seq".into(), &seq.into()).unwrap();
	js_sys::Reflect::set(&record, &"text".into(), text).unwrap();
	record
}

/// A record whose value `JSON.stringify` rejects (a BigInt, a cycle) is not an item, but
/// its key is still read: its sequence number counts, and it is deleted.
#[wasm_bindgen_test]
async fn test_records_that_cannot_be_read_as_json_still_count_and_are_discarded() {
	let db_name = "test-hydrate-unreadable";
	let Some(store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	settle().await;
	let db = store.db.clone().unwrap();
	let cyclic = js_sys::Object::new();
	js_sys::Reflect::set(&cyclic, &"me".into(), &cyclic).unwrap();
	add_raw(&db, &[idb::record(3, "\"kept\"")]).await;
	add_js(
		&db,
		&[
			js_record(5.0, &js_sys::BigInt::from(10).into()),
			js_record(40.0, &cyclic),
		],
	)
	.await;

	let mut reopened = WebStore::new(config(db_name, 4096, 1000)).await;
	assert!(reopened.is_persisted());
	assert_eq!(queued(&reopened), vec![json!("kept")]);
	// 40 is never given out again.
	assert_eq!(reopened.queue.next_seq, 41);
	// Both were deleted: the kept item is the only record left.
	assert_eq!(flushed(&reopened).await, stored(&reopened));
	let transaction = db.transaction_with_str(idb::STORE_NAME).unwrap();
	let count = transaction
		.object_store(idb::STORE_NAME)
		.unwrap()
		.count()
		.unwrap();
	assert_eq!(idb::await_request::<JsValue>(&count).await.unwrap(), 1);

	reopened.append(json!("new")).unwrap();
	assert_eq!(reopened.queue.items.back().unwrap().seq, 41);
	reopened.reset();
	settle().await;
}
