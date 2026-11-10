//! A file the validator rejects is deleted with every item in it, and the operation that
//! finished it carries on. Only a validator rejection deletes.

use super::support::*;
use super::*;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Rejects every file whose content contains `marker`; records every verdict.
fn reject_containing(
	store: &mut DirectoryStore,
	marker: &'static str,
) -> Arc<Mutex<Vec<(PathBuf, bool)>>> {
	let verdicts = Arc::new(Mutex::new(Vec::new()));
	let log = verdicts.clone();
	store.set_file_validator(move |path| {
		let reject = fs::read_to_string(path)?.contains(marker);
		log.lock().unwrap().push((path.to_path_buf(), !reject));
		if reject {
			Err(io::Error::other("rejected"))
		} else {
			Ok(())
		}
	});
	verdicts
}

fn one_per_file(dir: &TempDir, items: usize) -> Result<DirectoryStore> {
	let mut config = config(dir, 1024, with_source(KEY));
	config.max_file_items = Some(items);
	DirectoryStore::new(config)
}

fn rejected_paths(verdicts: &Arc<Mutex<Vec<(PathBuf, bool)>>>) -> Vec<PathBuf> {
	let verdicts = verdicts.lock().unwrap();
	verdicts
		.iter()
		.filter(|(_, ok)| !ok)
		.map(|(p, _)| p.clone())
		.collect()
}

fn gone(path: &Path) -> bool {
	!path.exists() && !DirectoryStore::finished_path(path).exists()
}

#[test]
fn test_rejection_on_rotation_keeps_the_new_item() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = one_per_file(&temp_dir, 1)?;
	let verdicts = reject_containing(&mut store, "bad");

	store.append(json!("bad"))?;
	let rejected = store.current_path.clone().unwrap();
	// Rotation finishes and rejects the "bad" file; the append itself succeeds.
	store.append(json!("good"))?;
	assert_eq!(rejected_paths(&verdicts), vec![rejected.clone()]);
	assert!(gone(&rejected));
	let open = store.current_path.clone().unwrap();
	assert_ne!(open, rejected);
	assert_eq!(fs::read(&open)?, unclosed(&[json!("good")]));
	assert_eq!((store.current_items, store.current_size), (1, OPENING + 6));

	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 1024), vec![json!("good")]);
	assert_eq!(files_on_disk(&temp_dir)?.len(), 1);
	Ok(())
}

#[test]
fn test_rejection_of_the_open_file_at_fetch() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = one_per_file(&temp_dir, 2)?;
	let verdicts = reject_containing(&mut store, "bad");
	let kept = vec![json!("a"), json!("b")];
	for item in &kept {
		store.append(item.clone())?;
	}
	store.append(json!("bad"))?;
	let open = store.current_path.clone().unwrap();

	// The fetch rejects and deletes the open file, and returns the finished one.
	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 1024), kept);
	assert_eq!(rejected_paths(&verdicts), vec![open.clone()]);
	assert!(gone(&open));
	assert!(store.has_data());
	let result = store.fetch(None, None)?.unwrap();
	store.remove(&result.removable.unwrap())?;
	assert!(!store.has_data());

	// Only a rejected file: Ok(None), and nothing left.
	store.append(json!("bad again"))?;
	assert!(store.has_data());
	assert!(store.fetch(None, None)?.is_none());
	assert!(!store.has_data());
	assert!(files_on_disk(&temp_dir)?.is_empty());
	Ok(())
}

#[test]
fn test_rejection_of_recovered_leftovers() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let good_open = write_leftover(&temp_dir, 1, &unclosed(&[json!("ok 1")]))?;
	let bad_open = write_leftover(&temp_dir, 2, &unclosed(&[json!("bad 2")]))?;
	let bad_closed = write_leftover(&temp_dir, 3, &closed(&[json!("bad 3")]))?;
	let good_closed = write_leftover(&temp_dir, 4, &closed(&[json!("ok 4")]))?;

	let mut store = store(&temp_dir, 1024)?;
	let verdicts = reject_containing(&mut store, "bad");
	let files = finished_files(&mut store)?;
	assert_eq!(
		assert_ceiling(&files, 1024),
		vec![json!("ok 1"), json!("ok 4")]
	);
	assert_eq!(
		rejected_paths(&verdicts),
		vec![bad_open.clone(), bad_closed.clone()]
	);
	assert!(gone(&bad_open) && gone(&bad_closed));
	assert!(DirectoryStore::finished_path(&good_open).exists());
	assert!(DirectoryStore::finished_path(&good_closed).exists());
	assert_eq!(files_on_disk(&temp_dir)?.len(), 2);
	Ok(())
}

#[test]
fn test_validator_that_rejects_everything_never_accumulates_files() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = one_per_file(&temp_dir, 2)?;
	store.set_file_validator(|_| Err(io::Error::other("no")));
	for round in 0..10 {
		for i in 0..5 {
			store.append(json!({"round": round, "i": i}))?;
			// At most the open file is on disk.
			assert_eq!(files_on_disk(&temp_dir)?.len(), 1);
		}
		assert!(store.fetch(None, None)?.is_none());
		assert!(!store.has_data(), "round {round}");
		assert!(files_on_disk(&temp_dir)?.is_empty(), "round {round}");
	}
	Ok(())
}

