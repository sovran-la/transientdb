//! Files an earlier session left unfinished are recovered by the first `fetch`, not by
//! `new`, so the validator set after `new` runs on them.

use super::support::*;
use super::*;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// A validator that records every path it sees and rejects those for which `reject`
/// returns true.
fn recording(
	store: &mut DirectoryStore,
	reject: impl Fn(&std::path::Path) -> bool + Send + Sync + 'static,
) -> Arc<Mutex<Vec<PathBuf>>> {
	let seen = Arc::new(Mutex::new(Vec::new()));
	let log = seen.clone();
	store.set_file_validator(move |path| {
		log.lock().unwrap().push(path.to_path_buf());
		if reject(path) {
			Err(io::Error::other("rejected"))
		} else {
			Ok(())
		}
	});
	seen
}

fn seen(log: &Arc<Mutex<Vec<PathBuf>>>) -> Vec<PathBuf> {
	log.lock().unwrap().clone()
}

/// Three leftovers at indexes 2, 5 and 9: unclosed, closed but not renamed, unclosed.
fn three_leftovers(dir: &TempDir) -> Result<(Vec<PathBuf>, Vec<Vec<Value>>)> {
	let batches = vec![
		vec![json!({"l": 2, "n": 0}), json!({"l": 2, "n": 1})],
		vec![json!({"l": 5, "n": 0})],
		vec![json!({"l": 9, "n": 0}), json!("nine"), json!([9])],
	];
	let paths = vec![
		write_leftover(dir, 2, &unclosed(&batches[0]))?,
		write_leftover(dir, 5, &closed(&batches[1]))?,
		write_leftover(dir, 9, &unclosed(&batches[2]))?,
	];
	Ok((paths, batches))
}

fn fetched(store: &mut DirectoryStore) -> Result<Vec<PathBuf>> {
	Ok(store
		.fetch(None, None)?
		.map(|r| r.data.unwrap())
		.unwrap_or_default())
}

#[test]
fn test_new_modifies_nothing_when_leftovers_exist() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let (paths, _) = three_leftovers(&temp_dir)?;
	let before = snapshot(temp_dir.path())?;

	let store = store(&temp_dir, 1024)?;
	assert_eq!(snapshot(temp_dir.path())?, before);
	assert_eq!(store.leftovers, paths);
	assert!(store.has_data(), "unrecovered leftovers are data");
	drop(store);
	assert_eq!(snapshot(temp_dir.path())?, before);
	Ok(())
}

#[test]
fn test_validator_set_after_new_sees_every_recovered_file() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let (paths, batches) = three_leftovers(&temp_dir)?;

	let mut store = store(&temp_dir, 1024)?;
	let log = recording(&mut store, |_| false);
	assert!(seen(&log).is_empty());

	let files = fetched(&mut store)?;
	assert_eq!(seen(&log), paths);
	assert_eq!(files, paths.iter().map(|p| finished(p)).collect::<Vec<_>>());
	for (file, batch) in files.iter().zip(&batches) {
		assert_eq!(read_finished(file, &with_source(KEY))?.batch(), batch);
	}
	Ok(())
}

#[test]
fn test_first_fetch_returns_leftovers_and_this_sessions_files_in_order() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let (paths, batches) = three_leftovers(&temp_dir)?;

	let mut config = config(&temp_dir, 1024, with_source(KEY));
	config.max_file_items = Some(2);
	let mut store = DirectoryStore::new(config)?;
	let log = recording(&mut store, |_| false);
	let new_items: Vec<Value> = (0..5).map(|i| json!({"new": i})).collect();
	for item in &new_items {
		store.append(item.clone())?;
	}
	// Two files finished by rotation, both this session's; nothing recovered yet.
	assert_eq!(seen(&log).len(), 2);
	assert!(seen(&log).iter().all(|p| !paths.contains(p)));
	assert!(store.current_path.as_ref().unwrap().ends_with("12-events"));

	let files = finished_files(&mut store)?;
	let expected: Vec<Value> = batches.concat().into_iter().chain(new_items).collect();
	assert_eq!(assert_ceiling(&files, 1024), expected);
	assert_eq!(batch_counts(&files), vec![2, 1, 3, 2, 2, 1]);
	// Every file validated exactly once: three leftovers, two rotations, the open file.
	let mut all = seen(&log);
	assert_eq!(all.len(), 6);
	all.sort();
	all.dedup();
	assert_eq!(all.len(), 6);
	Ok(())
}

#[test]
fn test_rejected_leftover_does_not_fail_the_fetch() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let (paths, batches) = three_leftovers(&temp_dir)?;
	let rejected = paths[1].clone();

	let mut store = store(&temp_dir, 1024)?;
	let target = rejected.clone();
	let log = recording(&mut store, move |p| p == target);
	store.append(json!("new"))?;

	let files = finished_files(&mut store)?;
	assert_eq!(
		assert_ceiling(&files, 1024),
		[batches[0].clone(), batches[2].clone(), vec![json!("new")]].concat()
	);
	assert_eq!(seen(&log).len(), 4);
	// Deleted, in every form.
	assert!(!rejected.exists());
	assert!(!finished(&rejected).exists());
	assert_eq!(files_on_disk(&temp_dir)?.len(), 3);

	// Nothing to retry in this session.
	let again = finished_files(&mut store)?;
	assert_eq!(again.len(), 3);
	assert_eq!(seen(&log).len(), 4);
	drop(store);

	// Nor in a new one.
	let mut store = DirectoryStore::new(config(&temp_dir, 1024, with_source(KEY)))?;
	assert!(store.leftovers.is_empty());
	let log = recording(&mut store, |_| false);
	assert_eq!(fetched(&mut store)?.len(), 3);
	assert!(seen(&log).is_empty());
	Ok(())
}

