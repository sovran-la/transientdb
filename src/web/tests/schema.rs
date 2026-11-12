//! The database schema: every earlier version is replaced empty, and the store's
//! sequence numbers stay exact as IndexedDB keys.

use super::*;
use crate::Error;
use contract::{append_all, tagged};
use web_sys::{IdbOpenDbRequest, IdbRequest, IdbTransactionMode};

pub(super) fn factory() -> web_sys::IdbFactory {
	web_sys::window().unwrap().indexed_db().unwrap().unwrap()
}

/// Deletes `db_name`, so the test starts from no database at all.
pub(super) async fn delete_database(db_name: &str) {
	let request = factory().delete_database(db_name).unwrap();
	idb::await_request::<JsValue>(&request).await.unwrap();
}

/// Creates `db_name` at `version` with one object store as an earlier WebStore made it,
/// holding `records`, and closes it (an open connection would block the upgrade).
async fn create_old_version(
	db_name: &str,
	version: u32,
	key_path: &'static str,
	auto_increment: bool,
	records: &[Value],
) {
	delete_database(db_name).await;
	let request: IdbOpenDbRequest = factory().open_with_u32(db_name, version).unwrap();
	let on_upgrade = Closure::once(move |event: web_sys::IdbVersionChangeEvent| {
		let request: IdbRequest = event.target().unwrap().unchecked_into();
		let db: IdbDatabase = request.result().unwrap().unchecked_into();
		let params = web_sys::IdbObjectStoreParameters::new();
		params.set_auto_increment(auto_increment);
		params.set_key_path(&JsValue::from_str(key_path));
		db.create_object_store_with_optional_parameters(idb::STORE_NAME, &params)
			.unwrap();
	});
	request.set_onupgradeneeded(Some(on_upgrade.as_ref().unchecked_ref()));
	on_upgrade.forget();
	let old = idb::await_request::<IdbDatabase>(&request).await.unwrap();
	add_raw(&old, records).await;
	assert_eq!(idb::read_all(&old).await.unwrap().len(), records.len());
	old.close();
}

/// Adds raw JSON `records` to `db`'s object store and waits for them to commit.
pub(super) async fn add_raw(db: &IdbDatabase, records: &[Value]) {
	let transaction = db
		.transaction_with_str_and_mode(idb::STORE_NAME, IdbTransactionMode::Readwrite)
		.unwrap();
	let store = transaction.object_store(idb::STORE_NAME).unwrap();
	for record in records {
		let js = js_sys::JSON::parse(&record.to_string()).unwrap();
		store.add(&js).unwrap();
	}
	idb::completion(&transaction).await.unwrap();
}

