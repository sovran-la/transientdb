//! Adopting records that are not items: without the record shape, unreadable as JSON, or
//! under a tab key that is not a string. Each is discarded, deleted from IndexedDB and
//! logged; nothing else is touched, and an item's text is not parsed again.

use super::schema::add_raw;
use super::*;
use crate::item_samples::assert_exact;
use crate::serialize::{nested, Shape, MAX_ITEM_DEPTH};

/// A store on `db_name` with an empty database, to add raw records through.
async fn writer(db_name: &str) -> Option<WebStore> {
	persisted(db_name, 4096, 1000).await
}

/// Records without the record shape are discarded; records with it are kept, whatever
/// their text says, as an item's text was checked when it was appended.
#[wasm_bindgen_test]
async fn test_records_that_are_not_items_are_discarded() {
	let db_name = "test-adopt-not-items";
	let Some(store) = writer(db_name).await else {
		return;
	};
	let too_deep = nested(MAX_ITEM_DEPTH + 1, Shape::Arrays, json!(0)).to_string();
	let raw = |seq: Value, text: Value| json!({"tab": "gone", "seq": seq, "text": text});
	add_raw(
		&store.db(),
		&[
			raw(json!(1), json!(json!({"n": u64::MAX}).to_string())),
			raw(json!(2), json!(42)),
			raw(json!(3), json!({"event": "a JavaScript value"})),
			raw(json!(4), json!(null)),
			raw(json!(5), json!("not json")),
			raw(json!(6), json!("")),
			raw(json!(7), json!("{\"a\":1} trailing")),
			raw(json!(8), json!("42.0")),
			raw(json!(9), json!(too_deep)),
			json!({"tab": "gone", "seq": 10}),
			raw(json!(0), json!("\"zero is not a sequence number\"")),
			raw(json!(12.5), json!("\"fractional\"")),
			raw(json!(9_007_199_254_740_992u64), json!("\"past MAX_SEQ\"")),
			raw(json!("text key"), json!("\"string key\"")),
			raw(json!(30), json!("[1,")),
		],
	)
	.await;
	drop(store);

	warnings();
	let mut adopter = WebStore::new(config(db_name, 4096, 1000)).await;
	assert!(adopter.is_persisted());
	let kept: Vec<String> = [
		json!({"n": u64::MAX}).to_string(),
		"not json".to_string(),
		"{\"a\":1} trailing".to_string(),
		"42.0".to_string(),
		too_deep,
		"[1,".to_string(),
	]
	.into();
	let texts: Vec<String> = stored(&adopter).into_iter().map(|s| s.1).collect();
	assert_eq!(texts, kept);
	assert_eq!(
		warnings(),
		vec!["Discarding an IndexedDB record that is not a stored item"; 9]
	);
	// The JSON ones read back exactly.
	let fetched = adopter.fetch(1, 4096).unwrap().unwrap();
	assert_exact(&fetched.values(), &[json!({"n": u64::MAX})]);
	drop(fetched);
	// Everything discarded was deleted: only the adopter's records are left.
	assert_eq!(flushed(&adopter).await, stored(&adopter));
	assert_eq!(records(&adopter).await.len(), kept.len());
	adopter.reset();
	settle().await;
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

/// A record `{tab, seq, text}` whose `tab` and `text` are the JavaScript values given.
fn js_record(tab: &JsValue, seq: f64, text: &JsValue) -> js_sys::Object {
	let record = js_sys::Object::new();
	js_sys::Reflect::set(&record, &"tab".into(), tab).unwrap();
	js_sys::Reflect::set(&record, &"seq".into(), &seq.into()).unwrap();
	js_sys::Reflect::set(&record, &"text".into(), text).unwrap();
	record
}

/// A record whose value `JSON.stringify` rejects (a BigInt, a cycle) is not an item, but
/// its key is still read, so it is deleted. So is a record under a tab key that is not a
/// string, which no store writes and no lock guards.
#[wasm_bindgen_test]
async fn test_unreadable_records_and_non_string_tabs_are_discarded() {
	let db_name = "test-adopt-unreadable";
	let Some(store) = writer(db_name).await else {
		return;
	};
	let cyclic = js_sys::Object::new();
	js_sys::Reflect::set(&cyclic, &"me".into(), &cyclic).unwrap();
	add_raw(&store.db(), &[idb::record("gone", 3, "\"kept\"")]).await;
	let gone = JsValue::from_str("gone");
	let text = JsValue::from_str("\"under a number\"");
	let array_tab: JsValue = js_sys::Array::of1(&"gone".into()).into();
	add_js(
		&store.db(),
		&[
			js_record(&gone, 5.0, &js_sys::BigInt::from(10).into()),
			js_record(&gone, 40.0, &cyclic),
			js_record(&JsValue::from_f64(7.0), 1.0, &text),
			js_record(&array_tab, 1.0, &text),
		],
	)
	.await;
	drop(store);

	warnings();
	let mut adopter = WebStore::new(config(db_name, 4096, 1000)).await;
	assert!(adopter.is_persisted());
	assert_eq!(queued(&adopter), vec![json!("kept")]);
	assert_eq!(
		warnings(),
		vec!["Discarding an IndexedDB record that is not a stored item"; 4]
	);
	// All four were deleted: the adopted item is the only record left.
	assert_eq!(flushed(&adopter).await, stored(&adopter));
	let transaction = adopter.db().transaction_with_str(idb::STORE_NAME).unwrap();
	let count = transaction
		.object_store(idb::STORE_NAME)
		.unwrap()
		.count()
		.unwrap();
	assert_eq!(idb::await_request::<JsValue>(&count).await.unwrap(), 1);
	adopter.reset();
	settle().await;
}
