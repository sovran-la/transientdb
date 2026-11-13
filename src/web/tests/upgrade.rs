//! Other pages upgrading or deleting the database: a store waits at most about a second
//! for them, and never makes them wait. It carries on in memory only instead.

use super::schema::{add_raw, delete_database, factory};
use super::*;
use contract::{append_all, tagged};
use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;
use web_sys::{IdbOpenDbRequest, IdbRequest};

/// Runs `future` in the background and returns its output if it finishes within about
/// two seconds; `None` if it is still waiting by then.
async fn within_two_seconds<T: 'static>(future: impl Future<Output = T> + 'static) -> Option<T> {
	let slot = Rc::new(RefCell::new(None));
	let out = slot.clone();
	wasm_bindgen_futures::spawn_local(async move { *out.borrow_mut() = Some(future.await) });
	for _ in 0..20 {
		if slot.borrow().is_some() {
			break;
		}
		settle().await;
	}
	let output = slot.borrow_mut().take();
	output
}

/// Opens `db_name` at `version` the way another page would, without closing it when a
/// newer version is asked for, creating the object store (keyed the way version 4 kept
/// it) if there is none.
async fn open_at(db_name: &'static str, version: u32) -> IdbDatabase {
	let request: IdbOpenDbRequest = factory().open_with_u32(db_name, version).unwrap();
	let on_upgrade = Closure::once(move |event: web_sys::IdbVersionChangeEvent| {
		let request: IdbRequest = event.target().unwrap().unchecked_into();
		let db: IdbDatabase = request.result().unwrap().unchecked_into();
		if !db.object_store_names().contains(idb::STORE_NAME) {
			let params = web_sys::IdbObjectStoreParameters::new();
			params.set_key_path(&JsValue::from_str("key"));
			db.create_object_store_with_optional_parameters(idb::STORE_NAME, &params)
				.unwrap();
		}
	});
	request.set_onupgradeneeded(Some(on_upgrade.as_ref().unchecked_ref()));
	on_upgrade.forget();
	idb::await_request::<IdbDatabase>(&request).await.unwrap()
}

/// Every record in `db`, read in a transaction that has finished by the time this
/// returns, so that closing `db` afterwards takes effect at once.
async fn read_all_finished(db: &IdbDatabase) -> Vec<Value> {
	let transaction = db.transaction_with_str(idb::STORE_NAME).unwrap();
	let finished = idb::completion(&transaction);
	let request = transaction
		.object_store(idb::STORE_NAME)
		.unwrap()
		.get_all()
		.unwrap();
	let records = idb::await_request::<js_sys::Array>(&request).await.unwrap();
	finished.await.unwrap();
	let json = |r: JsValue| js_sys::JSON::stringify(&r).unwrap().as_string().unwrap();
	records
		.iter()
		.map(|r| serde_json::from_str(&json(r)).unwrap())
		.collect()
}

/// A store opened now, while nothing else holds `db_name` open, upgrades it as usual:
/// persisted, at the current version, and empty.
async fn assert_opens_upgraded(db_name: &str) {
	let mut store = WebStore::new(config(db_name, 4096, 1000)).await;
	assert!(store.is_persisted());
	assert_eq!(store.db().version(), f64::from(idb::DB_VERSION));
	assert!(!store.has_data());
	append_all(&mut store, &tagged(0..2));
	assert_eq!(flushed(&store).await, stored(&store));
	store.reset();
	settle().await;
}

/// `store` still does everything, in memory: it keeps what it holds, and fetches,
/// removes and resets.
fn assert_works_in_memory(store: &mut WebStore, held: Vec<Value>) {
	assert_eq!(store.persistence_state(), PersistenceState::MemoryOnly);
	assert!(!store.is_persisted());
	assert_eq!(queued(store), held);
	append_all(store, &[json!("added")]);
	let items = store.fetch(held.len(), 4096).unwrap();
	assert_eq!(items.as_ref().map_or_else(Vec::new, Items::values), held);
	if let Some(items) = items {
		store.remove(items).unwrap();
	}
	assert_eq!(queued(store), vec![json!("added")]);
	store.reset();
	assert!(!store.has_data());
}

/// A page loaded before a deploy holds the database open at the older version and does
/// not close it. Opening a store does not wait for that page: it starts memory-only, and
/// the database is left as it was until a store opens it after that page is gone.
#[wasm_bindgen_test]
async fn test_open_gives_up_while_an_older_version_is_held_open() {
	let db_name = "test-upgrade-blocked";
	delete_database(db_name).await;
	let old = open_at(db_name, idb::DB_VERSION - 1).await;
	add_raw(&old, &[json!({"key": 0, "value": "old"})]).await;

	let opened = within_two_seconds(WebStore::new(config(db_name, 4096, 1000))).await;
	let mut store = opened.expect("WebStore::new waited for the old connection to close");
	assert!(store.shared.db.is_none());
	assert_works_in_memory(&mut store, vec![]);

	// The old page goes away. The abandoned request's upgrade is aborted, not done: the
	// next open, queued behind it, finds the old version and its record untouched.
	old.close();
	let request = factory().open(db_name).unwrap();
	let after = idb::await_request::<IdbDatabase>(&request).await.unwrap();
	assert_eq!(after.version(), f64::from(idb::DB_VERSION - 1));
	assert_eq!(
		read_all_finished(&after).await,
		vec![json!({"key": 0, "value": "old"})]
	);
	after.close();

	// A store opened now upgrades it as usual.
	assert_opens_upgraded(db_name).await;
}

