# TransientDB

TransientDB is a lightweight, thread-safe queue for transient data in Rust applications: events, logs, or anything else that is collected now and delivered later. You append JSON items, fetch the oldest of them in batches, and remove each batch once it has been delivered. Whatever could not be delivered stays queued for the next attempt. It is ideal for analytics and telemetry pipelines, offline buffering, and any producer that must not lose data while its consumer is unreachable.

## Features

- **Three Storage Backends, One Interface**
    - In-memory storage, for the fastest queue that need not outlive the process
    - Single-file storage on native targets, safe against a kill at any point
    - Browser-based IndexedDB storage for WASM targets, with an in-memory fallback
    - One trait, so code written against one store works with every store

- **Delivery Without Bookkeeping**
    - Fetch hands out the oldest items and keeps them from every other fetch until you remove or release them
    - Several requests can be in flight at once, each with its own items
    - A failed or abandoned delivery gives its items back automatically
    - Items come back as one JSON array, as parsed values, or as each item's own text

- **Bounded by Design**
    - Every store takes the same two limits: the largest item and the most items
    - Every fetch says how many items and how many bytes it wants
    - A full store drops its oldest item, so appending never fails for lack of room

- **Thread-Safe Operations**
    - `TransientDB` shares any store between threads
    - Fetch on one thread, remove on another

## Installation

Add TransientDB to your `Cargo.toml`:

```toml
[dependencies]
transientdb = "0.4.0"
```

For WASM/browser targets, enable the `web` feature:

```toml
[target.'cfg(target_arch = "wasm32")'.dependencies]
transientdb = { version = "0.4.0", features = ["web"] }
```

> **Note:** `WebStore` exists only on `wasm32` with the `web` feature. `FileStore` exists only on native targets. `MemoryStore` is everywhere.

## Core Types

### DataStore Trait
The interface every store implements:

```rust
use serde_json::Value;
use transientdb::{Items, Result};

pub trait DataStore {
    fn append(&mut self, data: Value) -> Result<()>;
    fn fetch(&mut self, count: usize, max_bytes: usize) -> Result<Option<Items>>;
    fn remove(&mut self, items: Items) -> Result<()>;
    fn reset(&mut self);
    fn has_data(&self) -> bool;
}
```

- `append()`: Add an item to the end of the queue
- `fetch()`: Hand out the oldest items, within a count and a byte limit
- `remove()`: Delete the items a fetch handed out, once they are delivered
- `reset()`: Delete every item
- `has_data()`: Whether a fetch could return something now

`remove()` acts only on an `Items` the same store instance handed out. One from another store, or from before a `reset()`, matches nothing: it is ignored, which is not an error.

### TransientDB
A wrapper that puts any store behind a mutex, so threads can share it. It has the same five operations, taking `&self`. On native targets it is `Send` and `Sync`.

### Items
What a fetch hands out: one or more items, oldest first, and the store's claim on them. An `Items` is never empty. It offers:

- `len()`: how many items
- `byte_len()`: the total bytes of the items' text, as counted against `max_bytes`
- `json()`: one JSON array holding every item, `[{...},{...}]`, built by joining the stored text without parsing anything. Its length is `byte_len()`, plus one comma between each pair of items, plus 2
- `values()`: every item parsed as a `serde_json::Value`
- `iter()`: each item's stored text as it is, the JSON serialized at append
- `release()`: give the items back for a later fetch

An item is checked when it is appended; what is fetched is what was stored, and is not parsed again. None of these methods can fail or panic. `json()` and `iter()` pass the stored text through as it is, and `values()` gives an item whose text does not parse (only possible if something else wrote to the store's storage, such as another script writing to a `WebStore`'s database) as a JSON string of that text. None of them wraps the items in anything either. The request body around them (a key, other fields, a timestamp) is yours to build.

