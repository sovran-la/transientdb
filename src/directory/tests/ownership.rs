//! A store reads, closes, renames, counts and deletes only the files it names:
//! `{index}-{base_filename}`, and that name with `.temp` appended once finished.

use super::support::*;
use super::*;
use std::path::{Path, PathBuf};

fn named(dir: &TempDir, base_filename: &str) -> DirectoryConfig {
	let mut config = config(dir, 1024, KEY);
	config.base_filename = base_filename.to_string();
	config
}

/// Files that are not the "events" store's, including near misses of its names.
const FOREIGN: [(&str, &str); 12] = [
	("7-other", "{ \"batch\": [1"),
	(
		"7-other.temp",
		"{\"batch\":[1],\"sentAt\":\"s\",\"writeKey\":\"k\"}",
	),
	("notes.txt", "not data"),
	("12-eventsX", "{ \"batch\": [2"),
	("12-events.json", "{ \"batch\": [3"),
	("12-events.temp.bak", "{ \"batch\": [4"),
	("12-events.tmp", "{ \"batch\": [5"),
	("012-events", "{ \"batch\": [6"),
	("+12-events", "{ \"batch\": [7"),
	("-12-events", "{ \"batch\": [8"),
	("99999999999-events", "{ \"batch\": [9"),
	("events", "{ \"batch\": [10"),
];

fn write_foreign(dir: &TempDir) -> Result<()> {
	for (name, content) in FOREIGN {
		fs::write(dir.path().join(name), content)?;
	}
	let sub = dir.path().join("subdir");
	fs::create_dir(&sub)?;
	fs::write(sub.join("3-events"), "{ \"batch\": [11")?;
	// A directory with one of the store's own names is still not a file it owns.
	fs::create_dir(dir.path().join("4-events.temp"))?;
	Ok(())
}

fn fetched_paths(store: &mut DirectoryStore) -> Result<Vec<PathBuf>> {
	Ok(store
		.fetch(None, None)?
		.map(|r| r.data.unwrap())
		.unwrap_or_default())
}

#[test]
fn test_ownership_rule() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let store = DirectoryStore::new(named(&temp_dir, "events"))?;
	let cases = [
		("0-events", Some((0, false))),
		("7-events", Some((7, false))),
		("7-events.temp", Some((7, true))),
		("4294967295-events", Some((u32::MAX, false))),
		("4294967296-events", None),
		("007-events", None),
		("+7-events", None),
		("-7-events", None),
		("7-events.temp.temp", None),
		("7-eventsX", None),
		("7-events.json", None),
		("7-events.tmp", None),
		("7-events.", None),
		("7-Events", None),
		("7-other", None),
		("7-other.temp", None),
		("7events", None),
		("events", None),
		("-events", None),
		("notes.txt", None),
		("", None),
	];
	for (name, expected) in cases {
		assert_eq!(store.owned_name(name), expected, "{name:?}");
	}
	Ok(())
}

#[test]
fn test_ownership_rule_with_a_dotted_base_filename() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let store = DirectoryStore::new(named(&temp_dir, "events.json"))?;
	assert_eq!(store.owned_name("3-events.json"), Some((3, false)));
	assert_eq!(store.owned_name("3-events.json.temp"), Some((3, true)));
	// What `with_extension` would have made of it, and what an "events" store owns.
	assert_eq!(store.owned_name("3-events.temp"), None);
	assert_eq!(store.owned_name("3-events"), None);
	assert_eq!(store.owned_name("3-events.log"), None);

	// A base that only contains "temp" somewhere other than a final ".temp" is fine.
	let store = DirectoryStore::new(named(&temp_dir, "events.temp.json"))?;
	assert_eq!(store.owned_name("3-events.temp.json"), Some((3, false)));
	assert_eq!(store.owned_name("3-events.temp.json.temp"), Some((3, true)));
	assert_eq!(store.owned_name("3-events.temp"), None);

	// A base containing hyphens: only the first hyphen ends the index.
	let store = DirectoryStore::new(named(&temp_dir, "my-events-v2"))?;
	assert_eq!(store.owned_name("3-my-events-v2"), Some((3, false)));
	assert_eq!(store.owned_name("3-my-events-v2.temp"), Some((3, true)));
	assert_eq!(store.owned_name("my-events-v2"), None);
	Ok(())
}

