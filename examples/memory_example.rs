use serde_json::json;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use transientdb::{MemoryConfig, MemoryStore, Result, TransientDB};

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
		thread::sleep(Duration::from_millis(100)); // Simulate time between events
	}

	// Fetch events two at a time, at most 64KB of them, and wrap each batch in the
	// request body this (imaginary) service expects.
	println!("Fetching and processing events...");
	while let Some(items) = db.fetch(2, 64 * 1024)? {
		let body = format!("{{\"events\":{},\"source\":\"sensor-7\"}}", items.json());
		println!("Sending {} events: {body}", items.len());

		// After a successful send, remove the fetched items. Had the send failed,
		// dropping `items` would have given them back for a later fetch.
		db.remove(items)?;
		println!("Removed processed events from store");
	}

	Ok(())
}

fn main() -> Result<()> {
	println!("=== Memory Store Example ===");
	memory_store_example()?;

	Ok(())
}