### Error
Every fallible operation returns `transientdb::Result<T>`, whose error is `transientdb::Error`. See [Error Handling](#error-handling).

## Usage

### The delivery cycle

Every store is used the same way:

1. `fetch(count, max_bytes)` the oldest items.
2. Put `items.json()` (or `values()`, or `iter()`) in your request.
3. If the request succeeds, `remove(items)`. If it fails, drop the `Items` or call `release()`: its items are returned, in their original order, by a later fetch.

A fetched item is not returned by another fetch until its `Items` is released, so consecutive fetches never hand out the same item. A caller that sends several requests at once calls `fetch` once per request and gets different items each time. Because dropping an `Items` releases it, an early return or a panic on the send path cannot hold items back forever.

```rust
use transientdb::{Result, TransientDB};

/// Sends what is queued, one request per fetch, until nothing is left or a request fails.
fn flush(db: &TransientDB, send: impl Fn(&str) -> bool) -> Result<()> {
    while let Some(items) = db.fetch(100, 500 * 1024)? {
        let body = format!("{{\"batch\":{}}}", items.json());
        if send(&body) {
            db.remove(items)?;
        } else {
            // Released for a later flush. Stop now: the next fetch would return them again.
            items.release();
            break;
        }
    }
    Ok(())
}
```

Here `send` returns `false` only for a failure worth retrying: a network error, or a server error such as a 5xx. If the server rejects a batch as malformed (a 400, say), `remove` it as if it had been delivered. Released, it would be fetched, sent and rejected again, forever, and nothing behind it would get through.

### Memory Store Example

```rust
use serde_json::json;
use transientdb::{MemoryConfig, MemoryStore, TransientDB};

fn main() -> transientdb::Result<()> {
    // Configure an in-memory store
    let config = MemoryConfig {
        max_item_size: 8 * 1024, // no item over 8 KiB serialized
        max_items: 1000,         // the 1001st append drops the oldest item
    };

    // Create the store
    let db = TransientDB::new(MemoryStore::new(config));

    // Append data
    db.append(json!({"event": "user_login", "user_id": 123}))?;
    db.append(json!({"event": "page_view", "page": "/home"}))?;

    // Fetch up to 100 items, at most 64 KiB of them
    if let Some(items) = db.fetch(100, 64 * 1024)? {
        for item in items.values() {
            println!("Processing event: {}", item["event"]);
        }
        // The items stay in the store until removed
        db.remove(items)?;
    }
    assert!(!db.has_data());
    Ok(())
}
```

### File Store Example

```rust
use serde_json::json;
use std::path::Path;
use transientdb::{FileConfig, FileStore, TransientDB};

/// Opens the event queue in `data_dir`, the app's own data directory
/// (not a temporary or cache directory; see "Where to put the file" below).
fn open_queue(data_dir: &Path) -> TransientDB {
    TransientDB::new(FileStore::new(FileConfig {
        path: data_dir.join("events.tdb"), // parent directory created if missing; see below
        id: "my-write-key".into(),         // whose data this is
        max_item_size: 8 * 1024,
        max_items: 2560,
    }))
}

fn run(data_dir: &Path) -> transientdb::Result<()> {
    let db = open_queue(data_dir);
    db.append(json!({"event": "purchase", "amount": 99.99}))?;
    drop(db); // the process ends

    // A later run finds the item where the last one left it
    let db = open_queue(data_dir);
    let items = db.fetch(100, 500 * 1024)?.expect("one item is stored");
    assert_eq!(items.values()[0]["amount"], 99.99);
    db.remove(items)?;
    Ok(())
}
```

### Web Store Example (WASM)

```rust
use serde_json::json;
use std::time::Duration;
use transientdb::{PersistenceState, TransientDB, WebConfig, WebStore};

/// Opens the queue, and says how often to flush it.
async fn start() -> transientdb::Result<(TransientDB, Duration)> {
    // Configure a browser-based store
    let config = WebConfig {
        database_name: "my-app-events".into(),
        max_item_size: 8 * 1024,
        max_items: 1000,
        // Take over what closed tabs left unsent this often (and once now)
        orphan_interval: Duration::from_secs(30),
    };

    // Create the store (async - opens IndexedDB, takes this tab's lock, and
    // adopts what earlier tabs and page loads left unsent)
    let store = WebStore::new(config).await;

    // Check persistence state - IndexedDB may be unavailable
    // in private browsing or third-party iframe contexts.
    // This is the state at startup; it can drop to MemoryOnly later
    // (see "Persistence States" below).
    let flush_every = match store.persistence_state() {
        // Full persistence available
        PersistenceState::Persisted => Duration::from_secs(30),
        // Degraded mode - flush more frequently
        PersistenceState::MemoryOnly => Duration::from_secs(5),
    };

    let db = TransientDB::new(store);
    db.append(json!({"event": "page_view", "url": "/home"}))?;
    Ok((db, flush_every))
}
```

> **See also:** [`examples/memory_example.rs`](examples/memory_example.rs) and [`examples/file_example.rs`](examples/file_example.rs) (`cargo run --example file_example`) show the whole cycle, a failed send included. The [`examples/web/`](examples/web/) directory contains a complete, runnable WASM example with HTML and build instructions.

## Limits

Every store takes the same two limits, so an item one store accepts, every store accepts. Both are required and neither may be 0; `new` panics otherwise.

- **`max_item_size`**: the largest item, in bytes of its compact `serde_json` text. `append` rejects a larger item with `Error::ItemTooLarge` and stores nothing.
- **`max_items`**: the most items the store holds at once. When it is full, `append` drops the oldest item to make room, whether or not that item has been fetched. This is not configurable, and `append` never fails for it. An `Items` holding a dropped item still removes or releases the rest normally.

`append` also rejects, with `Error::ItemTooDeep`, an item nested more than 125 levels deep in arrays and objects (each `[]` or `{}` is one level). `serde_json` could not read such an item back once it is inside a request body. The check does not recurse, so even an absurdly deep value is rejected without overflowing the stack.

Every **fetch** states its own limits, `count` and `max_bytes`; there is no default:

- It returns at most `count` items, whose text totals at most `max_bytes` bytes. An exact fit is allowed.
- `max_bytes` counts item bytes only. Whatever you put around or between the items is yours to account for: with `json()`, that is one comma between each pair of items and the two brackets, plus your own request body.
- Items are taken in order, stopping at the first one that does not fit. A smaller item behind it does not jump the queue.
- It returns `Ok(None)` when there is nothing to hand over: nothing is waiting, `count` is 0, or the oldest waiting item alone is larger than `max_bytes`. That is not an error.

For a service that rejects request bodies over some size, pass a `max_bytes` that leaves room for your wrapper, and a `max_item_size` no larger than that.

## Storage Implementations

### MemoryStore
- Stores each item as its JSON text, in memory, in append order
- Ideal for high-throughput, temporary data storage
- Nothing survives the store: dropping it discards every item
- Identical items are distinct entries: `remove` deletes exactly the items a fetch handed out

**MemoryConfig:**
- `max_item_size`: Largest item, in bytes; see [Limits](#limits)
- `max_items`: Most items held; past it the oldest is dropped

### FileStore (native only)
- Keeps every item in one file, so items survive the process and a kill at any point
- No recovery pass, no compaction, no background work, no file lock
- Ideal for apps that must not lose queued data when they are killed or suspended

**FileConfig:**
- `path`: The store's file. Its parent directory is created if missing, unless the path goes through a symbolic link to a missing directory: then `append`, `fetch` and `remove` return `Error::Io` (kind `InvalidInput`) naming the link, `has_data` is false and `reset` does nothing, and nothing is created, until that directory is back
- `id`: Whose data this is (a write key, for example), 1 to 224 bytes. It is recorded in the file, and a file recorded under a different id is discarded when opened
- `max_item_size`: Largest item, in bytes. This is the size of every slot in the file, so changing it discards the file
- `max_items`: Most items held. It can change between runs: raised, the file may grow further; lowered, the oldest items beyond it are dropped when the file is opened

**The file.** A 256 byte header, then slots, all `32 + max_item_size` bytes. Each slot holds one item and a 32 byte entry with the item's length, sequence number and CRC-32. Order is by sequence number, not by position. A removed item's slot is reused by a later append, lowest slot first, and the file grows by one slot only when none is free, so the file is as large as the queue has been deep since the file was last emptied. That is never more than `256 + (32 + max_item_size) * max_items` bytes, taking the largest `max_items` the file has been opened with since then: lowering `max_items` drops items, but does not shrink the file. With `max_item_size = 8192` and `max_items = 2560` that is 21,053,696 bytes, but a queue that is never more than 50 items deep keeps the file at 411,456. When the last item is removed, the file is cut back to its header. Nothing in the file is ever moved, rewritten or renamed.

A file that does not match the config (another id or `max_item_size`, another format version, a damaged header, or not a FileStore file at all) is deleted with everything in it, logged, and created again. `new` does not fail for this. A damaged item, one that fails its CRC or is not UTF-8, is caught when it is fetched: it is deleted, logged and skipped, so it can never be fetched forever. FileStore logs one line on stderr for each of these.

**One store per file.** A file is used by one store at a time. Within a process this is enforced: a second `FileStore` for a file another one is using, whether its path is spelled the same way, relative, through `..` or through a symbolic link, is created, but leaves the file alone. Its `append`, `fetch` and `remove` return `Error::InUse`, `has_data` is false, and `reset` does nothing, until the first store is dropped; its next call then opens the file and carries on. A hard link is a different path to the file, and is not detected. Across processes it is a rule for you, not enforced: an app and its extension, say, must not open the same file. If they do, items can be lost or sent twice (never torn: every fetched item's CRC is checked).

**Where to put the file.** The location is your choice, and it matters:
- Not a temporary or cache directory. The OS may delete those while the app is suspended, and writes to a deleted file succeed and go nowhere.
- Preferably the app's own container, rather than one shared with another app or an extension.
- With a data-protection class that allows access whenever the app can run. While the file cannot be read or written, calls return `Error::Io` and change nothing; they work again once it can.

**Durability.** FileStore does not buffer writes in user space: when `append` returns `Ok`, the item is with the OS, and a process killed at any point afterwards, or frozen by a mobile OS and never resumed, loses nothing. Nothing is ever synced, so a power loss or OS crash may lose recent appends and removes; a torn write is caught by its CRC, and costs at most that one item. Which items are fetched is kept in memory only: after a restart every stored item can be fetched again, including any that were fetched but not yet removed. Delivery is therefore at least once: a process killed between a successful send and its `remove` sends those items again.

### WebStore (WASM)
- Browser-based storage using IndexedDB, behind an in-memory queue
- Async construction, sync operations: every write to IndexedDB is fire-and-forget, issued during the call that causes it, so writes land in call order
- Each tab keeps, sends and removes its own items; items left by a tab that is gone are adopted by a live one (see [Several tabs](#several-tabs))
- Items are persisted as JSON text, so after a reload an integer above 2^53 or a float such as `42.0` comes back exactly as appended
- Which items are fetched is kept in memory only, so items adopted after a reload can all be fetched again
- A failed IndexedDB write is not an error: it is logged, and the item is still queued in memory
- `remove` only counts an `Items` this instance handed out, even if another `WebStore` is open on the same database
- A database in a record format this version does not write is emptied, not converted, when a store first opens it. An open blocked by another page (see [Database upgrades](#database-upgrades)) leaves it as it was, for a later open
- Needs a secure context (https, or `localhost`) to persist: it relies on Web Locks
- Ideal for web applications and browser-based analytics
- Requires the `web` feature flag

**WebConfig:**
- `database_name`: Name of the IndexedDB database (use unique names per store; every tab of a page opens the same one)
- `max_item_size`: Largest item, in bytes; see [Limits](#limits)
- `max_items`: Most items one tab holds, its own and those it adopted; past it the oldest is dropped, in memory and in IndexedDB. It is a soft limit: the store trims to it, but can hold more for a moment, while it adopts orphans (until the trim that ends the pass), or until the next append if the pass is cut short by another page's database upgrade
- `orphan_interval`: How often the store looks for items left by tabs that are gone, besides once when it opens. `Duration::from_secs(30)` is a good default. Must not be zero

**Persistence States:**
- `PersistenceState::Persisted` - IndexedDB available, data survives page refresh
- `PersistenceState::MemoryOnly` - data lost on refresh. The store is in this mode when IndexedDB is unavailable (private browsing, third-party context, storage blocked) or unreadable, when Web Locks are unavailable (not a secure context, or a browser from before 2022), when another page holds the database open at an older version, and from the moment another page asks for a newer version of the database or deletes it. A memory-only store writes nothing and adopts nothing

The store falls back to memory-only mode by itself and logs a warning to the console; it works the same way otherwise. `persistence_state()` and `is_persisted()` tell you which mode it is in now. That can change from `Persisted` to `MemoryOnly` while the store is in use, but never back, so a choice made from it at startup can go stale. A `TransientDB` does not give its store back, so an app that adjusts to the mode as it changes uses the `WebStore` itself (it implements `DataStore`) and checks again before each flush. When in memory-only mode, consider increasing flush frequency to minimize data loss window.

#### Several tabs

Every tab of a page opens the same database. Each `WebStore` instance is a tab of its own: it gets a random tab id when it is created (a reloaded page is a new tab), keeps its records under that id, and fetches, sends and removes only its own items. Live tabs never see each other's items, so two live tabs never send the same item, and `max_items` is per tab. Delivery is still at least once: a send in flight when a page is cached (see below) or dies can be sent again once its items are adopted, so the server should dedupe by an id each item carries (Segment's `messageId`, for example).

Items left behind by a tab that is gone (closed, crashed, discarded by the browser, reloaded) are *orphans*. A live tab adopts them: it moves them under its own id, behind the items it already holds, and sends them as its own. A store adopts once when it opens, before `new` returns (so a reloaded page picks up what it left at once), and then every `orphan_interval` until it is dropped. Adopted items count toward the adopting tab's `max_items`; past it the oldest are dropped and logged. A record that is not an item, or an item larger than `max_item_size`, is discarded and logged when it is adopted. Items from different tabs have no order relative to each other.

Each store holds the [Web Lock](https://developer.mozilla.org/docs/Web/API/Web_Locks_API) `transientdb:{database_name}:tab:{tab id}` for as long as it lives, and the browser releases it when the page goes away. Adopting a tab's items takes its lock first, so a live tab's items are never adopted, and two tabs never adopt the same items. Dropping a store releases its lock, so its items become orphans.

**Back/forward cache.** A page restored from the back/forward cache starts over as a new tab: a new tab id and lock, its old lock released, and its queue reset in memory, so an `Items` fetched before matches nothing. It then adopts its own earlier items like any orphan's, unless another tab adopted them while the page was cached, in which case that tab sends them. A send in flight when the page was cached can therefore be sent again (at least once). Measured in Firefox 160, a page holding a Web Lock is never cached, so a page with a persisted `WebStore` misses Firefox's back/forward cache. Measured in Chrome 157, a cached page keeps its lock, and is evicted when another tab requests it to adopt its items. Measured in Safari 26.2, caching a page releases its locks, so another tab can adopt its items while it is cached; starting over is what keeps the restored page from sending them too.

`reset()` clears this tab's items and every record in the database, every tab's, as for a logout or an opt-out. Other live tabs keep what they already hold in memory.

#### Flushing when the page closes

A send started while the page is closing cannot report back, so the store cannot learn whether it succeeded. For the last batch, remove the items *before* sending them:

```rust
let body = items.json();
store.remove(items)?; // the IndexedDB delete is issued during this call
navigator.send_beacon_with_opt_str(url, Some(&body))?;
```

If the beacon fails, that batch is lost (at most once, the last batch only). If the delete does not commit before the page is gone, the batch is adopted by another tab or the reloaded page and sent again. Do the main flush on `visibilitychange` to `hidden`, while the page is still alive, and keep this for what is left.

#### Database upgrades

Upgrading a database waits for every other connection to it to close. A store never waits long for that, and never makes another page wait:

- If another page holds the database open at an older version and does not close it (a page loaded before a deploy brought a newer version of this store), `WebStore::new` gives up after about a second, so it can take that long. The store starts in memory-only mode and logs a warning. The database is left as it was, to be upgraded by the next store opened once that page is gone.
- If another page asks for a newer version of the database, or deletes it, this store closes its connection at once and carries on in memory-only mode, with every item it holds, and logs a warning. Its items in IndexedDB stay there for the newer version to deal with, and nothing it does afterwards is written there.

## Thread Safety

`TransientDB` locks its store for the duration of each call, so any number of threads can append, fetch and remove at once. An `Items` never needs that lock: on native targets it is `Send`, so it can be fetched on one thread and removed on another, and dropping or releasing it works anywhere, even while another thread is inside a call, or after the store itself is gone.

```rust
use serde_json::json;
use std::sync::Arc;
use std::thread;
use transientdb::{MemoryConfig, MemoryStore, TransientDB};

fn main() {
    let db = Arc::new(TransientDB::new(MemoryStore::new(MemoryConfig {
        max_item_size: 1024,
        max_items: 100,
    })));
    db.append(json!({"event": "open"})).unwrap();

    let items = db.fetch(10, 4096).unwrap().unwrap();
    let worker = {
        let db = Arc::clone(&db);
        // Deliver on another thread, then remove from there.
        thread::spawn(move || db.remove(items).unwrap())
    };
    worker.join().unwrap();
    assert!(!db.has_data());
}
```

## Error Handling

Only the stores' own operations return a `Result`; nothing on `Items` does. `Error` is `#[non_exhaustive]`, and each variant says what to do:

- `ItemTooLarge { size, max }`: `append` only. The item is larger than `max_item_size` and will never be stored. Drop it or report it.
- `ItemTooDeep`: `append` only. The item is nested too deeply and will never be stored. Drop it or report it.
- `InUse`: FileStore only, from `append`, `fetch` and `remove`. Another store in this process is using the file, so this one did nothing. Two stores were made for one file: a mistake in your setup, not something to retry. Use the store that has it.
- `Io(std::io::Error)`: the storage failed. Nothing changed, except as noted below, and the store is ready for the next call. This is the only one worth retrying, unless the store has used up its sequence numbers (below).

What `Io` means for FileStore, operation by operation:
- `append`: the item is not stored. Every other item is as it was, except that a failed write while dropping the oldest item (store full) may have dropped it. Also returned, writing nothing, when a damaged file has used up every sequence number; that lasts until `reset()`, or until the store is emptied and opened again, so retrying does not help.
- `fetch`: nothing is handed out; a later fetch returns the same items.
- `remove`: every item is tried; those that could not be deleted stay stored and are fetched again.

MemoryStore never returns `InUse` or `Io`: each item takes the next of its `u64` sequence numbers, which at one append per nanosecond last more than 580 years. WebStore never returns `InUse`, and returns `Io`, storing nothing, only once the instance has used up its sequence numbers: after 2^53 - 1 appends and adopted items (the most an IndexedDB number key holds exactly). Each instance numbers its own items from 1, and `reset()` does not rewind them, so retrying does not help; a new instance, as on a page reload, starts again.

Things a store handles itself are not errors: a full store, a FileStore file that does not match, a damaged item, an unavailable IndexedDB. FileStore logs each of these, every item a full store drops included. MemoryStore and WebStore drop the oldest item without logging it; WebStore logs what it discards or drops when it adopts orphaned items, and each fallback to memory-only mode.

```rust
use serde_json::Value;
use transientdb::{Error, TransientDB};

fn track(db: &TransientDB, event: Value) {
    match db.append(event) {
        Ok(()) => {}
        // This event will never be stored: report it and move on.
        Err(e @ (Error::ItemTooLarge { .. } | Error::ItemTooDeep)) => eprintln!("dropped event: {e}"),
        // Two stores on one file: fix the setup.
        Err(Error::InUse) => panic!("two stores were made for one file"),
        // The storage failed and nothing changed: try again later.
        Err(e) => eprintln!("could not queue event, will retry: {e}"),
    }
}
```

## Testing

The library includes an extensive test suite covering:
- Every operation of every store, against one shared set of checks
- Concurrent access patterns
- Edge cases, boundaries and error conditions
- FileStore under a kill at every byte of a write, injected I/O errors, and damaged or mismatched files
- Performance under load
- WebStore in headless Chrome and Firefox, several tabs on one database included: orphans adopted, racing adopters, live tabs left alone, and a reset or a drop in the middle of an adoption

Run the tests using:

```bash
# Run all tests (native + WASM), and the examples
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
