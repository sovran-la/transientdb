use serde_json::json;
use std::io::Result;
use std::thread;
use std::time::Duration;
use transientdb::{DirectoryConfig, DirectoryStore, TransientDB};

fn directory_store_example() -> Result<()> {
	// Create a temporary directory for testing
	let dir = tempfile::tempdir()?;

	// Create a directory store configuration
	let config = DirectoryConfig {
		write_key: "logs-key".into(),
		storage_location: dir.path().to_owned(),
		base_filename: "events".into(),
		max_file_size: 1024 * 10, // 10KB max file size
		max_file_items: None,
	};

	// Initialize store with a validator
	let mut store = DirectoryStore::new(config)?;

	// Add an optional file validator that ensures files are properly formatted json
	store.set_file_validator(|path| {
		let content = std::fs::read_to_string(path)?;
		// Validate JSON structure
		let value: serde_json::Value = serde_json::from_str(&content)?;
		if value["batch"].as_array().is_none() {
			return Err(std::io::Error::new(
				std::io::ErrorKind::InvalidData,
				"Missing batch array",
			));
		}
		Ok(())
	});

	let db = TransientDB::new(store);

	// Simulate collecting log entries
	println!("Adding log entries...");
	for i in 0..5 {
		let log_entry = json!({
			"level": if i % 2 == 0 { "INFO" } else { "WARNING" },
			"service": "web-api",
			"message": format!("Request processed {}", i),
			"timestamp": chrono::Utc::now().to_rfc3339(),
			"details": {
				"request_id": format!("req_{}", i),
				"duration_ms": i * 100,
				"status_code": 200
			}
		});

		db.append(log_entry)?;
		thread::sleep(Duration::from_millis(100)); // Simulate time between logs
	}

	// Fetch and process log files
	println!("Fetching log files...");
	while let Some(result) = db.fetch(Some(2), Some(1024 * 5))? {
		if let Some(files) = result.data {
			for file in &files {
				println!("Processing file: {:?}", file.file_name().unwrap());
				let content = std::fs::read_to_string(file)?;
				let batch: serde_json::Value = serde_json::from_str(&content)?;
				println!("File contents: {}", serde_json::to_string_pretty(&batch)?);
			}
		}

		// Clean up processed files
		if let Some(removable) = result.removable {
			db.remove(&removable)?;
		}
	}

	Ok(())
}

fn main() -> Result<()> {
	println!("\n=== Directory Store Example ===");
	directory_store_example()?;

	Ok(())
}
