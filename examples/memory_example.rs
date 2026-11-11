use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use transientdb::{Error, MemoryConfig, MemoryStore, Result, TransientDB};

fn memory_store_example() -> Result<()> {
	// Create a memory store configuration.
	let config = MemoryConfig {
		max_item_size: 8 * 1024, // No event over 8KB serialized
		max_items: 1000,         // Store up to 1000 items; more drops the oldest
	};

	// Initialize the store and wrap it in TransientDB
	let store = MemoryStore::new(config);
	let db = TransientDB::new(store);

	// Simulate collecting user events
	println!("Adding user events...");
	for i in 0..5 {
		let millis = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.map_or(0, |since| since.as_millis() as u64);
		let event = json!({
			"event_type": "page_view",
			"user_id": format!("user_{}", i),
			"timestamp": millis,
			"page": format!("/product/{}", i),
			"metadata": {
				"browser": "Chrome",
				"platform": "macOS",
				"screen_size": "1920x1080"
			}
		});

		db.append(event)?;
	}

	// An event the store can never hold is rejected, and nothing is stored for it. Drop it
	// or report it; retrying will not help.
	match db.append(json!({"event_type": "upload", "data": "x".repeat(10 * 1024)})) {
		Err(e @ Error::ItemTooLarge { .. }) => println!("Dropped an event: {e}"),
		other => panic!("expected ItemTooLarge, got {other:?}"),
	}

	// Fetch events two at a time, at most 64KB of them, and wrap each batch in the
	// request body this (imaginary) service expects.
	println!("Fetching and processing events...");
	let mut attempt = 0;
	let mut sent = 0;
	while let Some(items) = db.fetch(2, 64 * 1024)? {
		let body = format!("{{\"events\":{},\"source\":\"sensor-7\"}}", items.json());
		attempt += 1;

		// Pretend the second request fails.
		if attempt == 2 {
			println!(
				"Sending {} events failed; keeping them for a retry",
				items.len()
			);
			// Releasing gives them back: the next fetch returns them, in order. Dropping
			// `items` does the same.
			items.release();
			continue;
		}

		println!("Sending {} events: {body}", items.len());
		sent += items.len();
		// After a successful send, remove the fetched items.
		db.remove(items)?;
		println!("Removed processed events from store");
	}

	// Every stored event was sent once, the released ones on a later attempt.
	assert_eq!(sent, 5, "every stored event should have been sent");
	assert!(!db.has_data(), "nothing should be left once all is sent");
	Ok(())
}

fn main() -> Result<()> {
	println!("=== Memory Store Example ===");
	memory_store_example()?;

	Ok(())
}