#[test]
fn test_selective_rejection_deletes_exactly_the_rejected_files() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = one_per_file(&temp_dir, 3)?;
	// Reject by size: any file over 150 bytes. Plain files are 113 bytes, padded 191.
	let verdicts = Arc::new(Mutex::new(Vec::new()));
	let log = verdicts.clone();
	store.set_file_validator(move |path| {
		let ok = fs::metadata(path)?.len() <= 150;
		log.lock().unwrap().push((path.to_path_buf(), ok));
		if ok {
			Ok(())
		} else {
			Err(io::Error::other("too big"))
		}
	});

	// Files of three items; every third file gets a large item and is rejected.
	let mut expected = Vec::new();
	for file in 0..9 {
		for i in 0..3 {
			let item = if file % 3 == 1 && i == 1 {
				json!({"f": file, "pad": "p".repeat(80)})
			} else {
				json!({"f": file, "i": i})
			};
			if file % 3 != 1 {
				expected.push(item.clone());
			}
			store.append(item)?;
		}
	}
	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 1024), expected);
	assert_eq!(batch_counts(&files), vec![3; 6]);

	let verdicts_now = verdicts.lock().unwrap().clone();
	assert_eq!(verdicts_now.len(), 9);
	for (path, ok) in verdicts_now {
		assert_eq!(
			DirectoryStore::finished_path(&path).exists(),
			ok,
			"{path:?}"
		);
		assert!(!path.exists(), "{path:?}");
	}
	assert_eq!(files_on_disk(&temp_dir)?.len(), 6);
	Ok(())
}

#[test]
fn test_new_session_after_rejections_finds_nothing_to_recover() -> Result<()> {
	let temp_dir = TempDir::new()?;
	{
		let mut store = one_per_file(&temp_dir, 1)?;
		reject_containing(&mut store, "bad");
		for item in ["ok", "bad", "ok", "bad", "bad"] {
			store.append(json!(item))?;
		}
		assert_eq!(finished_files(&mut store)?.len(), 2);
	}
	let before = snapshot(temp_dir.path())?;
	let mut store = one_per_file(&temp_dir, 1)?;
	assert!(store.leftovers.is_empty());
	let verdicts = reject_containing(&mut store, "bad");
	assert_eq!(
		assert_ceiling(&finished_files(&mut store)?, 1024),
		vec![json!("ok"), json!("ok")]
	);
	assert!(verdicts.lock().unwrap().is_empty());
	assert_eq!(snapshot(temp_dir.path())?, before);
	Ok(())
}

#[test]
fn test_failure_to_delete_a_rejected_file_is_not_an_error() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = one_per_file(&temp_dir, 1)?;
	// The validator removes the file itself, so the store's own delete fails.
	store.set_file_validator(|path| {
		if fs::read_to_string(path)?.contains("bad") {
			fs::remove_file(path)?;
			return Err(io::Error::other("rejected and already removed"));
		}
		Ok(())
	});
	store.append(json!("bad"))?;
	store.append(json!("ok"))?;
	store.append(json!("bad"))?;
	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 1024), vec![json!("ok")]);
	assert_eq!(files_on_disk(&temp_dir)?.len(), 1);
	Ok(())
}

#[test]
fn test_rename_failure_is_an_error_and_deletes_nothing() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = one_per_file(&temp_dir, 1)?;
	let verdicts = reject_containing(&mut store, "never");
	store.append(json!("a"))?;
	let first = store.current_path.clone().unwrap();
	// A directory where the finished file would go makes the rename fail.
	fs::create_dir(DirectoryStore::finished_path(&first))?;

	// On rotation: the append fails, and the file is kept as it was.
	assert!(store.append(json!("b")).is_err());
	assert_eq!(verdicts.lock().unwrap().len(), 1);
	assert!(first.exists());
	let first_bytes = fs::read(&first)?;
	assert!(first_bytes.starts_with(&unclosed(&[json!("a")])));
	serde_json::from_slice::<Value>(&first_bytes)?;

	// At fetch: the fetch fails, and the open file is kept.
	store.append(json!("c"))?;
	let second = store.current_path.clone().unwrap();
	fs::create_dir(DirectoryStore::finished_path(&second))?;
	assert!(store.fetch(None, None).is_err());
	assert!(second.exists());
	assert!(fs::read(&second)?.starts_with(&unclosed(&[json!("c")])));
	Ok(())
}

#[test]
fn test_recovery_rename_failure_deletes_nothing() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let blocked = write_leftover(&temp_dir, 1, &closed(&[json!("kept")]))?;
	write_leftover(&temp_dir, 2, &closed(&[json!("ok")]))?;
	fs::create_dir(DirectoryStore::finished_path(&blocked))?;

	let mut store = store(&temp_dir, 1024)?;
	reject_containing(&mut store, "never");
	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 1024), vec![json!("ok")]);
	assert_eq!(fs::read(&blocked)?, closed(&[json!("kept")]));
	Ok(())
}
