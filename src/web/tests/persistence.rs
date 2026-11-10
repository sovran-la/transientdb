//! Items persist under the key the store gave them, so what a later instance hydrates is
//! exactly what this one still holds: any JSON value, unmodified, in append order.

use super::support::*;
use super::*;

/// A store on `db_name` emptied of anything a previous run left, or `None`, after
/// logging, when IndexedDB is unavailable.
async fn persisted(config: WebConfig) -> Option<WebStore> {
	let store = fresh_store_with(config).await;
	if !store.is_persisted() {
		web_sys::console::log_1(&"Skipping persistence test - no persistence".into());
		return None;
	}
	Some(store)
}

fn config_for(db_name: &str) -> WebConfig {
	config(db_name, 4096, with_source(KEY))
}

fn tagged(range: std::ops::Range<i64>) -> Vec<Value> {
	range.map(|i| json!({"i": i})).collect()
}

fn append_all(store: &mut WebStore, items: &[Value]) {
	for item in items {
		store.append(item.clone()).unwrap();
	}
}

/// Fetches every queued item in one batch and removes those at `positions`.
fn remove_positions(store: &mut WebStore, positions: &[usize]) {
	let result = store.fetch(None, None).unwrap().unwrap();
	let chosen: Vec<Box<dyn Equivalent>> = result
		.removable
		.unwrap()
		.into_iter()
		.enumerate()
		.filter(|(i, _)| positions.contains(i))
		.map(|(_, removable)| removable)
		.collect();
	assert_eq!(chosen.len(), positions.len());
	store.remove(&chosen).unwrap();
}

fn keys(store: &WebStore) -> Vec<u32> {
	store.items.iter().map(|e| e.key).collect()
}