#[test]
fn test_ownership_matches_the_names_the_store_writes() -> Result<()> {
	for base in [
		"events",
		"events.json",
		"events.temp.json",
		"a.b.c",
		"my-events",
	] {
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(named(&temp_dir, base))?;
		store.append(json!(1))?;
		let open = store.current_path.clone().unwrap();
		let open_name = open.file_name().unwrap().to_str().unwrap();
		assert_eq!(open_name, format!("1-{base}"));
		assert_eq!(store.owned_name(open_name), Some((1, false)), "{base}");

		let files = fetched_paths(&mut store)?;
		assert_eq!(files.len(), 1, "{base}");
		let finished_name = files[0].file_name().unwrap().to_str().unwrap();
		assert_eq!(finished_name, format!("1-{base}.temp"));
		assert_eq!(store.owned_name(finished_name), Some((1, true)), "{base}");
		assert_eq!(read_finished(&files[0], KEY)?.batch(), &vec![json!(1)]);
	}
	Ok(())
}

#[test]
#[should_panic(expected = "base_filename ends in \".temp\"?")]
fn test_rejects_base_filename_ending_in_temp() {
	let temp_dir = TempDir::new().unwrap();
	// "1-events.temp" would be both this store's open file and an "events" store's
	// finished one.
	let _ = DirectoryStore::new(named(&temp_dir, "events.temp"));
}

#[test]
fn test_rejected_base_filename_creates_no_directory() {
	let temp_dir = TempDir::new().unwrap();
	let location = temp_dir.path().join("never-created");
	let mut config = named(&temp_dir, "x.temp");
	config.storage_location = location.clone();
	assert!(std::panic::catch_unwind(|| DirectoryStore::new(config)).is_err());
	assert!(!location.exists());
}

#[test]
fn test_foreign_files_are_never_touched() -> Result<()> {
	let temp_dir = TempDir::new()?;
	write_foreign(&temp_dir)?;
	let before = snapshot(temp_dir.path())?;

	let mut store = DirectoryStore::new(named(&temp_dir, "events"))?;
	assert_eq!(snapshot(temp_dir.path())?, before);
	assert!(!store.has_data(), "foreign files alone are not data");
	assert!(store.fetch(None, None)?.is_none());
	assert_eq!(snapshot(temp_dir.path())?, before);

	store.append(json!({"mine": 1}))?;
	store.append(json!({"mine": 2}))?;
	assert!(store.has_data());
	let files = fetched_paths(&mut store)?;
	assert_eq!(files.len(), 1);
	assert_eq!(
		read_finished(&files[0], KEY)?.batch(),
		&vec![json!({"mine": 1}), json!({"mine": 2})]
	);
	let mut after = snapshot(temp_dir.path())?;
	after.remove(Path::new(files[0].file_name().unwrap()));
	assert_eq!(after, before);

	store.append(json!({"mine": 3}))?;
	store.reset();
	assert!(!store.has_data());
	assert_eq!(snapshot(temp_dir.path())?, before);
	assert!(store.fetch(None, None)?.is_none());
	assert_eq!(snapshot(temp_dir.path())?, before);
	Ok(())
}

#[test]
fn test_foreign_files_survive_a_new_session() -> Result<()> {
	let temp_dir = TempDir::new()?;
	write_foreign(&temp_dir)?;
	{
		let mut store = DirectoryStore::new(named(&temp_dir, "events"))?;
		store.append(json!("left over"))?;
	}
	let mine = temp_dir.path().join("1-events");
	assert!(mine.exists());
	let mut before = snapshot(temp_dir.path())?;
	before.remove(Path::new("1-events"));

	let mut store = DirectoryStore::new(named(&temp_dir, "events"))?;
	let files = fetched_paths(&mut store)?;
	assert_eq!(files, vec![temp_dir.path().join("1-events.temp")]);
	assert_eq!(
		read_finished(&files[0], KEY)?.batch(),
		&vec![json!("left over")]
	);
	let mut after = snapshot(temp_dir.path())?;
	after.remove(Path::new("1-events.temp"));
	assert_eq!(after, before);
	Ok(())
}

