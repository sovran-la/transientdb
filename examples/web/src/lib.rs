//! TransientDB WebStore Example
//!
//! This example demonstrates using TransientDB with WebStore in a browser environment:
//! appending events, fetching them in batches, wrapping each batch in a request body,
//! removing what was delivered, and giving back what was not.

use serde_json::{json, Value};
use transientdb::{Error, PersistenceState, TransientDB, WebConfig, WebStore};
use wasm_bindgen::prelude::*;

/// The most events one request carries, and the most bytes of event text in it.
const BATCH_ITEMS: usize = 2;
const BATCH_BYTES: usize = 64 * 1024;

/// Log a message to the browser console and the page
fn log(msg: &str) {
	web_sys::console::log_1(&msg.into());
	append_entry("log-entry", msg);
}

/// Log an error
fn log_error(msg: &str) {
	web_sys::console::error_1(&msg.into());
	append_entry("log-entry error", &format!("❌ {msg}"));
}

/// Log a success
fn log_success(msg: &str) {
	log(&format!("✅ {msg}"));
}

/// Appends a line to the output div, if the page has one.
fn append_entry(class: &str, msg: &str) {
	let output = web_sys::window()
		.and_then(|window| window.document())
		.and_then(|document| document.get_element_by_id("output"));
	if let Some(output) = output {
		let current = output.inner_html();
		output.set_inner_html(&format!("{current}<div class=\"{class}\">{msg}</div>"));
	}
}

/// Main entry point - runs the demo
#[wasm_bindgen(start)]
pub async fn main() {
	// Set up panic hook for better error messages
	console_error_panic_hook::set_once();

	log("🦀 TransientDB WebStore Example");
	log("================================");
	log("");

	if let Err(e) = run_demo().await {
		log_error(&format!("Demo failed: {e}"));
	}
}

/// Queues an event. One the store can never hold is reported and dropped: retrying
/// would not help.
fn track(db: &TransientDB, event: Value) -> transientdb::Result<()> {
	match db.append(event) {
		Ok(()) => Ok(()),
		Err(e @ (Error::ItemTooLarge { .. } | Error::ItemTooDeep)) => {
			log(&format!("   Dropped an event: {e}"));
			Ok(())
		}
		Err(e) => Err(e),
	}
}

/// Stands in for a `fetch()` to a collection endpoint, which fails while `online` is
/// false.
fn send(body: &str, online: bool) -> Result<(), String> {
	if online {
		log(&format!("   POST {body}"));
		Ok(())
	} else {
		Err("the network is down".to_string())
	}
}

/// "1 event", "2 events".
fn event_count(count: usize) -> String {
	match count {
		1 => "1 event".to_string(),
		_ => format!("{count} events"),
	}
}

/// Sends what is queued, one request per fetch, until nothing is left or a request
/// fails. Returns how many events were sent.
fn flush(db: &TransientDB, online: bool) -> transientdb::Result<usize> {
	let mut sent = 0;
	while let Some(items) = db.fetch(BATCH_ITEMS, BATCH_BYTES)? {
		// The request body is ours to shape; the store only supplies the array.
		let body = format!("{{\"events\":{},\"source\":\"demo-app\"}}", items.json());
		match send(&body, online) {
			Ok(()) => {
				sent += items.len();
				// Delivered: delete them, in memory and in IndexedDB.
				db.remove(items)?;
			}
			Err(reason) => {
				log(&format!(
					"   Could not send {}: {reason}",
					event_count(items.len())
				));
				// Give them back: a later fetch returns them, in order. Dropping `items`
				// does the same. Stop here, or the next fetch would hand them straight back.
				items.release();
				break;
			}
		}
	}
	Ok(sent)
}

async fn run_demo() -> Result<(), Error> {
	// =========================================================================
	// Step 1: Create a WebStore with TransientDB
	// =========================================================================
	log("📦 Creating WebStore...");

	let config = WebConfig {
		database_name: "transientdb-demo".to_string(),
		max_item_size: 8 * 1024, // no event over 8 KiB serialized
		max_items: 100,          // the 101st append drops the oldest event
	};

	let store = WebStore::new(config).await;

	// Check persistence state
	match store.persistence_state() {
		PersistenceState::Persisted => {
			log_success("IndexedDB available - data will persist across page refreshes");
		}
		PersistenceState::MemoryOnly => {
			log("⚠️ Memory-only mode - data will be lost on page refresh");
			log("   (This happens in private browsing or third-party iframes)");
		}
	}

	// Wrap in TransientDB to share it like any other store
	let db = TransientDB::new(store);
	log_success("TransientDB created");
	log("");

	// =========================================================================
	// Step 2: Send anything left from the last visit
	// =========================================================================
	if db.has_data() {
		log("📤 Sending events left from the last visit...");
		let sent = flush(&db, true)?;
		log_success(&format!("Sent {} from the last visit", event_count(sent)));
	} else {
		log("📭 Nothing left from the last visit");
	}
	log("");

	// =========================================================================
	// Step 3: Append some events
	// =========================================================================
	log("📝 Appending events...");

	let events = vec![
		json!({"event": "page_view", "page": "/home"}),
		json!({"event": "button_click", "button": "sign_up"}),
		json!({"event": "form_submit", "form": "registration"}),
		json!({"event": "page_view", "page": "/dashboard"}),
	];
	for event in events {
		log(&format!(
			"   Added: {}",
			event["event"].as_str().unwrap_or("?")
		));
		track(&db, event)?;
	}
	// Larger than max_item_size: never stored.
	track(
		&db,
		json!({"event": "upload", "data": "x".repeat(10 * 1024)}),
	)?;
	log(&format!("📊 Has data: {}", db.has_data()));
	log("");

	// =========================================================================
	// Step 4: Try to send while offline
	// =========================================================================
	log("📡 Sending while offline...");
	let sent = flush(&db, false)?;
	log(&format!("   Sent {sent}; still queued: {}", db.has_data()));
	log("");

	// =========================================================================
	// Step 5: Send once back online
	// =========================================================================
	log("📡 Sending once back online...");
	let sent = flush(&db, true)?;
	log_success(&format!("Sent {}", event_count(sent)));
	log(&format!("📊 Has data after sending: {}", db.has_data()));
	log("");

	// =========================================================================
	// Step 6: Demo reset
	// =========================================================================
	log("🔄 Demo: Adding more events then resetting...");

	for i in 0..5 {
		track(&db, json!({"index": i}))?;
	}
	log(&format!("   Added 5 events, has_data: {}", db.has_data()));

	db.reset();
	log(&format!("   After reset, has_data: {}", db.has_data()));
	log_success("Reset complete");
	log("");

	// =========================================================================
	// Step 7: Leave one event for the next visit
	// =========================================================================
	track(&db, json!({"event": "page_hide", "page": "/dashboard"}))?;
	log("🎉 Demo complete!");
	log("");
	log("One event is left queued. Refresh the page: if IndexedDB is available,");
	log("step 2 sends it.");

	Ok(())
}
