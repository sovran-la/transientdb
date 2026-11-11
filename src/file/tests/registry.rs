//! The process-wide registry of store files in use.

use super::support::relative;
use crate::file::registry::{canonical, Lease};
use std::io::ErrorKind;
use std::path::Path;
use tempfile::TempDir;

#[test]
fn test_a_path_is_held_by_one_lease_at_a_time() {
	let dir = TempDir::new().unwrap();
	let path = canonical(&dir.path().join("a.tdb")).unwrap();
	let first = Lease::acquire(path.clone()).expect("free path");
	assert_eq!(first.path(), path);
	assert!(Lease::acquire(path.clone()).is_none());
	assert!(Lease::acquire(path.clone()).is_none());
	// Another path is not affected.
	let other = Lease::acquire(canonical(&dir.path().join("b.tdb")).unwrap());
	assert!(other.is_some());
	drop(first);
	let again = Lease::acquire(path.clone()).expect("released on drop");
	drop(again);
	assert!(Lease::acquire(path).is_some());
}

#[test]
fn test_relative_and_absolute_spellings_collide() {
	let dir = TempDir::new().unwrap();
	let absolute = dir.path().join("store.tdb");
	let relative = relative(&absolute);
	assert!(relative.is_relative());
	// Neither exists yet.
	assert!(!absolute.exists());
	let a = canonical(&absolute).unwrap();
	let b = canonical(&relative).unwrap();
	assert_eq!(a, b);
	let _lease = Lease::acquire(a).unwrap();
	assert!(Lease::acquire(b).is_none());
	// And once the file exists.
	std::fs::write(&absolute, b"x").unwrap();
	assert_eq!(canonical(&absolute).unwrap(), canonical(&relative).unwrap());
	assert!(Lease::acquire(canonical(&relative).unwrap()).is_none());
}

#[cfg(unix)]
#[test]
fn test_dot_dot_and_symlinked_spellings_collide() {
	let dir = TempDir::new().unwrap();
	let real = dir.path().join("real");
	std::fs::create_dir(&real).unwrap();
	let link = dir.path().join("link");
	std::os::unix::fs::symlink(&real, &link).unwrap();
	let plain = canonical(&real.join("s.tdb")).unwrap();
	assert_eq!(canonical(&link.join("s.tdb")).unwrap(), plain);
	assert_eq!(
		canonical(&real.join("..").join("real").join("s.tdb")).unwrap(),
		plain
	);
	assert_eq!(canonical(&real.join(".").join("s.tdb")).unwrap(), plain);
}

#[cfg(unix)]
#[test]
fn test_a_symlink_to_an_existing_file_is_the_file() {
	let dir = TempDir::new().unwrap();
	let real = dir.path().join("real.tdb");
	std::fs::write(&real, b"x").unwrap();
	let link = dir.path().join("link.tdb");
	std::os::unix::fs::symlink(&real, &link).unwrap();
	assert_eq!(canonical(&link).unwrap(), canonical(&real).unwrap());
}

#[test]
fn test_missing_parents_are_created() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("a").join("b").join("c").join("store.tdb");
	let canonical = canonical(&path).unwrap();
	assert!(dir.path().join("a/b/c").is_dir());
	assert!(!path.exists());
	assert!(canonical.ends_with("a/b/c/store.tdb"));
	assert!(canonical.is_absolute());
}

#[test]
fn test_a_bare_file_name_is_in_the_current_directory() {
	let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
	let name = "transientdb-registry-test-that-is-never-created.tdb";
	assert_eq!(canonical(Path::new(name)).unwrap(), cwd.join(name));
	assert!(!cwd.join(name).exists());
}

#[test]
fn test_a_path_that_names_no_file_is_refused() {
	for path in ["/", "..", "a/.."] {
		let e = canonical(Path::new(path)).unwrap_err();
		assert_eq!(e.kind(), ErrorKind::InvalidInput, "{path}");
	}
}

#[test]
fn test_a_parent_that_cannot_be_created_is_an_error() {
	let dir = TempDir::new().unwrap();
	let blocker = dir.path().join("file");
	std::fs::write(&blocker, b"x").unwrap();
	assert!(canonical(&blocker.join("store.tdb")).is_err());
}

#[test]
fn test_a_panic_while_holding_a_lease_releases_it() {
	let dir = TempDir::new().unwrap();
	let path = canonical(&dir.path().join("p.tdb")).unwrap();
	let held = path.clone();
	let result = std::panic::catch_unwind(move || {
		let _lease = Lease::acquire(held).unwrap();
		panic!("on the send path");
	});
	assert!(result.is_err());
	assert!(Lease::acquire(path).is_some());
}
