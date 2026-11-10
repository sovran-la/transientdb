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

#[test]
fn test_rejected_file_is_deleted_and_later_fetches_succeed() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let leftover = write(&temp_dir, "1-events", &format!("{OPENING}\"reject me\""))?;
	let mut config = config(&temp_dir);
	config.max_file_items = Some(1);
	let mut store = DirectoryStore::new(config)?;
	let rejections = Arc::new(Mutex::new(Vec::new()));
	let log = rejections.clone();
	store.set_file_validator(move |path| {
		if fs::read_to_string(path)?.contains("reject me") {
			log.lock().unwrap().push(path.to_path_buf());
			return Err(std::io::Error::other("rejected"));
		}
		Ok(())
	});
	let db = TransientDB::new(store);

	// Rotation rejects the first file; both appends still succeed.
	db.append(json!("reject me"))?;
	db.append(json!("keep me"))?;
	// The fetch rejects the leftover, keeps the open file.
	let result = db.fetch(None, None)?.unwrap();
	let files = result.data.unwrap();
	assert_eq!(files.len(), 1);
	assert_eq!(batch(&files[0])?, vec![json!("keep me")]);
	assert_eq!(rejections.lock().unwrap().len(), 2);
	assert!(!leftover.exists());
	db.remove(&result.removable.unwrap())?;
	assert!(!db.has_data());
	assert_eq!(fs::read_dir(temp_dir.path())?.count(), 0);

	// The store carries on normally.
	db.append(json!("later"))?;
	let files = db.fetch(None, None)?.unwrap().data.unwrap();
	assert_eq!(batch(&files[0])?, vec![json!("later")]);
	Ok(())
}

#[test]
fn test_leftover_cut_inside_an_item_is_salvaged() -> Result<()> {
	let temp_dir = TempDir::new()?;
	// Killed partway through the third item, which holds a "}" and a comma.
	let cut = write(
		&temp_dir,
		"1-events",
		&format!("{OPENING}{{\"n\":1,\"s\":\"a,}}\"}},[2,{{\"x\":\"]\"}}],{{\"n\":3,\"s\":\"b,}}"),
	)?;
	let unreadable = write(&temp_dir, "2-events", "garbage, not a data file")?;

	let (db, seen) = recording_db(&temp_dir)?;
	let result = db.fetch(None, None)?.unwrap();
	let files = result.data.unwrap();
	assert_eq!(files.len(), 1);
	assert_eq!(
		batch(&files[0])?,
		vec![json!({"n": 1, "s": "a,}"}), json!([2, {"x": "]"}])]
	);
	assert!(fs::read_to_string(&files[0])?.starts_with(&format!(
		"{OPENING}{{\"n\":1,\"s\":\"a,}}\"}},[2,{{\"x\":\"]\"}}]]"
	)));
	assert!(!cut.exists());
	// The validator saw the salvaged file; the unreadable one is deleted unvalidated.
	assert_eq!(seen.lock().unwrap().clone(), vec![cut]);
	assert!(!unreadable.exists());
	db.remove(&result.removable.unwrap())?;
	assert!(db.fetch(None, None)?.is_none());
	assert!(!db.has_data());
	assert_eq!(fs::read_dir(temp_dir.path())?.count(), 0);
	Ok(())
}
