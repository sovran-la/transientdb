//! `end_batch` boundaries persist: every record carries its batch number, and the
//! boundary record keeps one set after the last append. A reload alone is no boundary.

use super::schema::{add_raw, delete_database, factory, indexed_db_available};
use super::support::*;
use super::*;
use web_sys::{IdbOpenDbRequest, IdbRequest};

fn drained(store: &mut WebStore) -> Vec<Vec<Value>> {
	drain(store, Some(1000), None)
}

async fn reopen(db_name: &str) -> WebStore {
	WebStore::new(config(db_name, 4096, with_source(KEY))).await
}

/// A store emptied of earlier runs, or `None`, after logging, without persistence.
async fn persisted(db_name: &str) -> Option<WebStore> {
	let store = fresh_store(db_name, 4096).await;
	if !store.is_persisted() {
		web_sys::console::log_1(&"Skipping end_batch reload test - no persistence".into());
		return None;
	}
	Some(store)
}

#[wasm_bindgen_test]
async fn test_boundaries_survive_a_reopen() {
	let db_name = "test-end-batch-reload-survives";
	let Some(mut store) = persisted(db_name).await else {
		return;
	};
	let (a, b, c) = (json!("a"), json!("b"), json!("c"));
	store.append(a.clone()).unwrap();
	store.append(b.clone()).unwrap();
	store.end_batch().unwrap();
	store.append(c.clone()).unwrap();
	settle().await;

	let db = store.db.clone().unwrap();
	let batches: Vec<Option<u32>> = idb::read_all(&db)
		.await
		.unwrap()
		.iter()
		.filter(|r| !idb::is_boundary(r))
		.map(idb::record_batch)
		.collect();
	assert_eq!(batches, vec![Some(0), Some(0), Some(1)]);

	let mut reopened = reopen(db_name).await;
	assert_eq!(drained(&mut reopened), vec![vec![a, b], vec![c]]);
	settle().await;
	reopened.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_a_reload_alone_is_not_a_boundary() {
	let db_name = "test-end-batch-reload-not-boundary";
	let Some(mut store) = persisted(db_name).await else {
		return;
	};
	store.append(json!("a")).unwrap();
	settle().await;

	// Several reloads in a row add no boundary either.
	for _ in 0..3 {
		reopen(db_name).await;
	}
	let mut reopened = reopen(db_name).await;
	reopened.append(json!("b")).unwrap();
	assert_eq!(drained(&mut reopened), vec![vec![json!("a"), json!("b")]]);
	settle().await;
}

#[wasm_bindgen_test]
async fn test_a_boundary_after_the_last_append_survives_a_reopen() {
	let db_name = "test-end-batch-reload-trailing";
	let Some(mut store) = persisted(db_name).await else {
		return;
	};
	store.append(json!("a")).unwrap();
	store.end_batch().unwrap();
	// Repeated calls write nothing more: still one boundary.
	store.end_batch().unwrap();
	settle().await;
	let db = store.db.clone().unwrap();
	let boundaries: Vec<Value> = idb::read_all(&db)
		.await
		.unwrap()
		.into_iter()
		.filter(idb::is_boundary)
		.collect();
	assert_eq!(boundaries, vec![json!({"key": "boundary", "batch": 1})]);

	for _ in 0..2 {
		reopen(db_name).await;
	}
	let mut reopened = reopen(db_name).await;
	assert_eq!(reopened.current_batch, 1);
	reopened.append(json!("b")).unwrap();
	reopened.append(json!("c")).unwrap();
	assert_eq!(
		drained(&mut reopened),
		vec![vec![json!("a")], vec![json!("b"), json!("c")]]
	);
	settle().await;
	reopened.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_a_trailing_boundary_with_nothing_queued_changes_nothing_after_a_reopen() {
	let db_name = "test-end-batch-reload-trailing-empty";
	let Some(mut store) = persisted(db_name).await else {
		return;
	};
	store.append(json!("a")).unwrap();
	store.end_batch().unwrap();
	drain(&mut store, None, None);
	settle().await;

	let mut reopened = reopen(db_name).await;
	assert!(!reopened.has_data());
	reopened.end_batch().unwrap();
	reopened.append(json!("b")).unwrap();
	reopened.append(json!("c")).unwrap();
	assert_eq!(drained(&mut reopened), vec![vec![json!("b"), json!("c")]]);
	settle().await;
	reopened.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_reset_clears_the_boundary_record() {
	let db_name = "test-end-batch-reload-reset";
	let Some(mut store) = persisted(db_name).await else {
		return;
	};
	store.append(json!("a")).unwrap();
	store.end_batch().unwrap();
	store.reset();
	settle().await;
	let db = store.db.clone().unwrap();
	assert!(idb::read_all(&db).await.unwrap().is_empty());

	let mut reopened = reopen(db_name).await;
	reopened.append(json!("b")).unwrap();
	reopened.append(json!("c")).unwrap();
	assert_eq!(drained(&mut reopened), vec![vec![json!("b"), json!("c")]]);
	settle().await;
}

#[wasm_bindgen_test]
async fn test_records_without_a_valid_batch_number_are_skipped() {
	let db_name = "test-end-batch-reload-invalid";
	let Some(store) = persisted(db_name).await else {
		return;
	};
	settle().await;
	let db = store.db.clone().unwrap();
	add_raw(
		&db,
		&[
			json!({"key": 0, "value": "ok", "batch": 0}),
			json!({"key": 1, "value": "no batch"}),
			json!({"key": 2, "value": "negative", "batch": -1}),
			json!({"key": 3, "value": "fractional", "batch": 1.5}),
			json!({"key": 4, "value": "too large", "batch": 1u64 << 32}),
			json!({"key": 5, "value": "text", "batch": "1"}),
			json!({"key": 6, "value": "ok too", "batch": 2}),
			json!({"key": "boundary", "batch": "not a number"}),
		],
	)
	.await;

	let mut reopened = reopen(db_name).await;
	assert!(reopened.is_persisted());
	assert_eq!(queued(&reopened), vec![json!("ok"), json!("ok too")]);
	assert_eq!(reopened.next_key, 7);
	// Only valid numbers count: the invalid boundary record is ignored.
	assert_eq!(reopened.current_batch, 2);
	reopened.append(json!("new")).unwrap();
	assert_eq!(
		drained(&mut reopened),
		vec![vec![json!("ok")], vec![json!("ok too"), json!("new")]]
	);
	settle().await;
	reopened.reset();
	settle().await;
}

/// Creates `db_name` at version 2 exactly as the previous WebStore did: wrapper records
/// keyed by `key`, no batch number. Returns the open connection.
async fn create_version_2(db_name: &str) -> IdbDatabase {
	let request: IdbOpenDbRequest = factory().open_with_u32(db_name, 2).unwrap();
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
async fn test_version_2_database_is_replaced_empty() {
	let db_name = "test-end-batch-upgrade-v2";
	if !indexed_db_available() {
		return;
	}
	delete_database(db_name).await;
	let old = create_version_2(db_name).await;
	add_raw(
		&old,
		&[
			json!({"key": 0, "value": "old"}),
			json!({"key": 1, "value": "older"}),
		],
	)
	.await;
	assert_eq!(idb::read_all(&old).await.unwrap().len(), 2);
	old.close();

	let mut store = reopen(db_name).await;
	assert_eq!(store.persistence_state(), PersistenceState::Persisted);
	assert!(!store.has_data());
	assert_eq!((store.next_key, store.current_batch), (0, 0));
	let db = store.db.clone().unwrap();
	assert_eq!(db.version(), f64::from(idb::DB_VERSION));
	assert!(idb::read_all(&db).await.unwrap().is_empty());

	store.append(json!("a")).unwrap();
	store.end_batch().unwrap();
	store.append(json!("b")).unwrap();
	settle().await;
	let mut reopened = reopen(db_name).await;
	assert_eq!(
		drained(&mut reopened),
		vec![vec![json!("a")], vec![json!("b")]]
	);
	settle().await;
	reopened.reset();
	settle().await;
}
