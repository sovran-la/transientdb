//! A leftover of this store that does not start with its opening can never be read, so
//! recovery deletes it. Files of other names are never touched, whatever they hold.

use super::support::*;
use super::*;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Content no store writes: a wrong opening, part of the opening, nothing, garbage.
fn unreadable_contents() -> Vec<(&'static str, Vec<u8>)> {
	vec![
		("wrong opening", b"{\"batch\":[1,2,3".to_vec()),
		("partial opening", b"{ \"bat".to_vec()),
		("zero length", Vec::new()),
		("garbage", b"\xff\xfe garbage \0\0 not json".to_vec()),
	]
}

fn open(dir: &TempDir) -> Result<DirectoryStore> {
	store(dir, 1024)
}

fn validated_paths(store: &mut DirectoryStore) -> Arc<Mutex<Vec<PathBuf>>> {
	let seen = Arc::new(Mutex::new(Vec::new()));
	let log = seen.clone();
	store.set_file_validator(move |path| {
		log.lock().unwrap().push(path.to_path_buf());
		Ok(())
	});
	seen
}

fn gone(path: &Path) -> bool {
	!path.exists() && !finished(path).exists()
}

#[test]
fn test_unreadable_leftovers_are_deleted_and_the_rest_recovered() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let good = [
		write_leftover(&temp_dir, 1, &unclosed(&[json!("one")]))?,
		write_leftover(&temp_dir, 5, &closed(&[json!("five"), json!(5)]))?,
		write_leftover(
			&temp_dir,
			9,
			&[&unclosed(&[json!("nine")])[..], b",{\"cut"].concat(),
		)?,
	];
	let mut unreadable = Vec::new();
	for (i, (_, bytes)) in unreadable_contents().into_iter().enumerate() {
		unreadable.push(write_leftover(&temp_dir, 2 + 2 * i as u32, &bytes)?);
	}

	let mut store = open(&temp_dir)?;
	let seen = validated_paths(&mut store);
	assert!(store.has_data());
	assert_eq!(store.leftovers.len(), good.len() + unreadable.len());

	let files = finished_files(&mut store)?;
	assert_eq!(
		assert_ceiling(&files, 1024),
		vec![json!("one"), json!("five"), json!(5), json!("nine")]
	);
	assert_eq!(batch_counts(&files), vec![1, 2, 1]);
	// Only readable files reach the validator.
	assert_eq!(seen.lock().unwrap().clone(), good.to_vec());
	for path in &unreadable {
		assert!(gone(path), "{path:?}");
	}
	let expected: Vec<PathBuf> = good.iter().map(|p| finished(p)).collect();
	assert_eq!(files_on_disk(&temp_dir)?, expected);
	assert!(store.leftovers.is_empty());
	Ok(())
}

#[test]
fn test_an_only_unreadable_leftover_leaves_no_data() -> Result<()> {
	for (case, bytes) in unreadable_contents() {
		let temp_dir = TempDir::new()?;
		let path = write_leftover(&temp_dir, 3, &bytes)?;
		let mut store = open(&temp_dir)?;
		assert!(store.has_data(), "{case}");
		assert!(store.fetch(None, None)?.is_none(), "{case}");
		assert!(gone(&path), "{case}");
		assert!(!store.has_data(), "{case}");
		assert!(store.fetch(None, None)?.is_none(), "{case}");
		drop(store);

		// A new session finds nothing at all.
		let mut store = open(&temp_dir)?;
		assert!(store.leftovers.is_empty(), "{case}");
		assert!(!store.has_data(), "{case}");
		assert!(store.fetch(None, None)?.is_none(), "{case}");
		assert!(files_on_disk(&temp_dir)?.is_empty(), "{case}");
	}
	Ok(())
}

#[test]
fn test_foreign_files_with_unreadable_content_are_untouched() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let foreign = [
		"7-other",
		"events",
		"012-events",
		"3-events.json",
		"3-eventsX",
		"4294967296-events",
		"notes.txt",
	];
	for (name, (_, bytes)) in foreign
		.iter()
		.zip(unreadable_contents().into_iter().cycle())
	{
		fs::write(temp_dir.path().join(name), bytes)?;
	}
	let before = snapshot(temp_dir.path())?;
	let mine = write_leftover(&temp_dir, 3, b"garbage")?;

	let mut store = open(&temp_dir)?;
	assert!(store.fetch(None, None)?.is_none());
	assert!(gone(&mine));
	assert_eq!(snapshot(temp_dir.path())?, before);
	assert!(!store.has_data());
	drop(store);

	let mut store = open(&temp_dir)?;
	assert!(store.fetch(None, None)?.is_none());
	assert_eq!(snapshot(temp_dir.path())?, before);
	Ok(())
}

#[test]
fn test_failed_delete_is_logged_and_the_rest_recover() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let stuck = write_leftover(&temp_dir, 1, b"garbage")?;
	let good = write_leftover(&temp_dir, 2, &unclosed(&[json!("good")]))?;

	let mut store = open(&temp_dir)?;
	store.faults.lock().unwrap().fail_remove = true;
	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 1024), vec![json!("good")]);
	// Left as it was: not renamed, not fetched, and not retried in this session.
	assert_eq!(fs::read(&stuck)?, b"garbage");
	assert!(!finished(&stuck).exists());
	assert!(store.leftovers.is_empty());
	store.faults.lock().unwrap().fail_remove = false;
	assert_eq!(finished_files(&mut store)?.len(), 1);
	assert_eq!(fs::read(&stuck)?, b"garbage");
	drop(store);

	// The next session deletes it.
	let mut store = open(&temp_dir)?;
	assert_eq!(store.leftovers, vec![stuck.clone()]);
	assert_eq!(finished_files(&mut store)?.len(), 1);
	assert!(gone(&stuck));
	assert_eq!(files_on_disk(&temp_dir)?, vec![finished(&good)]);
	Ok(())
}

#[test]
fn test_abandoned_file_found_unreadable_at_salvage_is_deleted() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = open(&temp_dir)?;
	store.append(json!("a"))?;
	let abandoned = store.current_path.clone().unwrap();
	{
		let mut faults = store.faults.lock().unwrap();
		faults.write_budget = Some(2);
		faults.fail_truncate = true;
	}
	assert!(store.append(json!("lost")).is_err());
	assert_eq!(store.leftovers, vec![abandoned.clone()]);
	*store.faults.lock().unwrap() = Default::default();
	// Whatever went wrong with the disk also took the opening.
	fs::write(&abandoned, b"\0\0\0\0")?;

	store.append(json!("b"))?;
	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 1024), vec![json!("b")]);
	assert!(gone(&abandoned));
	assert!(store.leftovers.is_empty());
	assert_eq!(files_on_disk(&temp_dir)?.len(), 1);
	Ok(())
}
