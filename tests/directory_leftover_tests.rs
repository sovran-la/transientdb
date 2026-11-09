//! Files an earlier session left behind, through the public API only: built by hand,
//! then recovered by the first fetch of a `TransientDB` over a `DirectoryStore`.
#![cfg(not(target_arch = "wasm32"))]

use serde_json::{json, Value};
use std::fs;
use std::io::Result;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use transientdb::{DirectoryConfig, DirectoryStore, TransientDB};

const OPENING: &str = "{ \"batch\": [";

fn config(dir: &TempDir) -> DirectoryConfig {
	DirectoryConfig {
		write_key: "leftover-key".to_string(),
		storage_location: dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size: 4096,
		max_file_items: None,
	}
}

fn write(dir: &TempDir, name: &str, content: &str) -> Result<PathBuf> {
	let path = dir.path().join(name);
	fs::write(&path, content)?;
	Ok(path)
}

fn batch(path: &Path) -> Result<Vec<Value>> {
	let json: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
	assert_eq!(json["writeKey"], "leftover-key");
	assert!(json["sentAt"].is_string());
	Ok(json["batch"].as_array().unwrap().clone())
}

/// Every path a validator was given, in order.
type Seen = Arc<Mutex<Vec<PathBuf>>>;

/// A store over `dir` whose validator records every path it is given.
fn recording_db(dir: &TempDir) -> Result<(TransientDB<Vec<PathBuf>>, Seen)> {
	let mut store = DirectoryStore::new(config(dir))?;
	let seen = Arc::new(Mutex::new(Vec::new()));
	let log = seen.clone();
	store.set_file_validator(move |path| {
		serde_json::from_str::<Value>(&fs::read_to_string(path)?)?;
		log.lock().unwrap().push(path.to_path_buf());
		Ok(())
	});
	Ok((TransientDB::new(store), seen))
}

#[test]
fn test_validator_set_after_open_sees_leftovers_at_first_fetch() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let first = write(
		&temp_dir,
		"1-events",
		&format!("{OPENING}{{\"a\":1}},{{\"a\":2}}"),
	)?;
	let second = write(&temp_dir, "2-events", &format!("{OPENING}\"b\""))?;

	let (db, seen) = recording_db(&temp_dir)?;
	assert!(db.has_data());
	assert!(seen.lock().unwrap().is_empty());
	db.append(json!({"c": 3}))?;

	let files = db.fetch(None, None)?.unwrap().data.unwrap();
	assert_eq!(files.len(), 3);
	assert_eq!(batch(&files[0])?, vec![json!({"a": 1}), json!({"a": 2})]);
	assert_eq!(batch(&files[1])?, vec![json!("b")]);
	assert_eq!(batch(&files[2])?, vec![json!({"c": 3})]);

	let seen = seen.lock().unwrap().clone();
	assert_eq!(seen.len(), 3);
	assert!(seen.contains(&first) && seen.contains(&second));
	Ok(())
}
