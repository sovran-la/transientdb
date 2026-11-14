//! FileStore reports what it handles itself through the `log` facade, with the crate's
//! target: warnings for what it worked around, errors for what the OS refused.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use common::*;
use log::{Level, Log, Metadata, Record};
use std::path::Path;
use std::sync::{Mutex, OnceLock, PoisonError};
use transientdb::{DataStore, Error, FileConfig, FileStore, LOG_TARGET};

/// One logged message.
#[derive(Debug, Clone)]
struct Logged {
	level: Level,
	target: String,
	message: String,
}

/// Keeps every message logged in this test binary. Tests run in parallel, so each one
/// looks only at the messages naming its own file.
struct Capture(Mutex<Vec<Logged>>);

impl Log for Capture {
	fn enabled(&self, _: &Metadata) -> bool {
		true
	}

	fn log(&self, record: &Record) {
		self.0
			.lock()
			.unwrap_or_else(PoisonError::into_inner)
			.push(Logged {
				level: record.level(),
				target: record.target().to_owned(),
				message: record.args().to_string(),
			});
	}

	fn flush(&self) {}
}

fn capture() -> &'static Capture {
	static CAPTURE: OnceLock<Capture> = OnceLock::new();
	let capture = CAPTURE.get_or_init(|| Capture(Mutex::new(Vec::new())));
	// Only the first call in the binary installs it; later calls find it in place.
	if log::set_logger(capture).is_ok() {
		log::set_max_level(log::LevelFilter::Trace);
	}
	capture
}

/// The messages logged about the file at `path`. The store may name the file by its
/// canonical path (on macOS the temporary directory is reached through a symbolic link),
/// so a message is matched by the temporary directory's own name, which is unique.
fn logged_about(path: &Path) -> Vec<Logged> {
	let dir = path
		.parent()
		.and_then(Path::file_name)
		.unwrap()
		.to_string_lossy();
	let file = path.file_name().unwrap().to_string_lossy();
	let names = format!("{dir}/{file}:");
	capture()
		.0
		.lock()
		.unwrap_or_else(PoisonError::into_inner)
		.iter()
		.filter(|logged| {
			logged.message.starts_with("FileStore ") && logged.message.contains(&names)
		})
		.cloned()
		.collect()
}

fn config(path: &Path, id: &str, max_items: usize) -> FileConfig {
	FileConfig {
		path: path.to_path_buf(),
		id: id.to_string(),
		max_item_size: 1024,
		max_items,
	}
}

/// The one message about `path` that contains `text`, checked for the crate's target.
fn one(path: &Path, text: &str) -> Logged {
	let matching: Vec<Logged> = logged_about(path)
		.into_iter()
		.filter(|logged| logged.message.contains(text))
		.collect();
	assert_eq!(
		matching.len(),
		1,
		"messages about {path:?} containing {text:?}: {matching:?}"
	);
	let logged = matching[0].clone();
	assert_eq!(logged.target, LOG_TARGET);
	logged
}

#[test]
fn the_target_is_the_crate_name() {
	assert_eq!(LOG_TARGET, "transientdb");
}

#[test]
fn a_file_that_does_not_match_is_discarded_with_a_warning() {
	capture();
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("events.tdb");
	{
		let mut store = FileStore::new(config(&path, "first", 10));
		store.append(tagged(0..1).remove(0)).unwrap();
	}
	let store = FileStore::new(config(&path, "second", 10));
	assert!(!store.has_data());
	assert_eq!(one(&path, "discarding the file").level, Level::Warn);
}

#[test]
fn dropping_the_oldest_item_from_a_full_store_is_a_warning() {
	capture();
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("events.tdb");
	let mut store = FileStore::new(config(&path, "id", 2));
	for item in tagged(0..3) {
		store.append(item).unwrap();
	}
	assert_eq!(one(&path, "the oldest").level, Level::Warn);
}

#[test]
fn a_second_store_on_one_file_is_a_warning() {
	capture();
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("events.tdb");
	let _first = FileStore::new(config(&path, "id", 10));
	let mut second = FileStore::new(config(&path, "id", 10));
	assert!(matches!(
		second.append(tagged(0..1).remove(0)),
		Err(Error::InUse)
	));
	assert_eq!(
		one(&path, "another store in this process").level,
		Level::Warn
	);
}

#[test]
fn a_file_that_cannot_be_opened_is_an_error() {
	capture();
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("events.tdb");
	std::fs::create_dir(&path).unwrap();
	let _store = FileStore::new(config(&path, "id", 10));
	assert_eq!(one(&path, "could not open the file").level, Level::Error);
}

#[test]
fn nothing_is_logged_about_a_store_that_handles_nothing() {
	capture();
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("events.tdb");
	let mut store = FileStore::new(config(&path, "id", 10));
	for item in tagged(0..3) {
		store.append(item).unwrap();
	}
	let items = store.fetch(10, 4096).unwrap().unwrap();
	store.remove(items).unwrap();
	assert!(logged_about(&path).is_empty(), "{:?}", logged_about(&path));
}
