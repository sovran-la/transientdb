# TransientDB

TransientDB is a lightweight, thread-safe temporary data storage system designed for efficient handling of transient data in Rust applications. It provides flexible storage options with both in-memory and file-based implementations, making it ideal for scenarios requiring temporary data buffering, event batching, or intermediate data storage.

## Features

- **Multiple Storage Backends**
    - In-memory storage with configurable size limits
    - File-based storage with automatic file rotation
    - Browser-based IndexedDB storage for WASM targets
    - Extensible interface for custom implementations

- **Thread-Safe Operations**
    - Concurrent append and fetch operations
    - Atomic file operations for the directory-based store
    - Robust error handling and recovery
    - Mutex-based synchronization

- **Configurable Behavior**
    - Customizable maximum file/batch sizes
    - File validation hooks
    - Flexible fetch limits (count and size-based)
    - Consistent data format across stores

- **FIFO (First-In-First-Out) Data Management**
    - Automatic cleanup of old data
    - Efficient batch processing
    - Controlled memory usage
    - Safe removal of processed data

## Installation

Add TransientDB to your `Cargo.toml`:

```toml
[dependencies]
transientdb = "0.2"  # Replace with actual version
```

For WASM/browser targets, enable the `web` feature:

```toml
[target.'cfg(target_arch = "wasm32")'.dependencies]
transientdb = { version = "0.2", features = ["web"] }
```

> **Note:** The `web` feature only compiles on WASM targets. On native targets, it's automatically excluded.

## Core Types

### TransientDB<T>
The main wrapper type providing thread-safe access to any storage implementation. The type parameter `T` determines the output type of fetch operations (e.g., `Value` for MemoryStore or `Vec<PathBuf>` for DirectoryStore).

### DataResult<T>
A container for fetch results that includes:
- `data`: The fetched data of type `T` (JSON Value for MemoryStore or file paths for DirectoryStore)
- `removable`: Internal tracking data used by `remove()` to clean up processed items

### DataStore Trait
The core interface that storage implementations must provide:
- `append()`: Add new items to the store
- `fetch()`: Retrieve batches of data with optional limits
- `remove()`: Clean up processed data
- `has_data()`: Check if data is available
- `reset()`: Clear all stored data

## Usage

### Memory Store Example

```rust
use transientdb::{MemoryConfig, MemoryStore, TransientDB};
use serde_json::json;

// Configure an in-memory store
let config = MemoryConfig {
    write_key: "my-app".into(),
    max_items: 1000,
    max_fetch_size: 1024 * 1024, // 1MB
};

// Create the store
let store = MemoryStore::new(config);
let db = TransientDB::new(store);

// Append data
db.append(json!({
    "event": "user_login",
    "timestamp": "2024-01-01T00:00:00Z"
}))?;

// Fetch data (up to 100 items)
if let Some(result) = db.fetch(Some(100), None)? {
    if let Some(batch) = result.data {
        // Process the batch - it's already a serde_json::Value
        if let Some(items) = batch["batch"].as_array() {
            for item in items {
                println!("Processing event: {}", item["event"]);
            }
        }
    }
}
```

### Directory Store Example

```rust
use std::path::PathBuf;
use transientdb::{DirectoryConfig, DirectoryStore, TransientDB};
use serde_json::json;

// Configure a file-based store
let config = DirectoryConfig {
    write_key: "my-app".into(),
    storage_location: PathBuf::from("/tmp/events"),
    base_filename: "batch".into(),
    max_file_size: 1024 * 1024, // 1MB
};

// Create the store with custom validation
let mut store = DirectoryStore::new(config)?;
store.set_file_validator(|path| {
    // Custom validation logic
    let metadata = std::fs::metadata(path)?;
    if metadata.len() < 10 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "File too small"
        ));
    }
    Ok(())
});

let db = TransientDB::new(store);

// Append events
db.append(json!({
    "event": "purchase",
    "amount": 99.99
}))?;

// Fetch completed files
if let Some(result) = db.fetch(None, None)? {
    if let Some(files) = result.data {
        // Process the files
        for file_path in files {
            // Read and process each file
            let content = std::fs::read_to_string(file_path)?;
            let batch: serde_json::Value = serde_json::from_str(&content)?;
            
            // Access the items array
            if let Some(items) = batch["batch"].as_array() {
                for item in items {
                    println!("Processing event: {}", item["event"]);
                }
            }
        }
    }
    // Clean up processed files
    if let Some(removable) = result.removable {
        db.remove(&removable)?;
    }
}
```

### Web Store Example (WASM)

