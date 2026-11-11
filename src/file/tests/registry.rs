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

#[cfg(unix)]
#[test]
fn test_a_symlink_to_a_missing_file_is_where_the_file_will_be() {
	use std::os::unix::fs::symlink;
	let dir = TempDir::new().unwrap();
	let real = dir.path().join("real.tdb");
	// Relative to the link's directory, as a link usually is.
	let link = dir.path().join("link.tdb");
	symlink("real.tdb", &link).unwrap();
	// A link to that link, and an absolute link into a directory that is missing.
	let chain = dir.path().join("chain.tdb");
	symlink(&link, &chain).unwrap();
	let deep = dir.path().join("a").join("b").join("deep.tdb");
	let to_deep = dir.path().join("to-deep.tdb");
	symlink(&deep, &to_deep).unwrap();

	let expected = canonical(&real).unwrap();
	assert_eq!(canonical(&link).unwrap(), expected);
	assert_eq!(canonical(&chain).unwrap(), expected);
	assert_eq!(canonical(&relative(&link)).unwrap(), expected);
	assert!(!real.exists(), "nothing is created but parent directories");
	assert_eq!(canonical(&to_deep).unwrap(), canonical(&deep).unwrap());
	assert!(dir.path().join("a/b").is_dir());
	assert!(!deep.exists());
	// Once the file exists, every spelling gives the same path.
	std::fs::write(&real, b"x").unwrap();
	assert_eq!(canonical(&chain).unwrap(), expected);
	assert_eq!(canonical(&real).unwrap(), expected);
}

#[cfg(unix)]
#[test]
fn test_a_symlink_loop_is_an_error() {
	use std::os::unix::fs::symlink;
	let dir = TempDir::new().unwrap();
	let a = dir.path().join("a.tdb");
	let b = dir.path().join("b.tdb");
	symlink(&b, &a).unwrap();
	symlink(&a, &b).unwrap();
	let e = canonical(&a).unwrap_err();
	assert_ne!(e.kind(), ErrorKind::NotFound, "{e}");
	assert!(!dir.path().join("a.tdb").exists());
}

/// `canonical(path)` fails with the error for a link to a missing directory, naming `link`.
#[cfg(unix)]
fn assert_refused_at(path: &Path, link: &Path) {
	let e = canonical(path).unwrap_err();
	assert_eq!(e.kind(), ErrorKind::InvalidInput, "{e}");
	let expected = format!(
		"{} is a symbolic link to a missing directory",
		link.display()
	);
	assert_eq!(e.to_string(), expected);
}

#[cfg(unix)]
#[test]
fn test_a_symlink_to_a_missing_directory_in_the_path_is_refused() {
	use std::os::unix::fs::symlink;
	let dir = TempDir::new().unwrap();
	let gone = dir.path().join("moved-away");
	// Relative and absolute links, and a link to a link that dangles.
	let data = dir.path().join("data");
	symlink("moved-away", &data).unwrap();
	let absolute = dir.path().join("absolute");
	symlink(&gone, &absolute).unwrap();
	let chain = dir.path().join("chain");
	symlink(&data, &chain).unwrap();
	for link in [&data, &absolute, &chain] {
		// The link is the parent, or further up the path.
		assert_refused_at(&link.join("events.tdb"), link);
		assert_refused_at(&link.join("a").join("b").join("events.tdb"), link);
		assert_refused_at(&relative(&link.join("events.tdb")), &relative(link));
	}
	assert!(!gone.exists(), "nothing is created where the link points");
	assert!(!dir.path().join("a").exists());

	// Once the directory is back, the path resolves through the link.
	std::fs::create_dir(&gone).unwrap();
	let real = canonical(&gone.join("events.tdb")).unwrap();
	assert_eq!(canonical(&data.join("events.tdb")).unwrap(), real);
	assert_eq!(canonical(&chain.join("events.tdb")).unwrap(), real);
	assert!(!real.exists());
}

#[cfg(unix)]
#[test]
fn test_a_dangling_directory_link_reached_through_another_link_is_refused() {
	use std::os::unix::fs::symlink;
	let dir = TempDir::new().unwrap();
	let real = dir.path().join("real");
	std::fs::create_dir(&real).unwrap();
	symlink(dir.path().join("missing"), real.join("inner")).unwrap();
	let outer = dir.path().join("outer");
	symlink(&real, &outer).unwrap();
	let inner = outer.join("inner");
	assert_refused_at(&inner.join("x").join("s.tdb"), &inner);
	assert!(!dir.path().join("missing").exists());
}

#[cfg(unix)]
#[test]
fn test_a_directory_link_loop_is_not_called_a_missing_directory() {
	use std::os::unix::fs::symlink;
	let dir = TempDir::new().unwrap();
	let a = dir.path().join("a");
	let b = dir.path().join("b");
	symlink(&b, &a).unwrap();
	symlink(&a, &b).unwrap();
	let e = canonical(&a.join("s.tdb")).unwrap_err();
	assert!(!e.to_string().contains("missing directory"), "{e}");
	assert!(!e.to_string().is_empty());
}