#[wasm_bindgen_test]
async fn test_every_json_type_round_trips_unchanged_in_order() {
	let db_name = "test-persist-json-types";
	let Some(mut store) = persisted(config_for(db_name)).await else {
		return;
	};
	let items = vec![
		json!(null),
		json!(true),
		json!(false),
		json!(0),
		json!(42),
		json!(-7),
		json!(42.5),
		json!(-0.125),
		json!(""),
		json!("string"),
		json!([]),
		json!([1, "two", null, [3]]),
		json!({}),
		json!({"event": "e", "n": 1}),
		json!({"a": {"b": {"c": [1, {"d": null}]}}}),
		json!({"key": "not the record key"}),
		json!({"key": 999_999}),
		json!({"value": "not the record value"}),
		json!({"key": 1, "value": 2}),
		json!({"_idb_key": 5, "other": "kept"}),
	];
	append_all(&mut store, &items);
	settle().await;

	assert_eq!(hydrated(db_name, 4096).await, items);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_memory_keys_are_the_indexeddb_keys() {
	let db_name = "test-persist-keys-agree";
	let Some(mut store) = persisted(config_for(db_name)).await else {
		return;
	};
	append_all(&mut store, &tagged(0..5));
	remove_positions(&mut store, &[1]);
	settle().await;

	let db = store.db.clone().unwrap();
	let records = idb::read_all(&db).await.unwrap();
	let stored: Vec<(u32, Value)> = records
		.iter()
		.map(|r| (idb::record_key(r).unwrap(), idb::record_value(r).unwrap()))
		.collect();
	let in_memory: Vec<(u32, Value)> = store
		.items
		.iter()
		.map(|e| (e.key, e.value.clone()))
		.collect();
	assert_eq!(stored, in_memory);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_removed_items_stay_removed_after_reopen() {
	let db_name = "test-persist-remove-some";
	let Some(mut store) = persisted(config_for(db_name)).await else {
		return;
	};
	let items = tagged(0..6);
	append_all(&mut store, &items);
	remove_positions(&mut store, &[1, 3, 4]);
	let expected = vec![items[0].clone(), items[2].clone(), items[5].clone()];
	assert_eq!(queued(&store), expected);
	settle().await;

	assert_eq!(hydrated(db_name, 4096).await, expected);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_append_then_immediate_remove_is_gone() {
	let db_name = "test-persist-append-remove";
	let Some(mut store) = persisted(config_for(db_name)).await else {
		return;
	};
	// No await anywhere between these calls: the add must not land after the delete.
	store.append(json!("short-lived")).unwrap();
	let result = store.fetch(None, None).unwrap().unwrap();
	store.remove(&result.removable.unwrap()).unwrap();
	store.append(json!("kept")).unwrap();
	store.append(json!("also short-lived")).unwrap();
	remove_positions(&mut store, &[1]);
	settle().await;

	assert_eq!(hydrated(db_name, 4096).await, vec![json!("kept")]);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_writes_are_ordered_before_the_next_instance_reads() {
	let db_name = "test-persist-no-settle";
	let Some(mut store) = persisted(config_for(db_name)).await else {
		return;
	};
	// No settle before reopening: a new instance's read is ordered after every write
	// this one issued, so the result cannot depend on timing.
	append_all(&mut store, &tagged(0..4));
	remove_positions(&mut store, &[0, 2]);
	store.append(json!({"i": 4})).unwrap();

	assert_eq!(
		hydrated(db_name, 4096).await,
		vec![json!({"i": 1}), json!({"i": 3}), json!({"i": 4})]
	);
	store.reset();
	assert!(hydrated(db_name, 4096).await.is_empty());
}

#[wasm_bindgen_test]
async fn test_reset_of_this_sessions_items_persists() {
	let db_name = "test-persist-reset";
	let Some(mut store) = persisted(config_for(db_name)).await else {
		return;
	};
	append_all(&mut store, &tagged(0..5));
	store.reset();
	settle().await;
	assert!(hydrated(db_name, 4096).await.is_empty());

	// Appends right after a reset survive it.
	append_all(&mut store, &tagged(5..7));
	store.reset();
	append_all(&mut store, &tagged(7..9));
	settle().await;
	assert_eq!(hydrated(db_name, 4096).await, tagged(7..9));
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_drop_oldest_persists() {
	let db_name = "test-persist-drop-oldest";
	let mut config = config_for(db_name);
	config.max_items = Some(3);
	let Some(mut store) = persisted(config).await else {
		return;
	};
	let items = tagged(0..8);
	append_all(&mut store, &items);
	assert_eq!(queued(&store), items[5..]);
	settle().await;

	assert_eq!(hydrated(db_name, 4096).await, items[5..]);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_identical_items_are_removed_one_at_a_time() {
	let db_name = "test-persist-identical";
	let Some(mut store) = persisted(config_for(db_name)).await else {
		return;
	};
	let same = json!({"event": "click"});
	append_all(&mut store, &vec![same.clone(); 4]);

	let result = store.fetch(Some(1), None).unwrap().unwrap();
	assert_eq!(checked_batch(&result, 4096), vec![same.clone()]);
	store.remove(&result.removable.unwrap()).unwrap();
	assert_eq!(queued(&store), vec![same.clone(); 3]);
	settle().await;

	assert_eq!(hydrated(db_name, 4096).await, vec![same.clone(); 3]);
	store.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_keys_keep_increasing_across_instances() {
	let db_name = "test-persist-keys-increase";
	let Some(mut first) = persisted(config_for(db_name)).await else {
		return;
	};
	append_all(&mut first, &tagged(0..3));
	settle().await;

	let mut second = WebStore::new(config_for(db_name)).await;
	assert_eq!(queued(&second), tagged(0..3));
	append_all(&mut second, &tagged(3..6));
	let second_keys = keys(&second);
	assert!(
		second_keys.windows(2).all(|w| w[0] < w[1]),
		"{second_keys:?}"
	);
	settle().await;

	let mut third = WebStore::new(config_for(db_name)).await;
	assert_eq!(queued(&third), tagged(0..6));
	assert_eq!(keys(&third), second_keys);
	append_all(&mut third, &tagged(6..8));
	settle().await;

	assert_eq!(hydrated(db_name, 4096).await, tagged(0..8));
	third.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_remove_works_on_hydrated_and_new_items_together() {
	let db_name = "test-persist-mixed";
	let Some(mut first) = persisted(config_for(db_name)).await else {
		return;
	};
	append_all(&mut first, &tagged(0..3));
	settle().await;

	let mut second = WebStore::new(config_for(db_name)).await;
	append_all(&mut second, &tagged(3..5));
	assert_eq!(queued(&second), tagged(0..5));
	// One hydrated item, one appended this session.
	remove_positions(&mut second, &[1, 3]);
	let expected = vec![json!({"i": 0}), json!({"i": 2}), json!({"i": 4})];
	assert_eq!(queued(&second), expected);
	settle().await;

	assert_eq!(hydrated(db_name, 4096).await, expected);
	second.reset();
	settle().await;
}

#[wasm_bindgen_test]
async fn test_memory_only_mode_keeps_working() {
	let mut store = WebStore::new(config_for("test-persist-memory-only")).await;
	// Simulate unavailable IndexedDB: no handle, so no writes are attempted.
	store.db = None;
	store.persistence_state = PersistenceState::MemoryOnly;
	store.reset();
	assert!(!store.is_persisted());

	let same = json!("dup");
	append_all(&mut store, &[same.clone(), same.clone(), json!("x")]);
	let result = store.fetch(Some(1), None).unwrap().unwrap();
	store.remove(&result.removable.unwrap()).unwrap();
	assert_eq!(queued(&store), vec![same, json!("x")]);
	store.reset();
	assert!(!store.has_data());
}
