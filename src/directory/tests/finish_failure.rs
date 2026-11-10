//! A file that fails to finish (writing or flushing its closing, or renaming it) is
//! listed for recovery, so the next `fetch` of the same session delivers its items, once
//! and in order. The call that failed still returns the error.

use super::support::*;
use super::*;
use crate::directory::data_file::Faults;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy)]
pub(super) enum Fault {
	/// Writing the closing fails after this many of its bytes.
	Closing(usize),
	/// Flushing the closing fails; the closing itself reaches the file.
	Flush,
	/// Renaming the closed file fails: a directory sits at its finished path.
	Rename,
}

/// Arms `fault` for the next attempt to finish the open file. Returns the directory
/// put in the way of a rename, if any, for [`heal`] to remove.
pub(super) fn arm(store: &DirectoryStore, fault: Fault) -> Option<PathBuf> {
	match fault {
		Fault::Closing(bytes) => store.faults.lock().unwrap().write_budget = Some(bytes),
		Fault::Flush => store.faults.lock().unwrap().fail_flush = true,
		Fault::Rename => {
			let blocker = finished(store.current_path.as_ref()?);
			fs::create_dir(&blocker).unwrap();
			return Some(blocker);
		}
	}
	None
}

pub(super) fn heal(store: &DirectoryStore, blocker: Option<PathBuf>) {
	*store.faults.lock().unwrap() = Faults::default();
	if let Some(blocker) = blocker {
		fs::remove_dir(blocker).unwrap();
	}
}

pub(super) const FAULTS: [Fault; 6] = [
	Fault::Closing(0),
	Fault::Closing(1),
	Fault::Closing(30),
	Fault::Closing(CLOSING - 1),
	Fault::Flush,
	Fault::Rename,
];

/// A unique JSON string item, `tag` then padding, exactly `len` bytes serialized.
pub(super) fn tagged(len: usize, tag: &str) -> Value {
	let item = Value::String(format!("{tag}{}", "x".repeat(len - 2 - tag.len())));
	assert_eq!(item_len(&item), len);
	item
}

fn recording(store: &mut DirectoryStore) -> Arc<Mutex<Vec<PathBuf>>> {
	let seen = Arc::new(Mutex::new(Vec::new()));
	let log = seen.clone();
	store.set_file_validator(move |path| {
		serde_json::from_slice::<Value>(&fs::read(path)?)?;
		log.lock().unwrap().push(path.to_path_buf());
		Ok(())
	});
	seen
}

/// Fills a 200 byte file exactly with `a` and `b`, then fails to finish it with
/// `fault`, in a rotating append or in a fetch. After healing, appends `e` and `f`
/// and checks the next fetch delivers every item once, in order, within the ceiling.
/// With `fetch_first`, a fetch has already run (and recovered nothing) before.
fn fails_to_finish(fault: Fault, at_fetch: bool, fetch_first: bool) -> Result<()> {
	let case = format!("{fault:?}, at_fetch {at_fetch}, fetch_first {fetch_first}");
	let max = 200;
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, max)?;
	let seen = recording(&mut store);
	let mut expected = Vec::new();
	if fetch_first {
		store.append(tagged(20, "first"))?;
		expected.extend(assert_ceiling(&finished_files(&mut store)?, max));
	}

	let (a, b) = (tagged(60, "a"), tagged(FILLER, "b"));
	store.append(a.clone())?;
	store.append(b.clone())?;
	assert_eq!(store.current_size + CLOSING, max, "{case}");
	let failed = store.current_path.clone().unwrap();

	let blocker = arm(&store, fault);
	let err = if at_fetch {
		store.fetch(None, None).map(|_| ()).unwrap_err()
	} else {
		// Does not fit, so the append rotates first, and fails with the rotation.
		store.append(tagged(10, "d")).unwrap_err()
	};
	assert!(
		err.to_string().starts_with("injected") || matches!(fault, Fault::Rename),
		"{case}: {err}"
	);
	assert!(store.writer.is_none(), "{case}");
	assert_eq!(store.leftovers, vec![failed.clone()], "{case}");
	assert!(failed.exists(), "{case}");
	assert!(store.has_data(), "{case}");

	heal(&store, blocker);
	let (e, f) = (tagged(20, "e"), tagged(20, "f"));
	store.append(e.clone())?;
	store.append(f.clone())?;
	assert_ne!(store.current_path.as_ref(), Some(&failed), "{case}");

	let files = finished_files(&mut store)?;
	expected.extend([a, b, e, f]);
	assert_eq!(assert_ceiling(&files, max), expected, "{case}");
	assert_eq!(files[files.len() - 2].len, max as u64, "{case}");
	assert!(store.leftovers.is_empty(), "{case}");
	assert!(!failed.exists() && finished(&failed).exists(), "{case}");
	// The recovered file was validated, once or (after a failed rename) twice.
	let checks = seen
		.lock()
		.unwrap()
		.iter()
		.filter(|p| **p == failed)
		.count();
	let expected_checks = if matches!(fault, Fault::Rename) { 2 } else { 1 };
	assert_eq!(checks, expected_checks, "{case}");
	drop(store);

	// Nothing is left for a new session to recover, and nothing changed.
	let before = snapshot(temp_dir.path())?;
	let mut store = super::support::store(&temp_dir, max)?;
	assert!(store.leftovers.is_empty(), "{case}");
	assert_eq!(assert_ceiling(&finished_files(&mut store)?, max), expected);
	assert_eq!(snapshot(temp_dir.path())?, before, "{case}");
	Ok(())
}