#[test]
fn test_two_stores_in_one_directory_are_isolated() -> Result<()> {
	for (a_name, b_name) in [
		("events", "logs"),
		("events", "events.json"),
		("events.json", "events.log"),
		("events.json", "events.txt"),
		("events", "events.temp.json"),
		("events", "eventsX"),
	] {
		let temp_dir = TempDir::new()?;
		let mut a = DirectoryStore::new(named(&temp_dir, a_name))?;
		let mut b = DirectoryStore::new(named(&temp_dir, b_name))?;
		assert!(!a.has_data() && !b.has_data());

		let a_items: Vec<Value> = (0..3).map(|i| json!({"a": i})).collect();
		let b_items: Vec<Value> = (0..4).map(|i| json!({"b": i})).collect();
		for (x, y) in a_items.iter().zip(&b_items) {
			a.append(x.clone())?;
			b.append(y.clone())?;
		}
		b.append(b_items[3].clone())?;

		let case = format!("{a_name} / {b_name}");
		let a_files = finished_files(&mut a)?;
		assert_eq!(assert_ceiling(&a_files, 1024), a_items, "{case}");
		// a's fetch finished only its own file; b's is still open and not b's data yet.
		assert!(b.writer.is_some(), "{case}");
		let b_files = finished_files(&mut b)?;
		assert_eq!(assert_ceiling(&b_files, 1024), b_items, "{case}");
		assert_eq!(assert_ceiling(&finished_files(&mut a)?, 1024), a_items);

		a.reset();
		assert!(!a.has_data(), "{case}");
		assert!(b.has_data(), "{case}");
		assert!(a.fetch(None, None)?.is_none(), "{case}");
		assert_eq!(assert_ceiling(&finished_files(&mut b)?, 1024), b_items);
		assert_eq!(files_on_disk(&temp_dir)?.len(), 1, "{case}");
	}
	Ok(())
}

#[test]
fn test_two_stores_recover_only_their_own_leftovers() -> Result<()> {
	let temp_dir = TempDir::new()?;
	for (name, item) in [("events.json", "a"), ("events.log", "b"), ("events", "c")] {
		let mut store = DirectoryStore::new(named(&temp_dir, name))?;
		store.append(json!(item))?;
		// Dropped unfinished: each leaves "1-{name}" behind.
	}
	assert_eq!(files_on_disk(&temp_dir)?.len(), 3);

	for (name, item) in [("events.json", "a"), ("events.log", "b"), ("events", "c")] {
		let mut store = DirectoryStore::new(named(&temp_dir, name))?;
		let files = fetched_paths(&mut store)?;
		assert_eq!(files, vec![temp_dir.path().join(format!("1-{name}.temp"))]);
		assert_eq!(read_finished(&files[0], KEY)?.batch(), &vec![json!(item)]);
	}
	assert_eq!(files_on_disk(&temp_dir)?.len(), 3);
	Ok(())
}

#[test]
fn test_new_store_picks_an_index_above_its_own_files_only() -> Result<()> {
	let temp_dir = TempDir::new()?;
	fs::write(
		temp_dir.path().join("5-events.temp"),
		"{\"batch\":[1],\"sentAt\":\"s\",\"writeKey\":\"k\"}",
	)?;
	fs::write(temp_dir.path().join("9-events"), "{ \"batch\": [2")?;
	fs::write(temp_dir.path().join("500-other"), "{ \"batch\": [3")?;
	fs::write(temp_dir.path().join("700-eventsX"), "{ \"batch\": [4")?;

	let mut store = DirectoryStore::new(named(&temp_dir, "events"))?;
	store.append(json!("new"))?;
	assert_eq!(
		store.current_path.as_deref(),
		Some(temp_dir.path().join("10-events").as_path())
	);
	Ok(())
}

#[test]
fn test_index_scan_counts_finished_and_unfinished_files() -> Result<()> {
	for (name, expected) in [("41-events.temp", "42-events"), ("41-events", "42-events")] {
		let temp_dir = TempDir::new()?;
		fs::write(temp_dir.path().join(name), "{ \"batch\": [1")?;
		let mut store = DirectoryStore::new(named(&temp_dir, "events"))?;
		store.append(json!("new"))?;
		assert_eq!(
			store.current_path.as_deref(),
			Some(temp_dir.path().join(expected).as_path()),
			"{name}"
		);
	}
	Ok(())
}