/// Opens `db_name` after an old version was written to it: the store must start empty
/// at the current version, with the current schema, and work normally from there.
async fn assert_replaced_empty(db_name: &str) {
	let mut store = WebStore::new(config(db_name, 4096, 1000)).await;
	assert_eq!(store.persistence_state(), PersistenceState::Persisted);
	assert!(!store.has_data());
	assert_eq!(store.queue.next_seq, 1);

	let db = store.db.clone().unwrap();
	assert_eq!(db.version(), f64::from(idb::DB_VERSION));
	assert!(idb::read_all(&db).await.unwrap().is_empty());
	let names = db.object_store_names();
	assert_eq!(
		(names.length(), names.item(0)),
		(1, Some(idb::STORE_NAME.into()))
	);
	let transaction = db.transaction_with_str(idb::STORE_NAME).unwrap();
	let object_store = transaction.object_store(idb::STORE_NAME).unwrap();
	assert_eq!(object_store.key_path().unwrap(), JsValue::from_str("seq"));
	assert!(!object_store.auto_increment());

	let items = vec![json!("bare string"), json!({"event": "new"}), json!(7)];
	append_all(&mut store, &items);
	settle().await;
	assert_eq!(hydrated(db_name).await, items);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_version_1_database_is_replaced_empty() {
	let db_name = "test-schema-upgrade-v1";
	let records = [json!({"event": "old"}), json!({"event": "older"})];
	create_old_version(db_name, 1, "_idb_key", true, &records).await;
	assert_replaced_empty(db_name).await;
}

#[wasm_bindgen_test]
async fn test_version_2_database_is_replaced_empty() {
	let db_name = "test-schema-upgrade-v2";
	let records = [
		json!({"key": 0, "value": "old"}),
		json!({"key": 1, "value": "older"}),
	];
	create_old_version(db_name, 2, "key", false, &records).await;
	assert_replaced_empty(db_name).await;
}

#[wasm_bindgen_test]
async fn test_version_3_database_is_replaced_empty() {
	let db_name = "test-schema-upgrade-v3";
	let records = [
		json!({"key": 0, "value": {"event": "old"}, "batch": 0}),
		json!({"key": 1, "value": "older", "batch": 2}),
		json!({"key": "boundary", "batch": 3}),
	];
	create_old_version(db_name, 3, "key", false, &records).await;
	assert_replaced_empty(db_name).await;
}

#[wasm_bindgen_test]
async fn test_version_4_database_is_replaced_empty() {
	let db_name = "test-schema-upgrade-v4";
	let records = [
		json!({"key": 0, "value": "{\"event\":\"old\"}", "batch": 0}),
		json!({"key": 1, "value": "42.0", "batch": 1}),
		json!({"key": "boundary", "batch": 2}),
	];
	create_old_version(db_name, 4, "key", false, &records).await;
	assert_replaced_empty(db_name).await;
}

#[wasm_bindgen_test]
async fn test_new_database_is_created_at_current_version() {
	let db_name = "test-schema-new";
	delete_database(db_name).await;
	let store = WebStore::new(config(db_name, 4096, 1000)).await;
	assert!(store.is_persisted());
	assert!(!store.has_data());
	assert_eq!(
		store.db.as_ref().unwrap().version(),
		f64::from(idb::DB_VERSION)
	);
}

#[wasm_bindgen_test]
async fn test_records_are_seq_and_text_only() {
	let db_name = "test-schema-record";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	store.append(json!({"a": [1, 2.0]})).unwrap();
	let seq = store.queue.next_seq - 1;
	let _held = store.fetch(1, 4096).unwrap().unwrap();
	let records = idb::read_all(store.db.as_ref().unwrap()).await.unwrap();
	assert_eq!(
		records,
		vec![json!({"seq": seq, "text": "{\"a\":[1,2.0]}"})]
	);
	store.reset();
	settle().await;
}

/// Sequence numbers never wrap and never leave what an `f64` key holds exactly: the
/// last one is `MAX_SEQ`, after which `append` fails and changes nothing.
#[wasm_bindgen_test]
async fn test_sequence_numbers_stop_at_the_last_exact_key() {
	let db_name = "test-schema-seq-limit";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	store.queue.next_seq = idb::MAX_SEQ - 1;
	append_all(&mut store, &[json!("second to last"), json!("last")]);
	assert_eq!(store.queue.items.back().unwrap().seq, idb::MAX_SEQ);
	for _ in 0..3 {
		let err = store.append(json!("one too many")).unwrap_err();
		assert!(matches!(&err, Error::Io(e) if e.to_string() == "sequence numbers exhausted"));
	}
	assert_eq!(queued(&store), vec![json!("second to last"), json!("last")]);
	// The keys came back exactly: neither was rounded onto the other.
	assert_eq!(flushed(&store).await, stored(&store));
	let reopened = WebStore::new(config(db_name, 4096, 1000)).await;
	assert_eq!(stored(&reopened), stored(&store));
	assert_eq!(reopened.queue.next_seq, idb::MAX_SEQ + 1);
	let items = store.fetch(10, 4096).unwrap().unwrap();
	assert_eq!(items.values(), vec![json!("second to last"), json!("last")]);
	store.remove(items).unwrap();
	settle().await;
	assert!(hydrated(db_name).await.is_empty());
}

/// A record already at `MAX_SEQ` leaves no sequence number to give: the reopened store
/// hands it out and refuses new items rather than reuse one. Once it is gone, a store
/// opened again starts over.
#[wasm_bindgen_test]
async fn test_record_at_the_last_seq_is_kept_and_never_reused() {
	let db_name = "test-schema-last-seq";
	let Some(store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	settle().await;
	let db = store.db.clone().unwrap();
	add_raw(&db, &[idb::record(idb::MAX_SEQ, "\"at the end\"")]).await;

	let mut reopened = WebStore::new(config(db_name, 4096, 1000)).await;
	assert_eq!(queued(&reopened), vec![json!("at the end")]);
	assert!(reopened.append(json!("new")).is_err());
	let items = reopened.fetch(10, 4096).unwrap().unwrap();
	assert_eq!(items.values(), vec![json!("at the end")]);
	reopened.remove(items).unwrap();
	settle().await;
	assert!(idb::read_all(&db).await.unwrap().is_empty());

	let mut again = WebStore::new(config(db_name, 4096, 1000)).await;
	assert_eq!(again.queue.next_seq, 1);
	again.append(json!("fresh")).unwrap();
	again.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_sequence_numbers_continue_after_reset_and_reload() {
	let db_name = "test-schema-seq-reset";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let start = store.queue.next_seq;
	append_all(&mut store, &tagged(0..3));
	store.reset();
	append_all(&mut store, &tagged(3..4));
	assert_eq!(stored(&store)[0].0, start + 3);
	settle().await;
	let reopened = WebStore::new(config(db_name, 4096, 1000)).await;
	assert_eq!(reopened.queue.next_seq, start + 4);
	store.reset();
	settle().await;
}
