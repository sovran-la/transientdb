//! `end_batch` finishes the open file, so items appended before and after it are never
//! in the same file. With no open file it changes nothing on disk.

use super::finish_failure::{arm, heal, tagged, FAULTS};
use super::support::*;
use super::*;

pub(super) fn tag(n: usize) -> Value {
	json!({"tag": n})
}

pub(super) fn tags_of(files: &[Finished]) -> Vec<Vec<usize>> {
	files
		.iter()
		.map(|f| {
			f.batch()
				.iter()
				.map(|v| v["tag"].as_u64().unwrap() as usize)
				.collect()
		})
		.collect()
}

pub(super) fn append_tags(store: &mut DirectoryStore, tags: std::ops::Range<usize>) -> Result<()> {
	for n in tags {
		store.append(tag(n))?;
	}
	Ok(())
}

#[test]
fn test_items_before_and_after_never_share_a_file() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 4096)?;
	append_tags(&mut store, 0..2)?;
	store.end_batch()?;
	append_tags(&mut store, 2..4)?;
	let files = finished_files(&mut store)?;
	assert_eq!(tags_of(&files), vec![vec![0, 1], vec![2, 3]]);
	Ok(())
}

#[test]
fn test_several_boundaries_make_one_file_per_span() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 4096)?;
	append_tags(&mut store, 0..1)?;
	store.end_batch()?;
	append_tags(&mut store, 1..3)?;
	store.end_batch()?;
	append_tags(&mut store, 3..6)?;
	store.end_batch()?;
	append_tags(&mut store, 6..7)?;
	let files = finished_files(&mut store)?;
	assert_eq!(
		tags_of(&files),
		vec![vec![0], vec![1, 2], vec![3, 4, 5], vec![6]]
	);
	Ok(())
}

#[test]
fn test_nothing_to_end_changes_nothing() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 4096)?;

	// A new store.
	let before = snapshot(temp_dir.path())?;
	store.end_batch()?;
	store.end_batch()?;
	assert_eq!(snapshot(temp_dir.path())?, before);
	assert!(!store.has_data());
	assert!(store.fetch(None, None)?.is_none());

	// After everything was fetched and removed.
	append_tags(&mut store, 0..2)?;
	let result = store.fetch(None, None)?.unwrap();
	store.remove(&result.removable.unwrap())?;
	let before = snapshot(temp_dir.path())?;
	store.end_batch()?;
	store.end_batch()?;
	assert_eq!(snapshot(temp_dir.path())?, before);
	assert!(!store.has_data());

	// After reset.
	append_tags(&mut store, 2..4)?;
	store.reset();
	store.end_batch()?;
	store.end_batch()?;
	assert!(files_on_disk(&temp_dir)?.is_empty());
	assert!(!store.has_data());

	// The next appends still form one file.
	append_tags(&mut store, 4..7)?;
	let files = finished_files(&mut store)?;
	assert_eq!(tags_of(&files), vec![vec![4, 5, 6]]);
	Ok(())
}

#[test]
fn test_repeated_end_batch_makes_no_empty_file() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 4096)?;
	append_tags(&mut store, 0..1)?;
	store.end_batch()?;
	let after_first = snapshot(temp_dir.path())?;
	for _ in 0..3 {
		store.end_batch()?;
	}
	assert_eq!(snapshot(temp_dir.path())?, after_first);
	append_tags(&mut store, 1..2)?;
	store.end_batch()?;
	store.end_batch()?;
	let files = finished_files(&mut store)?;
	assert_eq!(tags_of(&files), vec![vec![0], vec![1]]);
	assert_eq!(files_on_disk(&temp_dir)?.len(), 2);
	Ok(())
}

#[test]
fn test_end_batch_finishes_the_open_file_in_the_envelope() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let envelope = Envelope::new("events")
		.field("source", "sensor-7")
		.timestamp("sentAt");
	let max = 300;
	let mut store = DirectoryStore::new(config(&temp_dir, max, envelope.clone()))?;
	// Fill the file to the byte: the closing written by end_batch must still fit.
	let opening = envelope.opening().len();
	let closing = envelope.closing(fixed_time()).len();
	let a = item_of_len(60);
	let b = item_of_len(max - opening - closing - 60 - 1);
	store.append(a.clone())?;
	store.append(b.clone())?;
	let open = store.current_path.clone().unwrap();
	store.end_batch()?;

	assert!(store.writer.is_none() && store.current_path.is_none());
	assert!(!open.exists());
	let done = finished(&open);
	let file = read_finished(&done, &envelope)?;
	assert_eq!(file.len, max as u64);
	assert_eq!(file.batch(), &vec![a.clone(), b.clone()]);
	assert_eq!(store.sorted_files(false)?, vec![done.clone()]);

	// The next append goes to a new file, with a higher index.
	store.append(tag(9))?;
	let next = store.current_path.clone().unwrap();
	assert_ne!(next, open);
	assert_eq!(store.sorted_files(true)?, vec![done, next]);
	let files = finished_files(&mut store)?;
	assert_eq!(batch_counts(&files), vec![2, 1]);
	assert_eq!(assert_ceiling(&files, max), vec![a, b, tag(9)]);
	Ok(())
}

