//! `new` refuses a `base_filename` that would put the store's files outside its
//! directory or break the ownership rule, and accepts every ordinary file name.

use super::support::*;
use super::*;
use std::panic::{catch_unwind, AssertUnwindSafe};

fn named(dir: &TempDir, base_filename: &str) -> DirectoryConfig {
	let mut config = config(dir, 1024, with_source(KEY));
	config.base_filename = base_filename.to_string();
	config
}

/// The message `new` panics with for `base_filename`, or `None` if it does not panic.
/// The storage location is a directory that does not exist yet, to check that a
/// rejected name creates nothing.
fn panic_message(base_filename: &str) -> Option<String> {
	let temp_dir = TempDir::new().unwrap();
	let mut config = named(&temp_dir, base_filename);
	config.storage_location = temp_dir.path().join("never-created");
	let location = config.storage_location.clone();
	let payload = match catch_unwind(AssertUnwindSafe(|| DirectoryStore::new(config))) {
		Ok(store) => {
			store.unwrap();
			return None;
		}
		Err(payload) => payload,
	};
	assert!(
		!location.exists(),
		"{base_filename:?} created the directory"
	);
	assert!(
		fs::read_dir(temp_dir.path()).unwrap().next().is_none(),
		"{base_filename:?} created something"
	);
	Some(match payload.downcast::<String>() {
		Ok(message) => *message,
		Err(payload) => payload.downcast::<&str>().map(|m| m.to_string()).unwrap(),
	})
}

const EMPTY: &str = "base_filename is empty?";
const SEPARATOR: &str = "base_filename contains a path separator?";
const DOTS: &str = "base_filename is \".\" or \"..\"?";
const NUL: &str = "base_filename contains a NUL byte?";
const TEMP: &str = "base_filename ends in \".temp\"?";

#[test]
fn test_every_rejected_form_panics_with_its_message() {
	let cases = [
		("", EMPTY),
		("/", SEPARATOR),
		("a/b", SEPARATOR),
		("/events", SEPARATOR),
		("events/", SEPARATOR),
		("../events", SEPARATOR),
		("./events", SEPARATOR),
		("\\", SEPARATOR),
		("a\\b", SEPARATOR),
		("\\events", SEPARATOR),
		("..\\events", SEPARATOR),
		("a/b\\c", SEPARATOR),
		(".", DOTS),
		("..", DOTS),
		("\0", NUL),
		("ev\0ents", NUL),
		("events\0", NUL),
		("events.temp", TEMP),
		("x.temp", TEMP),
		(".temp", TEMP),
	];
	for (base, expected) in cases {
		let message = panic_message(base).unwrap_or_else(|| panic!("{base:?} was accepted"));
		assert!(message.starts_with(expected), "{base:?}: {message}");
	}
}

#[test]
#[should_panic(expected = "base_filename is empty?")]
fn test_rejects_empty_base_filename() {
	let temp_dir = TempDir::new().unwrap();
	let _ = DirectoryStore::new(named(&temp_dir, ""));
}

#[test]
#[should_panic(expected = "base_filename contains a path separator?")]
fn test_rejects_slash_in_base_filename() {
	let temp_dir = TempDir::new().unwrap();
	let _ = DirectoryStore::new(named(&temp_dir, "../events"));
}

#[test]
#[should_panic(expected = "base_filename contains a path separator?")]
fn test_rejects_backslash_in_base_filename() {
	let temp_dir = TempDir::new().unwrap();
	let _ = DirectoryStore::new(named(&temp_dir, "..\\events"));
}

#[test]
#[should_panic(expected = "base_filename is \".\" or \"..\"?")]
fn test_rejects_dot_base_filename() {
	let temp_dir = TempDir::new().unwrap();
	let _ = DirectoryStore::new(named(&temp_dir, "."));
}

#[test]
#[should_panic(expected = "base_filename is \".\" or \"..\"?")]
fn test_rejects_dot_dot_base_filename() {
	let temp_dir = TempDir::new().unwrap();
	let _ = DirectoryStore::new(named(&temp_dir, ".."));
}

#[test]
#[should_panic(expected = "base_filename contains a NUL byte?")]
fn test_rejects_nul_in_base_filename() {
	let temp_dir = TempDir::new().unwrap();
	let _ = DirectoryStore::new(named(&temp_dir, "ev\0ents"));
}

const ACCEPTED: [&str; 16] = [
	"events",
	"events.json",
	"a.b.c",
	"a..b",
	".hidden",
	"my events",
	" spaced ",
	"my-events-v2",
	"-",
	"2024",
	"0",
	"événements",
	"日本語",
	"🦀 log",
	"events.temp.json",
	"temp",
];

#[test]
fn test_ordinary_names_are_accepted() {
	for base in ACCEPTED {
		assert_eq!(panic_message(base), None, "{base:?}");
	}
}

#[test]
fn test_ordinary_names_round_trip_across_sessions() -> Result<()> {
	for base in ACCEPTED {
		let parent = TempDir::new()?;
		let location = parent.path().join("store");
		let mut config = named(&parent, base);
		config.storage_location = location.clone();

		let mut store = DirectoryStore::new(config.clone())?;
		store.append(json!({"base": base, "n": 1}))?;
		store.append(json!({"base": base, "n": 2}))?;
		let files = store.fetch(None, None)?.unwrap().data.unwrap();
		assert_eq!(
			files,
			vec![location.join(format!("1-{base}.temp"))],
			"{base:?}"
		);
		assert_eq!(
			read_finished(&files[0], &with_source(KEY))?.batch(),
			&vec![json!({"base": base, "n": 1}), json!({"base": base, "n": 2})],
			"{base:?}"
		);
		// Left unfinished for the next session.
		store.append(json!({"base": base, "n": 3}))?;
		assert_eq!(
			store.current_path,
			Some(location.join(format!("2-{base}"))),
			"{base:?}"
		);
		drop(store);

		let mut store = DirectoryStore::new(config)?;
		assert_eq!(store.leftovers, vec![location.join(format!("2-{base}"))]);
		let files = store.fetch(None, None)?.unwrap().data.unwrap();
		assert_eq!(files.len(), 2, "{base:?}");
		assert_eq!(
			files[1],
			location.join(format!("2-{base}.temp")),
			"{base:?}"
		);
		assert_eq!(
			read_finished(&files[1], &with_source(KEY))?.batch(),
			&vec![json!({"base": base, "n": 3})],
			"{base:?}"
		);
		// Everything stayed inside the storage directory.
		let entries: Vec<_> = fs::read_dir(parent.path())?.collect::<Result<_>>()?;
		assert_eq!(entries.len(), 1, "{base:?}");
		assert_eq!(fs::read_dir(&location)?.count(), 2, "{base:?}");
	}
	Ok(())
}
