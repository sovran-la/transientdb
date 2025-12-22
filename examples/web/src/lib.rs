//! TransientDB WebStore Example
//!
//! This example demonstrates using TransientDB with WebStore in a browser environment.
//! It shows basic operations: append, fetch, remove, and persistence state checking.

use serde_json::json;
use transientdb::{PersistenceState, TransientDB, WebConfig, WebStore};
use wasm_bindgen::prelude::*;

/// Log a message to the browser console and the page
fn log(msg: &str) {
    web_sys::console::log_1(&msg.into());

    // Also append to the output div if it exists
    if let Some(window) = web_sys::window() {
        if let Some(document) = window.document() {
            if let Some(output) = document.get_element_by_id("output") {
                let current = output.inner_html();
                output.set_inner_html(&format!("{}<div class=\"log-entry\">{}</div>", current, msg));
            }
        }
    }
}

/// Log an error
fn log_error(msg: &str) {
    web_sys::console::error_1(&msg.into());

    if let Some(window) = web_sys::window() {
        if let Some(document) = window.document() {
            if let Some(output) = document.get_element_by_id("output") {
                let current = output.inner_html();
                output.set_inner_html(&format!(
                    "{}<div class=\"log-entry error\">❌ {}</div>",
                    current, msg
                ));
            }
        }
    }
}

/// Log a success
fn log_success(msg: &str) {
    log(&format!("✅ {}", msg));
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
        log_error(&format!("Demo failed: {:?}", e));
    }
}

async fn run_demo() -> Result<(), Box<dyn std::error::Error>> {
    // =========================================================================
    // Step 1: Create a WebStore with TransientDB
    // =========================================================================
    log("📦 Creating WebStore...");

    let config = WebConfig {
        write_key: "demo-app".to_string(),
        database_name: "transientdb-demo".to_string(),
        max_items: 100,
        max_fetch_size: 1024 * 1024,
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

    // Wrap in TransientDB for thread-safe access
    let db = TransientDB::new(store);
    log_success("TransientDB created");
    log("");

    // =========================================================================
    // Step 2: Append some events
    // =========================================================================
    log("📝 Appending events...");

    let events = vec![
        json!({
            "event": "page_view",
            "page": "/home",
            "timestamp": "2024-01-01T10:00:00Z"
        }),
        json!({
            "event": "button_click",
            "button": "sign_up",
            "timestamp": "2024-01-01T10:00:05Z"
        }),
        json!({
            "event": "form_submit",
            "form": "registration",
            "timestamp": "2024-01-01T10:00:30Z"
        }),
        json!({
            "event": "page_view",
            "page": "/dashboard",
            "timestamp": "2024-01-01T10:00:35Z"
        }),
    ];

    for event in &events {
        db.append(event.clone())?;
        log(&format!("   Added: {}", event["event"].as_str().unwrap_or("?")));
    }

    log_success(&format!("Appended {} events", events.len()));
    log("");

    // =========================================================================
    // Step 3: Check if we have data
    // =========================================================================
    log(&format!("📊 Has data: {}", db.has_data()));
    log("");

    // =========================================================================
    // Step 4: Fetch events (partial - just 2)
    // =========================================================================
    log("📥 Fetching first 2 events...");

    if let Some(result) = db.fetch(Some(2), None)? {
        if let Some(batch) = &result.data {
            log(&format!("   writeKey: {}", batch["writeKey"]));
            log(&format!("   sentAt: {}", batch["sentAt"]));

            if let Some(items) = batch["batch"].as_array() {
                log(&format!("   items: {}", items.len()));
                for item in items {
                    log(&format!("     - {}", item["event"].as_str().unwrap_or("?")));
                }
            }
        }

        // Remove the fetched events
        if let Some(removable) = result.removable {
            db.remove(&removable)?;
            log_success("Removed fetched events");
        }
    }
    log("");

    // =========================================================================
    // Step 5: Fetch remaining events
    // =========================================================================
    log("📥 Fetching remaining events...");

    if let Some(result) = db.fetch(None, None)? {
        if let Some(batch) = &result.data {
            if let Some(items) = batch["batch"].as_array() {
                log(&format!("   Remaining items: {}", items.len()));
                for item in items {
                    log(&format!("     - {}", item["event"].as_str().unwrap_or("?")));
                }
            }
        }

        if let Some(removable) = result.removable {
            db.remove(&removable)?;
            log_success("Removed remaining events");
        }
    }
    log("");

    // =========================================================================
    // Step 6: Verify empty
    // =========================================================================
    log(&format!("📊 Has data after removal: {}", db.has_data()));
    log("");

    // =========================================================================
    // Step 7: Demo reset
    // =========================================================================
    log("🔄 Demo: Adding more events then resetting...");

    for i in 0..5 {
        db.append(json!({"index": i}))?;
    }
    log(&format!("   Added 5 events, has_data: {}", db.has_data()));

    db.reset();
    log(&format!("   After reset, has_data: {}", db.has_data()));
    log_success("Reset complete");
    log("");

    // =========================================================================
    // Done!
    // =========================================================================
    log("🎉 Demo complete!");
    log("");
    log("Try refreshing the page - if IndexedDB is available,");
    log("any unsent events would persist across refreshes.");

    Ok(())
}