/// The older page has closed its connection, but the close is still waiting for that
/// page's last write to commit, which blocks the upgrade until it has. Opening waits for
/// that, instead of taking it for a page that holds the database open.
#[wasm_bindgen_test]
async fn test_open_waits_for_a_connection_that_is_closing() {
	let db_name = "test-upgrade-closing";
	delete_database(db_name).await;
	let old = open_at(db_name, idb::DB_VERSION - 1).await;
	let transaction = old
		.transaction_with_str_and_mode(idb::STORE_NAME, web_sys::IdbTransactionMode::Readwrite)
		.unwrap();
	let records = transaction.object_store(idb::STORE_NAME).unwrap();
	for key in 0..2000 {
		let record = js_sys::JSON::parse(&json!({"key": key, "value": "old"}).to_string());
		records.put(&record.unwrap()).unwrap();
	}
	old.close();

	assert_opens_upgraded(db_name).await;
}

/// The older page closes its connection soon after the open is blocked by it: opening
/// still succeeds, with that open's own connection.
#[wasm_bindgen_test]
async fn test_open_succeeds_when_the_holder_closes_during_the_wait() {
	let db_name = "test-upgrade-closes-soon";
	delete_database(db_name).await;
	let old = open_at(db_name, idb::DB_VERSION - 1).await;
	add_raw(&old, &[json!({"key": 0, "value": "old"})]).await;
	wasm_bindgen_futures::spawn_local(async move {
		// About 300 ms in: after `new` is blocked, well within its wait.
		for _ in 0..3 {
			settle().await;
		}
		old.close();
	});

	assert_opens_upgraded(db_name).await;
}

/// The older page closes its connection just before the open's wait runs out, and the
/// upgrade, which has a lot of old data to delete, is still committing when it does:
/// opening uses that upgrade instead of giving up on a database it has just upgraded.
///
/// Timers are not exact, and the upgrade starts only some time after the close, so it
/// may start after the wait has run out instead. Then the store rightly gives up, and
/// its upgrade, when it gets to start, is aborted. Either way the store is persisted if
/// and only if the database ended up at the current version: if it is, the old records
/// are gone; if it is not, the database is still at the older version with its old
/// records intact. A store that gives up on a database it has just upgraded fails this
/// whenever the upgrade starts in time, which is on most runs but not on every one.
#[wasm_bindgen_test]
async fn test_open_uses_an_upgrade_that_started_before_the_wait_ran_out() {
	let db_name = "test-upgrade-late-close";
	delete_database(db_name).await;
	let old = open_at(db_name, idb::DB_VERSION - 1).await;
	let filler = "x".repeat(1000);
	let records: Vec<Value> = (0..10_000)
		.map(|key| json!({"key": key, "value": filler}))
		.collect();
	add_raw(&old, &records).await;
	let on_versionchange = Closure::once({
		let old = old.clone();
		move |_event: web_sys::IdbVersionChangeEvent| {
			wasm_bindgen_futures::spawn_local(async move {
				let close_at = connect::BLOCKED_GRACE_MS - 25;
				gloo_timers::future::TimeoutFuture::new(close_at as u32).await;
				old.close();
			});
		}
	});
	old.set_onversionchange(Some(on_versionchange.as_ref().unchecked_ref()));
	on_versionchange.forget();

	let store = WebStore::new(config(db_name, 4096, 1000)).await;
	let persisted = store.is_persisted();
	drop(store);
	let request = factory().open(db_name).unwrap();
	let after = idb::await_request::<IdbDatabase>(&request).await.unwrap();
	let version = after.version();
	let held = read_all_finished(&after).await;
	after.close();
	if persisted {
		assert_eq!(
			version,
			f64::from(idb::DB_VERSION),
			"the store is persisted on a database that was not upgraded"
		);
		assert_eq!(held, Vec::<Value>::new());
	} else {
		assert_ne!(
			version,
			f64::from(idb::DB_VERSION),
			"the store gave up on the database it had just upgraded"
		);
		assert_eq!(version, f64::from(idb::DB_VERSION - 1));
		assert!(
			held == records,
			"the abandoned upgrade changed the old records"
		);
	}
	delete_database(db_name).await;
}

/// Another page asks for a newer version while a store is open. The store closes its
/// connection rather than block that page, and carries on in memory only; nothing it
/// does afterwards reaches IndexedDB.
#[wasm_bindgen_test]
async fn test_store_closes_its_connection_for_a_newer_version() {
	let db_name = "test-upgrade-versionchange";
	delete_database(db_name).await;
	let mut store = WebStore::new(config(db_name, 4096, 1000)).await;
	assert!(store.is_persisted());
	append_all(&mut store, &tagged(0..2));

	let newer = within_two_seconds(open_at(db_name, idb::DB_VERSION + 1)).await;
	let newer = newer.expect("the store's connection blocked the newer version");
	assert_works_in_memory(&mut store, tagged(0..2));

	// The writes issued before the close landed; none after it did, the reset included.
	let records = idb::read_all(&newer).await.unwrap();
	let texts: Vec<String> = records
		.iter()
		.map(|r| idb::parse_record(r).unwrap().1)
		.collect();
	assert_eq!(texts, [r#"{"i":0}"#, r#"{"i":1}"#]);
	newer.close();
	delete_database(db_name).await;
}

/// Deleting the database elsewhere is not blocked either.
#[wasm_bindgen_test]
async fn test_store_closes_its_connection_for_a_delete() {
	let db_name = "test-upgrade-delete";
	let mut store = fresh_store(db_name, 4096, 1000).await;
	assert!(store.is_persisted());
	append_all(&mut store, &tagged(0..1));

	let deleted = within_two_seconds(delete_database(db_name)).await;
	assert!(
		deleted.is_some(),
		"the store's connection blocked the delete"
	);
	assert_works_in_memory(&mut store, tagged(0..1));
}