```rust
use transientdb::{TransientDB, WebConfig, WebStore, PersistenceState};
use serde_json::json;

// Configure a browser-based store
let config = WebConfig {
    write_key: "my-app".into(),
    database_name: "my-app-events".into(),
    max_items: 1000,
    max_fetch_size: 1024 * 1024, // 1MB
};

// Create the store (async - opens IndexedDB)
let store = WebStore::new(config).await;

// Check persistence state - IndexedDB may be unavailable
// in private browsing or third-party iframe contexts
match store.persistence_state() {
    PersistenceState::Persisted => {
        // Full persistence available
    }
    PersistenceState::MemoryOnly => {
        // Degraded mode - consider flushing more frequently
        println!("Warning: Running in memory-only mode");
    }
}

// Wrap in TransientDB for thread-safe access
let db = TransientDB::new(store);

// Append data
db.append(json!({
    "event": "page_view",
    "url": "/home"
}))?;

// Fetch data
if let Some(result) = db.fetch(Some(100), None)? {
    if let Some(batch) = result.data {
        // Process the batch
        if let Some(items) = batch["batch"].as_array() {
            for item in items {
                println!("Processing event: {}", item["event"]);
            }
        }
    }
    // Clean up processed items
    if let Some(removable) = result.removable {
        db.remove(&removable)?;
    }
}
```

> **See also:** The [`examples/web/`](examples/web/) directory contains a complete, runnable WASM example with HTML and build instructions.

## Storage Implementations

### MemoryStore
- Stores data in memory using a FIFO queue
- Automatically removes old items when max_items is reached
- Returns data as serde_json::Value
- Ideal for high-throughput, temporary data storage
- No cleanup required (fetch automatically removes returned items)

### DirectoryStore
- Stores data in rotating files in a specified directory
- Automatic file management and rotation
- Returns paths to completed files
- Supports custom file validation
- Requires explicit cleanup via remove()
- Ideal for larger datasets and persistent storage needs

### WebStore (WASM)
- Browser-based storage using IndexedDB
- Async construction, sync operations (fire-and-forget persistence)
- Graceful fallback to memory-only if IndexedDB unavailable
- Automatic hydration from IndexedDB on startup
- Ideal for web applications and browser-based analytics
- Requires the `web` feature flag

**Persistence States:**
- `PersistenceState::Persisted` - IndexedDB available, data survives page refresh
- `PersistenceState::MemoryOnly` - IndexedDB unavailable (private browsing, third-party context), data lost on refresh

When in memory-only mode, consider increasing flush frequency to minimize data loss window.

## Configuration Options

### MemoryConfig
- `write_key`: Identifier for the data source
- `max_items`: Maximum number of items to store (must be > 0)
- `max_fetch_size`: Maximum size in bytes for a single fetch operation (must be ≥ 100)

### DirectoryConfig
- `write_key`: Identifier for the data source
- `storage_location`: Directory path for storing files
- `base_filename`: Base name for generated files
- `max_file_size`: Maximum size in bytes for individual files (must be ≥ 100)

### WebConfig
- `write_key`: Identifier for the data source
- `database_name`: Name of the IndexedDB database (use unique names per store)
- `max_items`: Maximum number of items to store (must be > 0)
- `max_fetch_size`: Maximum size in bytes for a single fetch operation (must be ≥ 100)

## Data Format

All stores produce data in a consistent JSON format:
```json
{
    "batch": [
        // Array of stored items
    ],
    "sentAt": "2024-01-01T00:00:00Z",
    "writeKey": "store-identifier"
}
```

## Thread Safety

TransientDB is designed to be thread-safe and can handle concurrent operations from multiple threads:

- All public methods are thread-safe
- The DirectoryStore uses atomic operations for file management
- Batch operations are atomic
- Safe concurrent append and fetch operations

## Error Handling

All operations that could fail return `Result<T, std::io::Error>`. The library includes comprehensive error handling and recovery mechanisms:

- Safe handling of concurrent access
- Atomic file operations
- Validation of configuration values
- Recovery from interrupted operations
- File system error handling
- JSON parsing error handling

## Testing

The library includes an extensive test suite covering:
- Basic operations
- Concurrent access patterns
- Edge cases and error conditions
- Recovery scenarios
- Performance under load
- File system interactions
- Data format validation
- Configuration validation

Run the tests using:

```bash
# Run all tests (native + WASM)
./test_all.sh

# Native tests only
cargo test

# WASM tests only (requires wasm-pack)
wasm-pack test --headless --chrome --features web

# Or use Firefox/Safari
wasm-pack test --headless --firefox --features web
wasm-pack test --safari --features web  # macOS only, not headless
```

## License

Copyright 2024 Sovran.la, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
