//! The database schema: upgrading from the old format, and records that do not have the
//! wrapper shape.

use super::support::*;
use super::*;
use web_sys::{IdbOpenDbRequest, IdbRequest, IdbTransactionMode};

fn factory() -> web_sys::IdbFactory {
	web_sys::window().unwrap().indexed_db().unwrap().unwrap()
}

fn indexed_db_available() -> bool {
	let available = web_sys::window()
		.and_then(|w| w.indexed_db().ok().flatten())
		.is_some();
	if !available {
		web_sys::console::log_1(&"Skipping schema test - no IndexedDB".into());
	}
	available
}

/// Deletes `db_name`, so the test starts from no database at all.
async fn delete_database(db_name: &str) {
	let request = factory().delete_database(db_name).unwrap();
	idb::await_request::<JsValue>(&request).await.unwrap();
}

/// Creates `db_name` at version 1 exactly as the old WebStore did: an auto-incremented
/// in-line `_idb_key` key path. Returns the open connection.
async fn create_version_1(db_name: &str) -> IdbDatabase {
	let request: IdbOpenDbRequest = factory().open_with_u32(db_name, 1).unwrap();
	let on_upgrade = Closure::once(move |event: web_sys::IdbVersionChangeEvent| {
		let request: IdbRequest = event.target().unwrap().unchecked_into();
		let db: IdbDatabase = request.result().unwrap().unchecked_into();
		let params = web_sys::IdbObjectStoreParameters::new();
		params.set_auto_increment(true);
		params.set_key_path(&JsValue::from_str("_idb_key"));
		db.create_object_store_with_optional_parameters(idb::STORE_NAME, &params)
			.unwrap();
	});
	request.set_onupgradeneeded(Some(on_upgrade.as_ref().unchecked_ref()));
	on_upgrade.forget();
	idb::await_request::<IdbDatabase>(&request).await.unwrap()
}

