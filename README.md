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

`fetch()` never removes anything, in any store: it returns the batch and its `removable`
items, and the data stays in the store until you pass `removable` to `remove()`. Fetching
again before removing returns the same items. Remove a batch only once it has been
processed (for example, after the upload succeeded), so a failure never loses data.

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
    max_fetch_items: None,
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
    // The items stay in the store until removed
    if let Some(removable) = result.removable {
        db.remove(&removable)?;
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
    max_file_items: None,
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

### File Validation (DirectoryStore)

A validator set with `set_file_validator` is your own code. It is given the path of each
finished file before the file is marked ready (renamed with `.temp`), and it runs:

- when `append` finishes the open file because the next item would not fit (rotation),
- when `fetch` finishes the open file,
- when the first `fetch` of a session recovers or salvages files an earlier session left
  unfinished. Recovery happens there, not in `DirectoryStore::new`, so a validator set
  right after `new` sees those files too.

Returning `Ok(())` accepts the file. **Returning an error means "delete this file and
every item in it".** The store deletes it, and the operation that triggered validation
continues and succeeds: `append` still writes its item, to a fresh file, and `fetch`
returns the other finished files, or nothing. A rejection is never reported through the
return value of `append` or `fetch`.

The validator is therefore the one place to observe a rejection. If you want to record,
count, report, or keep a copy of a rejected file, do it inside the validator, before you
return the error: once it returns, the file is gone. The store's own notice is a single
line on stderr, which many targets (mobile apps in particular) never surface, so do not
rely on it.

```rust
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

let rejected = Arc::new(AtomicUsize::new(0));
let counter = rejected.clone();
store.set_file_validator(move |path| {
    let content = std::fs::read_to_string(path)?;
    if serde_json::from_str::<serde_json::Value>(&content).is_err() {
        counter.fetch_add(1, Ordering::SeqCst); // record it now; the file is about to go
        return Err(std::io::Error::other("not valid JSON"));
    }
    Ok(())
});
```

### Durability (DirectoryStore)

Files are not fsynced, so durability across a power loss or OS crash is not guaranteed.
What was written is salvaged by the next session's first `fetch`: a file cut short keeps
every complete item, byte for byte and in order, and loses whatever follows the last one.
A leftover with no complete item is deleted, and so is one of the store's files that does
not even start like a data file (empty, or cut inside its opening): nothing in it can be
read, so it could never be delivered. Files not named for the store are never touched. A
write that fails while the store is running (disk full, an I/O error) is rolled back, so
the failed item leaves nothing behind and the next append continues the same file. If
finishing a file fails (writing its closing, or renaming it), the `append` or `fetch` that
tried returns the error, and the next `fetch` recovers that file with all its items.

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
    max_fetch_items: None,
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
- Requires explicit cleanup via remove(); fetch does not remove returned items

### DirectoryStore
- Stores data in rotating files in a specified directory
- Automatic file management and rotation; no finished file exceeds `max_file_size` bytes
- Returns paths to completed files
- Supports custom file validation
- Requires explicit cleanup via remove()
- Ideal for larger datasets and persistent storage needs

### WebStore (WASM)
- Browser-based storage using IndexedDB
- Async construction, sync operations (fire-and-forget persistence)
- Graceful fallback to memory-only if IndexedDB unavailable
- Automatic hydration from IndexedDB on startup, in append order
- Stores any JSON value (objects, arrays, strings, numbers, booleans, null) unmodified
- IndexedDB writes are issued in call order, so a `remove()` or `reset()` right after an
  `append()` is never undone by the append landing late
- A database written by transientdb 0.2.x uses an older record format; it is emptied the
  first time this version opens it (old records are discarded, not migrated)
- Ideal for web applications and browser-based analytics
- Requires the `web` feature flag

**Persistence States:**
- `PersistenceState::Persisted` - IndexedDB available, data survives page refresh
- `PersistenceState::MemoryOnly` - IndexedDB unavailable (private browsing, third-party context), data lost on refresh

When in memory-only mode, consider increasing flush frequency to minimize data loss window.

## Configuration Options

### MemoryConfig
- `write_key`: Identifier for the data source
- `max_items`: Total store capacity; oldest items are dropped beyond this (must be > 0)
- `max_fetch_size`: Hard ceiling in bytes on one fetched batch, envelope included (must be ≥ 100; see [Byte limits](#byte-limits))
- `max_fetch_items`: Maximum number of items one fetch returns, or `None` for no limit (must not be `Some(0)`)

### DirectoryConfig
- `write_key`: Identifier for the data source
- `storage_location`: Directory path for storing files
- `base_filename`: Base name for generated files, which are named `{index}-{base_filename}`. Must be a plain file name: not empty, not `.` or `..`, no `/`, `\` or NUL byte (on every platform), and not ending in `.temp`. Dots, spaces, hyphens, digits and non-ASCII characters are fine. The index counts up from one above the highest index on disk and never wraps: if index 4294967295 is taken, an `append` that needs a new file returns an error, and the existing files can still be fetched
- `max_file_size`: Hard ceiling in bytes on each finished file, envelope included (must be ≥ 100; see [Byte limits](#byte-limits))
- `max_file_items`: Maximum number of items in one file, or `None` for no limit (must not be `Some(0)`)

### WebConfig
- `write_key`: Identifier for the data source
- `database_name`: Name of the IndexedDB database (use unique names per store)
- `max_items`: Total store capacity; oldest items are dropped beyond this (must be > 0)
- `max_fetch_size`: Hard ceiling in bytes on one fetched batch, envelope included (must be ≥ 100; see [Byte limits](#byte-limits))
- `max_fetch_items`: Maximum number of items one fetch returns, or `None` for no limit (must not be `Some(0)`)

### Byte limits

Some consumers send each batch as one request body to an API that rejects bodies over a
fixed size. Every store's byte limit is a hard ceiling on what it hands back, measured in
bytes (not characters) and counting the whole JSON batch: the envelope (`batch`, `sentAt`,
`writeKey`), every item, and the commas between items. An exact fit is allowed.

- **DirectoryStore**: no finished file is ever larger than `max_file_size` on disk. Before
  each `append` the store computes the size the open file would have once finished with the
  item in it; if that exceeds `max_file_size`, it finishes the open file and the item starts
  a new one. A file also rotates when it holds `max_file_items` items, whichever comes first.
- **MemoryStore and WebStore**: the batch one `fetch` returns, serialized with
  `serde_json::to_string`, is never longer than `max_bytes` if given, otherwise
  `max_fetch_size`. A `max_bytes` too small for the oldest item returns `Ok(None)`; a fetch at
  the configured `max_fetch_size` always returns it. The fetch also stops at `count` or
  `max_fetch_items` items.

An item too large to fit by itself under the configured limit (`max_file_size` or
`max_fetch_size`) can never be delivered, so `append` rejects it with
`std::io::ErrorKind::InvalidInput` and a message giving the item's size and the largest size
that fits. The rejected item is not stored, and the store is otherwise unchanged:
DirectoryStore does not rotate or create a file, and WebStore issues no IndexedDB write.

`append` also rejects, with `InvalidInput` and the same guarantees, an item nested more than
125 levels deep in arrays and objects (each `[]` or `{}` is one level; scalars add none).
`serde_json` cannot parse anything nested deeper inside a batch, so such an item could not
be recovered from a file, hydrated from IndexedDB, or read by the consumer. The check does
not recurse, so even an absurdly deep value is rejected without overflowing the stack.

```rust
match db.append(event) {
    Ok(()) => {}
    Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => eprintln!("dropped event: {e}"),
    Err(e) => return Err(e),
}
```

WebStore may hold items persisted under a larger `max_fetch_size`. Hydration loads them as usual; when one reaches the head of the queue and
cannot fit under `max_fetch_size`, `fetch` drops it, deletes it from IndexedDB, logs a
`console.warn`, and continues with the next item.

DirectoryStore may likewise find unfinished files left by a session with a larger
`max_file_size`. Recovery cuts each one down to the leading complete items that fit under
the current `max_file_size` once closed, dropping the rest (with one line on stderr giving
the bytes kept of the total), and deletes it if not even its first item fits. That holds
for a file the earlier session had already closed but not renamed, too: one at or under the
current limit is renamed untouched. Files an earlier session already finished (`.temp`) are
fetched as they are.

Because the envelope counts, the limit must leave room for at least a 1-byte item after it;
the envelope grows with `write_key`'s length. Constructing a store whose envelope alone fills
the limit panics.

### Item limits per batch

Some consumers send each batch as one request and need a hard cap on items per batch.
Each store can enforce one:

- `DirectoryConfig::max_file_items` caps the items in each file. When the open file holds
  that many items, the next `append` finishes it and starts a new one. It is independent of
  `max_file_size`; whichever is reached first rotates the file.

  ```rust
  let config = DirectoryConfig {
      write_key: "my-app".into(),
      storage_location: PathBuf::from("/tmp/events"),
      base_filename: "batch".into(),
      max_file_size: 1024 * 1024,
      max_file_items: Some(8), // no file ever holds more than 8 items
  };
  ```

- `MemoryConfig::max_fetch_items` and `WebConfig::max_fetch_items` cap the items returned by
  one `fetch`. If `fetch` is also given a `count`, the smaller of the two applies; a larger
  `count` is clamped silently. The [byte limit](#byte-limits) still applies. This is separate from
  `max_items`, which is the store's total capacity.

  ```rust
  let config = MemoryConfig {
      write_key: "my-app".into(),
      max_items: 1000,
      max_fetch_size: 1024 * 1024,
      max_fetch_items: Some(8), // every fetch returns at most 8 items
  };
  ```

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
