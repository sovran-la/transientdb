//! FileStore example: queue events in a file, send them in batches, and keep whatever
//! could not be sent for a later attempt, across a restart too.
//!
//! FileStore exists only on native targets; in a browser, use WebStore (see `examples/web`).

#[cfg(not(target_arch = "wasm32"))]
fn main() -> transientdb::Result<()> {
	native::main()
}

// `cargo test` builds every example, for wasm32 too.
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
mod native {
	use serde_json::{json, Value};
	use std::path::Path;
	use std::time::{SystemTime, UNIX_EPOCH};
	use transientdb::{Error, FileConfig, FileStore, Result, TransientDB};

	/// The most items one request carries, and the most bytes of item text in it.
	const BATCH_ITEMS: usize = 3;
	const BATCH_BYTES: usize = 16 * 1024;

	fn config(path: &Path) -> FileConfig {
		FileConfig {
			path: path.to_path_buf(),
			// Whose events these are. Opening the file with another id discards what it holds.
			id: "example-write-key".to_string(),
			max_item_size: 1024, // no event over 1 KiB serialized
			max_items: 1000,     // the 1001st append drops the oldest event
		}
	}

	/// Queues an event, and says whether it was stored. One the store can never hold is
	/// reported and dropped; anything else that goes wrong is the caller's to handle.
	fn track(db: &TransientDB, event: Value) -> Result<bool> {
		match db.append(event) {
			Ok(()) => Ok(true),
			Err(e @ (Error::ItemTooLarge { .. } | Error::ItemTooDeep)) => {
				println!("  dropped an event: {e}");
				Ok(false)
			}
			Err(e) => Err(e),
		}
	}

	/// Stands in for an HTTP request that fails while the network is down.
	fn send(body: &str, online: bool) -> std::result::Result<(), String> {
		if online {
			println!("  sent {body}");
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

	/// Sends what is queued, one request per fetch, until nothing is left or a request fails.
	/// Returns how many events were sent.
	fn flush(db: &TransientDB, online: bool) -> Result<usize> {
		let mut sent = 0;
		while let Some(items) = db.fetch(BATCH_ITEMS, BATCH_BYTES)? {
			// The request body is ours to shape; the store only supplies the array.
			let sent_at = SystemTime::now()
				.duration_since(UNIX_EPOCH)
				.map_or(0, |since| since.as_millis() as u64);
			let body = format!("{{\"batch\":{},\"sentAt\":{sent_at}}}", items.json());

			match send(&body, online) {
				Ok(()) => {
					sent += items.len();
					// Delivered: delete them from the file.
					db.remove(items)?;
				}
				Err(reason) => {
					println!("  could not send {}: {reason}", event_count(items.len()));
					// Give them back: a later fetch returns them, in order. Dropping `items`
					// does the same. Stop here, or the next fetch would hand them straight back.
					items.release();
					break;
				}
			}
		}
		Ok(sent)
	}

	pub fn main() -> Result<()> {
		println!("=== File Store Example ===");

		// An app keeps this file in its own data directory, never in a temporary or cache
		// directory, which the OS may empty while the app is suspended. The example cleans up
		// after itself, so it uses a temporary one.
		let dir = tempfile::tempdir()?;
		let path = dir.path().join("queue").join("events.tdb");

		println!("First run, offline:");
		{
			let db = TransientDB::new(FileStore::new(config(&path)));
			for i in 0..5 {
				let page = json!({"event": "page_view", "page": format!("/product/{i}")});
				assert!(track(&db, page)?, "a page view should have been stored");
			}
			let upload = json!({"event": "upload", "data": "x".repeat(2000)});
			assert!(
				!track(&db, upload)?,
				"an event over max_item_size should have been dropped"
			);

			// One store per file: a second store for the same file does nothing while the first
			// one has it. That is a setup mistake, not something to retry.
			let second = TransientDB::new(FileStore::new(config(&path)));
			match second.append(json!({"event": "lost"})) {
				Err(Error::InUse) => println!("  a second store on the file was refused: in use"),
				other => panic!("expected InUse, got {other:?}"),
			}

			let sent = flush(&db, false)?;
			println!("  sent {sent}; still queued: {}", db.has_data());
			assert_eq!(sent, 0, "nothing can be sent while offline");
			// The failed batch was given back, so the next fetch starts with it again.
			let retry = db.fetch(BATCH_ITEMS, BATCH_BYTES)?;
			let first = retry
				.as_ref()
				.map(|items| items.values()[0]["page"].clone());
			assert_eq!(
				first,
				Some(json!("/product/0")),
				"the events of a failed send should be fetched again, first"
			);
			// Dropping an `Items` gives its events back too.
			drop(retry);
			// The app quits here, with five events queued.
		}

		println!("Second run, online:");
		let db = TransientDB::new(FileStore::new(config(&path)));
		println!("  queued from last run: {}", db.has_data());
		assert!(db.has_data(), "the events should have survived the restart");
		let sent = flush(&db, true)?;
		println!("  sent {sent}; still queued: {}", db.has_data());
		assert_eq!(
			sent, 5,
			"every event from the last run should have been sent"
		);
		assert!(!db.has_data(), "nothing should be left once all is sent");

		// With every event removed, the file is back to its 256 byte header.
		let size = std::fs::metadata(&path)?.len();
		println!("  file size: {size} bytes");
		assert_eq!(size, 256, "an empty store's file should be just its header");
		Ok(())
	}
}