#[test]
fn test_end_batch_without_an_open_file_leaves_the_directory_unchanged() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 4096)?;
	append_tags(&mut store, 0..3)?;
	// Finished files stay, unfetched and unremoved, and a foreign file sits alongside.
	store.end_batch()?;
	fs::write(temp_dir.path().join("1-other"), b"not ours")?;
	let before = snapshot(temp_dir.path())?;
	store.end_batch()?;
	assert_eq!(snapshot(temp_dir.path())?, before);
	assert!(store.has_data());
	Ok(())
}

#[test]
fn test_end_batch_does_not_recover_leftovers() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let leftover_items = [tag(100), tag(101)];
	let leftover = write_leftover(&temp_dir, 1, &unclosed(&leftover_items))?;
	let mut store = store(&temp_dir, 4096)?;
	assert_eq!(store.leftovers, vec![leftover.clone()]);

	// With no open file, then with one.
	store.end_batch()?;
	append_tags(&mut store, 0..2)?;
	store.end_batch()?;
	assert_eq!(fs::read(&leftover)?, unclosed(&leftover_items));
	assert!(!finished(&leftover).exists());
	assert_eq!(store.leftovers, vec![leftover.clone()]);

	// The next fetch recovers it, ahead of the file end_batch finished.
	let files = finished_files(&mut store)?;
	assert_eq!(tags_of(&files), vec![vec![100, 101], vec![0, 1]]);
	assert!(store.leftovers.is_empty());
	Ok(())
}

#[test]
fn test_validator_rejection_at_end_batch_deletes_the_file() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 4096)?;
	store.set_file_validator(|path| {
		if fs::read_to_string(path)?.contains("reject") {
			return Err(io::Error::other("rejected"));
		}
		Ok(())
	});
	store.append(json!("reject me"))?;
	let open = store.current_path.clone().unwrap();
	store.end_batch()?;
	assert!(!open.exists() && !finished(&open).exists());
	assert!(files_on_disk(&temp_dir)?.is_empty());
	assert!(store.leftovers.is_empty());
	assert!(!store.has_data());

	store.append(json!("keep me"))?;
	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 4096), vec![json!("keep me")]);
	Ok(())
}

#[test]
fn test_failure_finishing_at_end_batch_is_recovered_by_the_next_fetch() -> Result<()> {
	let max = 200;
	for fault in FAULTS {
		let temp_dir = TempDir::new()?;
		let mut store = store(&temp_dir, max)?;
		let (a, b) = (tagged(60, "a"), tagged(FILLER, "b"));
		store.append(a.clone())?;
		store.append(b.clone())?;
		let failed = store.current_path.clone().unwrap();

		let blocker = arm(&store, fault);
		assert!(store.end_batch().is_err(), "{fault:?}");
		assert!(store.writer.is_none(), "{fault:?}");
		assert_eq!(store.leftovers, vec![failed.clone()], "{fault:?}");
		assert!(failed.exists(), "{fault:?}");
		heal(&store, blocker);

		let c = tagged(20, "c");
		store.append(c.clone())?;
		assert_ne!(store.current_path.as_ref(), Some(&failed), "{fault:?}");
		let files = finished_files(&mut store)?;
		assert_eq!(batch_counts(&files), vec![2, 1], "{fault:?}");
		assert_eq!(assert_ceiling(&files, max), vec![a, b, c], "{fault:?}");
		assert!(store.leftovers.is_empty(), "{fault:?}");
	}
	Ok(())
}

#[test]
fn test_boundary_does_not_leak_across_reset() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 4096)?;
	append_tags(&mut store, 0..1)?;
	store.end_batch()?;
	append_tags(&mut store, 1..2)?;
	store.reset();
	append_tags(&mut store, 2..4)?;
	store.end_batch()?;
	append_tags(&mut store, 4..5)?;
	let files = finished_files(&mut store)?;
	assert_eq!(tags_of(&files), vec![vec![2, 3], vec![4]]);
	Ok(())
}
