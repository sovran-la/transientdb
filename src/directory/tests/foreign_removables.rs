//! `remove` deletes only this store's own finished files in its own directory; any
//! other path it is handed is left in place, and that is not an error.

use super::support::*;
use super::*;
use crate::Equivalent;
use std::path::{Component, Path, PathBuf};

fn removables(paths: &[PathBuf]) -> Vec<Box<dyn Equivalent>> {
	paths
		.iter()
		.map(|p| Box::new(p.clone()) as Box<dyn Equivalent>)
		.collect()
}

/// A store over `dir` holding `count` finished files of one item each, fetched; returns
/// the store and the fetched paths.
fn store_with_files(dir: &Path, count: usize) -> Result<(DirectoryStore, Vec<PathBuf>)> {
	let mut store = DirectoryStore::new(DirectoryConfig {
		write_key: KEY.to_string(),
		storage_location: dir.to_path_buf(),
		base_filename: "events".to_string(),
		max_file_size: 1024,
		max_file_items: Some(1),
	})?;
	for i in 0..count {
		store.append(json!({"i": i}))?;
	}
	let files = store.fetch(None, None)?.unwrap().data.unwrap();
	assert_eq!(files.len(), count);
	Ok((store, files))
}

fn all_exist(paths: &[PathBuf]) -> bool {
	paths.iter().all(|p| p.exists())
}

fn none_exist(paths: &[PathBuf]) -> bool {
	paths.iter().all(|p| !p.exists())
}

#[test]
fn test_another_stores_removables_remove_nothing() -> Result<()> {
	let (dir_a, dir_b) = (TempDir::new()?, TempDir::new()?);
	let (mut a, a_files) = store_with_files(dir_a.path(), 3)?;
	let (mut b, b_files) = store_with_files(dir_b.path(), 3)?;
	// Same config in different directories: the same file names.
	let names = |files: &[PathBuf]| -> Vec<PathBuf> {
		files
			.iter()
			.map(|f| f.file_name().unwrap().into())
			.collect()
	};
	assert_eq!(names(&a_files), names(&b_files));

	b.remove(&removables(&a_files))?;
	assert!(all_exist(&a_files) && all_exist(&b_files));
	a.remove(&removables(&b_files))?;
	assert!(all_exist(&a_files) && all_exist(&b_files));

	// Each store's own removables still remove exactly what it fetched.
	a.remove(&removables(&a_files[..2]))?;
	assert!(none_exist(&a_files[..2]) && all_exist(&a_files[2..]));
	assert!(all_exist(&b_files));
	Ok(())
}

#[test]
fn test_a_mix_removes_only_the_stores_own_files() -> Result<()> {
	let (dir_a, dir_b) = (TempDir::new()?, TempDir::new()?);
	let (_a, a_files) = store_with_files(dir_a.path(), 3)?;
	let (mut b, b_files) = store_with_files(dir_b.path(), 3)?;
	let mixed = [
		a_files[0].clone(),
		b_files[0].clone(),
		a_files[1].clone(),
		b_files[2].clone(),
	];
	b.remove(&removables(&mixed))?;
	assert!(all_exist(&a_files));
	assert!(!b_files[0].exists() && b_files[1].exists() && !b_files[2].exists());
	Ok(())
}

#[test]
fn test_paths_that_are_not_the_stores_finished_files_are_left_in_place() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let outside = TempDir::new()?;
	let (mut store, own) = store_with_files(temp_dir.path(), 1)?;

	// Another store's finished file in the same directory.
	let mut logs = DirectoryStore::new(DirectoryConfig {
		base_filename: "logs".to_string(),
		..config(&temp_dir, 1024, KEY)
	})?;
	logs.append(json!("log"))?;
	let other_finished = logs.fetch(None, None)?.unwrap().data.unwrap();
	// Foreign files, one with a finished name of a near miss.
	let notes = temp_dir.path().join("notes.txt");
	fs::write(&notes, "not data")?;
	let near_miss = temp_dir.path().join("007-events.temp");
	fs::write(&near_miss, "{}")?;
	// This store's unfinished files: a leftover and the open file.
	let leftover = write_leftover(&temp_dir, 50, &unclosed(&[json!(1)]))?;
	store.append(json!("open"))?;
	let open = store.current_path.clone().unwrap();
	// A finished name outside the directory, and one in a subdirectory.
	let elsewhere = outside.path().join("1-events.temp");
	fs::write(&elsewhere, "{}")?;
	let sub = temp_dir.path().join("sub");
	fs::create_dir(&sub)?;
	let nested = sub.join("2-events.temp");
	fs::write(&nested, "{}")?;
	// A directory with one of the store's finished names.
	let dir_named = temp_dir.path().join("60-events.temp");
	fs::create_dir(&dir_named)?;

	let mut foreign = vec![
		notes,
		near_miss,
		leftover,
		open,
		elsewhere,
		nested,
		dir_named,
		temp_dir.path().join("70-events.temp"),
		PathBuf::from("1-events.temp"),
		PathBuf::new(),
	];
	foreign.extend(other_finished);
	let before = snapshot(temp_dir.path())?;
	let outside_before = snapshot(outside.path())?;
	store.remove(&removables(&foreign))?;
	assert_eq!(snapshot(temp_dir.path())?, before);
	assert_eq!(snapshot(outside.path())?, outside_before);
	assert!(foreign[6].is_dir());

	// The store's own finished file, in the same call, is still removed.
	foreign.extend(own.clone());
	store.remove(&removables(&foreign))?;
	assert!(none_exist(&own));
	let mut expected = before;
	expected.remove(Path::new(own[0].file_name().unwrap()));
	assert_eq!(snapshot(temp_dir.path())?, expected);
	Ok(())
}

/// `path` relative to the current directory, if one can be written and resolves back to
/// it (on Windows, not across drives).
fn relative_to_cwd(path: &Path) -> Option<PathBuf> {
	let cwd = fs::canonicalize(std::env::current_dir().ok()?).ok()?;
	let target = fs::canonicalize(path).ok()?;
	let (cwd, target): (Vec<Component>, Vec<Component>) =
		(cwd.components().collect(), target.components().collect());
	let common = cwd.iter().zip(&target).take_while(|(a, b)| a == b).count();
	if common == 0 {
		return None;
	}
	let mut relative = PathBuf::new();
	for _ in common..cwd.len() {
		relative.push("..");
	}
	relative.extend(&target[common..]);
	(fs::canonicalize(&relative).ok()? == fs::canonicalize(path).ok()?).then_some(relative)
}

#[test]
fn test_other_spellings_of_the_storage_directory_still_match() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let base = temp_dir.path().to_path_buf();
	fs::create_dir(base.join("sub"))?;
	let canonical = fs::canonicalize(&base)?;
	let mut spellings = vec![base.clone(), canonical.clone(), base.join("sub").join("..")];
	let relative = relative_to_cwd(&base);
	if relative.is_none() {
		eprintln!(
			"No relative path to {base:?} from the current directory; skipping that spelling"
		);
	}
	spellings.extend(relative);

	// Every pair: the store opened with one spelling, the removable in another.
	for store_spelling in &spellings {
		for removable_spelling in &spellings {
			let (mut store, files) = store_with_files(store_spelling, 1)?;
			let name = files[0].file_name().unwrap();
			let respelled = removable_spelling.join(name);
			assert!(respelled.exists());
			store.remove(&removables(std::slice::from_ref(&respelled)))?;
			assert!(
				!canonical.join(name).exists(),
				"store {store_spelling:?}, removable {respelled:?}"
			);
		}
	}
	Ok(())
}