#[test]
fn test_failure_finishing_on_rotation_is_recovered_by_the_next_fetch() -> Result<()> {
	for fault in FAULTS {
		fails_to_finish(fault, false, false)?;
	}
	Ok(())
}

#[test]
fn test_failure_finishing_at_fetch_is_recovered_by_the_next_fetch() -> Result<()> {
	for fault in FAULTS {
		fails_to_finish(fault, true, false)?;
	}
	Ok(())
}

#[test]
fn test_failure_after_the_first_fetch_is_recovered_by_a_later_one() -> Result<()> {
	for fault in FAULTS {
		fails_to_finish(fault, false, true)?;
		fails_to_finish(fault, true, true)?;
	}
	Ok(())
}

#[test]
fn test_rollback_failure_after_the_first_fetch_is_salvaged_by_a_later_one() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 1024)?;
	store.append(json!("first"))?;
	assert_eq!(finished_files(&mut store)?.len(), 1);

	store.append(json!("a"))?;
	let abandoned = store.current_path.clone().unwrap();
	{
		let mut faults = store.faults.lock().unwrap();
		faults.write_budget = Some(3);
		faults.fail_truncate = true;
	}
	assert!(store.append(json!("lost")).is_err());
	assert_eq!(store.leftovers, vec![abandoned.clone()]);
	heal(&store, None);
	store.append(json!("b"))?;
	let files = finished_files(&mut store)?;
	assert_eq!(
		assert_ceiling(&files, 1024),
		vec![json!("first"), json!("a"), json!("b")]
	);
	assert!(store.leftovers.is_empty());
	assert!(!abandoned.exists());
	Ok(())
}

#[test]
fn test_recovery_failure_is_not_retried_in_the_same_session() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 1024)?;
	let seen = recording(&mut store);
	store.append(json!("stuck"))?;
	let stuck = store.current_path.clone().unwrap();
	let blocker = arm(&store, Fault::Rename);
	assert!(store.fetch(None, None).is_err());
	assert_eq!(store.leftovers, vec![stuck.clone()]);

	// Still blocked: the next fetch tries once, logs it, and lets it go.
	store.append(json!("ok"))?;
	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 1024), vec![json!("ok")]);
	assert!(store.leftovers.is_empty());
	for _ in 0..3 {
		assert_eq!(finished_files(&mut store)?.len(), 1);
	}
	let checks = || seen.lock().unwrap().iter().filter(|p| **p == stuck).count();
	assert_eq!(checks(), 2);
	assert_eq!(fs::read(&stuck)?, closed_bytes_of(&stuck)?);
	drop(store);

	// A later session, with the obstacle gone, delivers it.
	heal_dir(blocker);
	let mut store = super::support::store(&temp_dir, 1024)?;
	assert_eq!(store.leftovers, vec![stuck.clone()]);
	let files = finished_files(&mut store)?;
	assert_eq!(
		assert_ceiling(&files, 1024),
		vec![json!("stuck"), json!("ok")]
	);
	Ok(())
}

