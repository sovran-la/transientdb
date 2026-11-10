//! Items come back from IndexedDB exactly as they were appended: each record holds its
//! item as JSON text, so numbers a JavaScript number cannot hold, and floats with no
//! fractional part, survive a reload.

use super::schema::{add_raw, delete_database, factory, indexed_db_available, text};
use super::support::*;
use super::*;
use crate::item_samples::{assert_exact, awkward_items};
use web_sys::{IdbOpenDbRequest, IdbRequest};

/// Creates `db_name` at version 3 exactly as the previous WebStore did: wrapper records
/// keyed by `key`, each holding its item as a JavaScript value. Returns the open
/// connection.
async fn create_version_3(db_name: &str) -> IdbDatabase {
	let request: IdbOpenDbRequest = factory().open_with_u32(db_name, 3).unwrap();
	let on_upgrade = Closure::once(move |event: web_sys::IdbVersionChangeEvent| {
		let request: IdbRequest = event.target().unwrap().unchecked_into();
		let db: IdbDatabase = request.result().unwrap().unchecked_into();
		let params = web_sys::IdbObjectStoreParameters::new();
		params.set_key_path(&JsValue::from_str("key"));
		db.create_object_store_with_optional_parameters(idb::STORE_NAME, &params)
			.unwrap();
	});
	request.set_onupgradeneeded(Some(on_upgrade.as_ref().unchecked_ref()));
	on_upgrade.forget();
	idb::await_request::<IdbDatabase>(&request).await.unwrap()
}

#[wasm_bindgen_test]
async fn test_version_3_database_is_replaced_empty() {
	let db_name = "test-exact-items-upgrade-v3";
	if !indexed_db_available() {
		return;
	}
	delete_database(db_name).await;
	let old = create_version_3(db_name).await;
	add_raw(
		&old,
		&[
			json!({"key": 0, "value": {"event": "old"}, "batch": 0}),
			json!({"key": 1, "value": "older", "batch": 2}),
			json!({"key": "boundary", "batch": 3}),
		],
	)
	.await;
	assert_eq!(idb::read_all(&old).await.unwrap().len(), 3);
	// An open version 3 connection would block the upgrade.
	old.close();

	let mut store = WebStore::new(config(db_name, 4096, with_source(KEY))).await;
	assert_eq!(store.persistence_state(), PersistenceState::Persisted);
	assert!(!store.has_data());
	assert_eq!((store.next_key, store.current_batch), (0, 0));
	let db = store.db.clone().unwrap();
	assert_eq!(db.version(), f64::from(idb::DB_VERSION));
	assert!(idb::read_all(&db).await.unwrap().is_empty());

	let items = awkward_items();
	for item in &items {
		store.append(item.clone()).unwrap();
	}
	store.end_batch().unwrap();
	store.append(json!("next batch")).unwrap();
	settle().await;
	let mut reopened = WebStore::new(config(db_name, 4096, with_source(KEY))).await;
	let first = reopened.fetch(None, None).unwrap().unwrap();
	assert_exact(&checked_batch(&first, 4096), &items);
	reopened.remove(&first.removable.unwrap()).unwrap();
	let second = reopened.fetch(None, None).unwrap().unwrap();
	assert_eq!(checked_batch(&second, 4096), vec![json!("next batch")]);
	reopened.remove(&second.removable.unwrap()).unwrap();
	assert!(!reopened.has_data());
	settle().await;
	assert!(hydrated(db_name, 4096).await.is_empty());
}

#[wasm_bindgen_test]
async fn test_records_whose_value_is_not_json_text_are_skipped() {
	let db_name = "test-exact-items-malformed-values";
	let store = fresh_store(db_name, 4096).await;
	if !store.is_persisted() {
		web_sys::console::log_1(&"Skipping exact items test - no persistence".into());
		return;
	}
	settle().await;
	let db = store.db.clone().unwrap();
	add_raw(
		&db,
		&[
			json!({"key": 0, "value": text(json!({"n": u64::MAX})), "batch": 0}),
			json!({"key": 1, "value": 42, "batch": 0}),
			json!({"key": 2, "value": {"event": "a JavaScript value"}, "batch": 0}),
			json!({"key": 3, "value": null, "batch": 0}),
			json!({"key": 4, "value": "not json", "batch": 0}),
			json!({"key": 5, "value": "", "batch": 0}),
			json!({"key": 6, "value": "{\"a\":1} trailing", "batch": 0}),
			json!({"key": 7, "value": text(json!(42.0)), "batch": 1}),
			json!({"key": 9, "value": "[1,", "batch": 1}),
		],
	)
	.await;

	let mut reopened = WebStore::new(config(db_name, 4096, with_source(KEY))).await;
	assert!(reopened.is_persisted());
	let valid = vec![json!({"n": u64::MAX}), json!(42.0)];
	assert_exact(&queued(&reopened), &valid);
	assert_eq!(
		reopened.items.iter().map(|e| e.key).collect::<Vec<_>>(),
		vec![0, 7]
	);
	// The skipped records' keys still count: key 9 is never given out.
	assert_eq!(reopened.next_key, 10);
	assert_eq!(reopened.current_batch, 1);

	reopened.append(json!("new")).unwrap();
	assert_eq!(reopened.items.back().unwrap().key, 10);
	settle().await;
	assert_exact(
		&hydrated(db_name, 4096).await,
		&[valid, vec![json!("new")]].concat(),
	);
	reopened.reset();
	settle().await;
	assert!(idb::read_all(&db).await.unwrap().is_empty());
}

#[wasm_bindgen_test]
async fn test_awkward_items_hydrate_exactly() {
	let db_name = "test-exact-items-round-trip";
	let mut store = fresh_store(db_name, 4096).await;
	if !store.is_persisted() {
		web_sys::console::log_1(&"Skipping exact items test - no persistence".into());
		return;
	}
	let items = awkward_items();
	for item in &items {
		store.append(item.clone()).unwrap();
	}
	settle().await;

	let mut reopened = WebStore::new(config(db_name, 4096, with_source(KEY))).await;
	assert!(reopened.is_persisted());
	assert_exact(&queued(&reopened), &items);
	let result = reopened.fetch(None, None).unwrap().unwrap();
	assert_exact(&checked_batch(&result, 4096), &items);
	reopened.reset();
	settle().await;
}