/// Adds raw JSON `records` to `db`'s object store and waits for them to commit.
async fn add_raw(db: &IdbDatabase, records: &[Value]) {
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

#[wasm_bindgen_test]
async fn test_version_1_database_is_replaced_empty() {
	let db_name = "test-schema-upgrade-v1";
	if !indexed_db_available() {
		return;
	}
	delete_database(db_name).await;
	let old = create_version_1(db_name).await;
	add_raw(&old, &[json!({"event": "old"}), json!({"event": "older"})]).await;
	assert_eq!(idb::read_all(&old).await.unwrap().len(), 2);
	// An open version 1 connection would block the upgrade.
	old.close();

	let mut store = WebStore::new(config(db_name, 4096, with_source(KEY))).await;
	assert_eq!(store.persistence_state(), PersistenceState::Persisted);
	assert!(!store.has_data());
	assert_eq!(store.next_key, 0);

	let db = store.db.clone().unwrap();
	assert_eq!(db.version(), f64::from(idb::DB_VERSION));
	assert!(idb::read_all(&db).await.unwrap().is_empty());
	let transaction = db.transaction_with_str(idb::STORE_NAME).unwrap();
	let object_store = transaction.object_store(idb::STORE_NAME).unwrap();
	assert_eq!(object_store.key_path().unwrap(), JsValue::from_str("key"));
	assert!(!object_store.auto_increment());

	let items = vec![json!("bare string"), json!({"event": "new"}), json!(7)];
	for item in &items {
		store.append(item.clone()).unwrap();
	}
	settle().await;
	assert_eq!(hydrated(db_name, 4096).await, items);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_new_database_is_created_at_current_version() {
	let db_name = "test-schema-new";
	if !indexed_db_available() {
		return;
	}
	delete_database(db_name).await;
	let store = WebStore::new(config(db_name, 4096, with_source(KEY))).await;
	if !store.is_persisted() {
		web_sys::console::log_1(&"Skipping schema test - no persistence".into());
		return;
	}
	assert!(!store.has_data());
	assert_eq!(
		store.db.as_ref().unwrap().version(),
		f64::from(idb::DB_VERSION)
	);
}

#[wasm_bindgen_test]
async fn test_records_without_wrapper_shape_are_skipped() {
	let db_name = "test-schema-foreign-records";
	let store = fresh_store(db_name, 4096).await;
	if !store.is_persisted() {
		web_sys::console::log_1(&"Skipping schema test - no persistence".into());
		return;
	}
	settle().await;
	let db = store.db.clone().unwrap();
	add_raw(
		&db,
		&[
			json!({"key": 3, "value": "ok"}),
			json!({"key": 10}),
			json!({"key": 12.5, "value": "fractional key"}),
			json!({"key": "text", "value": "string key"}),
			json!({"key": 4, "value": null}),
		],
	)
	.await;

	let mut reopened = WebStore::new(config(db_name, 4096, with_source(KEY))).await;
	assert!(reopened.is_persisted());
	assert_eq!(queued(&reopened), vec![json!("ok"), json!(null)]);
	// Key 10 has no item, but a new item must not take it.
	assert_eq!(reopened.next_key, 11);

	reopened.append(json!("new")).unwrap();
	assert_eq!(reopened.items.back().unwrap().key, 11);
	settle().await;
	assert_eq!(
		hydrated(db_name, 4096).await,
		vec![json!("ok"), json!(null), json!("new")]
	);

	// reset clears the skipped records too.
	reopened.reset();
	settle().await;
	assert!(idb::read_all(&db).await.unwrap().is_empty());
}

/// Keys never wrap: the last key is `u32::MAX - 1`, after which `append` fails and
/// changes nothing, so no new item can take a key already in use.
#[wasm_bindgen_test]
async fn test_keys_stop_before_wrapping() {
	let db_name = "test-schema-key-limit";
	let mut store = fresh_store(db_name, 4096).await;
	if !store.is_persisted() {
		web_sys::console::log_1(&"Skipping schema test - no persistence".into());
		return;
	}
	store.next_key = u32::MAX - 1;
	store.append(json!("last")).unwrap();
	assert_eq!(store.items.back().unwrap().key, u32::MAX - 1);
	for _ in 0..3 {
		let err = store.append(json!("one too many")).unwrap_err();
		assert!(err.to_string().contains("keys exhausted"), "{err}");
	}
	assert_eq!(queued(&store), vec![json!("last")]);
	settle().await;
	assert_eq!(hydrated(db_name, 4096).await, vec![json!("last")]);
	let result = store.fetch(None, None).unwrap().unwrap();
	assert_eq!(checked_batch(&result, 4096), vec![json!("last")]);
	store.reset();
	settle().await;
}

/// A record already at key `u32::MAX` leaves no key to give: the reopened store
/// delivers it and refuses new items rather than reuse a key.
#[wasm_bindgen_test]
async fn test_record_at_the_last_key_is_kept_and_never_reused() {
	let db_name = "test-schema-last-key";
	let store = fresh_store(db_name, 4096).await;
	if !store.is_persisted() {
		web_sys::console::log_1(&"Skipping schema test - no persistence".into());
		return;
	}
	settle().await;
	let db = store.db.clone().unwrap();
	add_raw(&db, &[json!({"key": u32::MAX, "value": "at the end"})]).await;

	let mut reopened = WebStore::new(config(db_name, 4096, with_source(KEY))).await;
	assert_eq!(queued(&reopened), vec![json!("at the end")]);
	assert_eq!(reopened.next_key, u32::MAX);
	assert!(reopened.append(json!("new")).is_err());
	assert_eq!(queued(&reopened), vec![json!("at the end")]);
	settle().await;
	assert_eq!(
		idb::read_all(&db).await.unwrap(),
		vec![json!({"key": u32::MAX, "value": "at the end"})]
	);

	let result = reopened.fetch(None, None).unwrap().unwrap();
	assert_eq!(checked_batch(&result, 4096), vec![json!("at the end")]);
	reopened.remove(&result.removable.unwrap()).unwrap();
	settle().await;
	assert!(idb::read_all(&db).await.unwrap().is_empty());
	// Once it is gone, a store opened again starts over.
	let mut again = WebStore::new(config(db_name, 4096, with_source(KEY))).await;
	assert_eq!(again.next_key, 0);
	again.append(json!("fresh")).unwrap();
	again.reset();
	settle().await;
}