#[test]
fn test_leftovers_recover_as_before_without_a_validator() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let (paths, batches) = three_leftovers(&temp_dir)?;
	let originals: Vec<Vec<u8>> = paths.iter().map(fs::read).collect::<Result<_>>()?;

	let mut store = store(&temp_dir, 1024)?;
	let files = fetched(&mut store)?;
	assert_eq!(files.len(), 3);
	for ((file, original), batch) in files.iter().zip(&originals).zip(&batches) {
		let bytes = fs::read(file)?;
		if original.ends_with(b"}") && serde_json::from_slice::<Value>(original).is_ok() {
			// Already closed: renamed as is.
			assert_eq!(&bytes, original);
		} else {
			// Unclosed: the original bytes, then one closing.
			assert!(bytes.starts_with(original));
			assert_eq!(bytes.len(), original.len() + CLOSING);
		}
		assert_eq!(read_finished(file, &with_source(KEY))?.batch(), batch);
	}
	Ok(())
}

#[test]
fn test_reset_before_first_fetch_removes_leftovers() -> Result<()> {
	let temp_dir = TempDir::new()?;
	three_leftovers(&temp_dir)?;
	fs::write(temp_dir.path().join("3-other"), "{\"items\":[1")?;

	let mut store = store(&temp_dir, 1024)?;
	let log = recording(&mut store, |_| false);
	store.reset();
	assert!(!store.has_data());
	assert!(store.leftovers.is_empty());
	assert_eq!(
		files_on_disk(&temp_dir)?,
		vec![temp_dir.path().join("3-other")]
	);
	assert!(store.fetch(None, None)?.is_none());
	assert!(seen(&log).is_empty());

	store.append(json!("after reset"))?;
	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 1024), vec![json!("after reset")]);
	assert_eq!(seen(&log).len(), 1);
	Ok(())
}

#[test]
fn test_append_never_recovers() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let (paths, _) = three_leftovers(&temp_dir)?;
	let before: Vec<Vec<u8>> = paths.iter().map(fs::read).collect::<Result<_>>()?;

	let mut config = config(&temp_dir, 1024, with_source(KEY));
	config.max_file_items = Some(1);
	let mut store = DirectoryStore::new(config)?;
	let log = recording(&mut store, |_| false);
	for i in 0..5 {
		store.append(json!(i))?;
	}
	// Four rotations, none of them a leftover.
	assert_eq!(seen(&log).len(), 4);
	assert!(seen(&log).iter().all(|p| !paths.contains(p)));
	let after: Vec<Vec<u8>> = paths.iter().map(fs::read).collect::<Result<_>>()?;
	assert_eq!(after, before);
	assert_eq!(store.leftovers, paths);
	Ok(())
}

#[test]
fn test_recovery_runs_once() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let (paths, _) = three_leftovers(&temp_dir)?;

	let mut store = store(&temp_dir, 1024)?;
	let log = recording(&mut store, |_| false);
	assert_eq!(fetched(&mut store)?.len(), 3);
	assert_eq!(seen(&log), paths);
	assert!(store.leftovers.is_empty());

	// An unfinished file that appears after the first fetch is not this session's
	// leftover; later fetches leave it alone.
	let late = write_leftover(&temp_dir, 4, &unclosed(&[json!("late")]))?;
	for _ in 0..3 {
		assert_eq!(fetched(&mut store)?.len(), 3);
	}
	assert_eq!(seen(&log), paths);
	assert_eq!(fs::read(&late)?, unclosed(&[json!("late")]));
	Ok(())
}

#[test]
fn test_a_file_that_fails_to_finish_is_retried_once_by_the_next_fetch() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let (paths, _) = three_leftovers(&temp_dir)?;

	let mut config = config(&temp_dir, 1024, with_source(KEY));
	config.max_file_items = Some(1);
	let mut store = DirectoryStore::new(config)?;
	// A directory where this session's first file would be renamed to makes finishing
	// it fail, leaving it unfinished on disk.
	let first_new = temp_dir.path().join("10-events");
	fs::create_dir(finished(&first_new))?;
	let log = recording(&mut store, |_| false);
	store.append(json!("a"))?;
	assert!(store.append(json!("b")).is_err());
	assert!(first_new.exists());
	// Listed once, after the earlier session's leftovers.
	let mut listed = paths.clone();
	listed.push(first_new.clone());
	assert_eq!(store.leftovers, listed);
	store.append(json!("c"))?;

	let files = fetched(&mut store)?;
	// Leftovers, then this session's later file. The fetch retries the unfinished one,
	// which still cannot be renamed: it is logged, kept, and not listed again.
	assert_eq!(files.len(), 4);
	assert_eq!(files[3], finished(&temp_dir.path().join("11-events")));
	assert_eq!(
		&files[..3],
		&paths.iter().map(|p| finished(p)).collect::<Vec<_>>()[..]
	);
	assert!(first_new.exists());
	assert!(store.leftovers.is_empty());
	let validated = || seen(&log).iter().filter(|p| **p == first_new).count();
	assert_eq!(validated(), 2, "{:?}", seen(&log));
	for _ in 0..3 {
		assert_eq!(fetched(&mut store)?.len(), 4);
	}
	assert_eq!(validated(), 2, "{:?}", seen(&log));
	Ok(())
}