/// The file's own bytes, asserting it is one closed envelope holding "stuck".
fn closed_bytes_of(path: &std::path::Path) -> Result<Vec<u8>> {
	let bytes = fs::read(path)?;
	let json: Value = serde_json::from_slice(&bytes)?;
	assert_eq!(json["items"], json!(["stuck"]));
	Ok(bytes)
}

fn heal_dir(blocker: Option<PathBuf>) {
	fs::remove_dir(blocker.unwrap()).unwrap();
}

#[test]
fn test_a_path_is_listed_once() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 1024)?;
	let (one, two) = (
		temp_dir.path().join("1-events"),
		temp_dir.path().join("2-events"),
	);
	store.list_leftover(one.clone());
	store.list_leftover(two.clone());
	store.list_leftover(one.clone());
	store.list_leftover(two.clone());
	assert_eq!(store.leftovers, vec![one, two]);
	Ok(())
}

/// A tiny xorshift, so the schedule of failures is varied but the same every run.
struct Schedule(u32);

impl Schedule {
	fn next(&mut self, below: u32) -> u32 {
		self.0 ^= self.0 << 13;
		self.0 ^= self.0 >> 17;
		self.0 ^= self.0 << 5;
		self.0 % below
	}
}

#[test]
fn test_mixed_failures_lose_and_duplicate_nothing() -> Result<()> {
	let max = 300;
	let temp_dir = TempDir::new()?;
	let mut config = config(&temp_dir, max, with_source(KEY));
	config.max_file_items = Some(4);
	let mut store = DirectoryStore::new(config.clone())?;
	recording(&mut store);
	let mut schedule = Schedule(0x2545_f491);
	let (mut accepted, mut delivered) = (Vec::new(), Vec::new());
	let (mut failed_appends, mut failed_fetches) = (0, 0);

	let deliver = |store: &mut DirectoryStore, delivered: &mut Vec<Value>| -> Result<()> {
		if let Some(result) = store.fetch(None, None)? {
			for path in result.data.as_ref().unwrap() {
				let file = read_finished(path, &with_source(KEY))?;
				assert!(file.len <= max as u64, "{path:?} is {} bytes", file.len);
				delivered.extend(file.batch().iter().cloned());
			}
			store.remove(&result.removable.unwrap())?;
		}
		Ok(())
	};

	for i in 0..400 {
		let fault = match schedule.next(10) {
			0 => Some(Fault::Closing(schedule.next(CLOSING as u32) as usize)),
			1 => Some(Fault::Flush),
			2 => Some(Fault::Rename),
			_ => None,
		};
		let blocker = fault.and_then(|f| arm(&store, f));
		if schedule.next(7) == 0 {
			if deliver(&mut store, &mut delivered).is_err() {
				failed_fetches += 1;
			}
		} else {
			let item = json!({"i": i, "pad": "p".repeat(schedule.next(60) as usize)});
			match store.append(item.clone()) {
				Ok(()) => accepted.push(item),
				Err(_) => failed_appends += 1,
			}
		}
		heal(&store, blocker);
	}
	deliver(&mut store, &mut delivered)?;
	assert!(failed_appends >= 10, "only {failed_appends} failed appends");
	assert!(failed_fetches >= 3, "only {failed_fetches} failed fetches");
	assert_eq!(delivered.len(), accepted.len());
	assert_eq!(delivered, accepted);
	assert!(!store.has_data());
	assert!(files_on_disk(&temp_dir)?.is_empty());
	drop(store);

	let mut store = DirectoryStore::new(config)?;
	assert!(store.leftovers.is_empty());
	assert!(store.fetch(None, None)?.is_none());
	Ok(())
}
