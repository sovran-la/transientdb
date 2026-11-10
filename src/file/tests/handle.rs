//! The file handle: positional reads and writes, and the faults tests inject into it.

use super::support::{bytes_of, temp_path};
use crate::file::faults::{Faults, Op};
use crate::file::handle::SlotFile;
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::path::Path;

fn handle(path: &Path) -> (SlotFile, crate::file::faults::Hook) {
	let file = OpenOptions::new()
		.read(true)
		.write(true)
		.create(true)
		.truncate(false)
		.open(path)
		.unwrap();
	let hook = Faults::shared();
	(SlotFile::new(file, hook.clone()), hook)
}

#[test]
fn test_writes_land_where_asked_and_reach_the_file_at_once() {
	let (_dir, path) = temp_path();
	let (mut file, hook) = handle(&path);
	file.write_at(4, b"abc", Op::Payload).unwrap();
	// Nothing is buffered: another reader sees the bytes before anything is flushed.
	assert_eq!(bytes_of(&path), b"\0\0\0\0abc");
	file.write_at(1, b"Z", Op::State).unwrap();
	assert_eq!(bytes_of(&path), b"\0Z\0\0abc");
	assert_eq!(file.len().unwrap(), 7);
	let mut buf = [0u8; 3];
	file.read_at(4, &mut buf, Op::ReadItem).unwrap();
	assert_eq!(&buf, b"abc");
	assert_eq!(hook.lock().unwrap().done, vec![Op::Payload, Op::State]);
}

#[test]
fn test_reading_past_the_end_is_unexpected_eof() {
	let (_dir, path) = temp_path();
	let (mut file, _hook) = handle(&path);
	file.write_at(0, b"abc", Op::Payload).unwrap();
	let mut buf = [0u8; 4];
	let e = file.read_at(0, &mut buf, Op::ReadItem).unwrap_err();
	assert_eq!(e.kind(), ErrorKind::UnexpectedEof);
}

#[test]
fn test_set_len_cuts_and_extends() {
	let (_dir, path) = temp_path();
	let (mut file, hook) = handle(&path);
	file.write_at(0, b"abcdef", Op::Payload).unwrap();
	file.set_len(2).unwrap();
	assert_eq!(bytes_of(&path), b"ab");
	assert_eq!(hook.lock().unwrap().done, vec![Op::Payload, Op::Cut]);
}

#[test]
fn test_every_op_can_be_made_to_fail_without_touching_the_file() {
	let (_dir, path) = temp_path();
	let (mut file, hook) = handle(&path);
	file.write_at(0, b"abcdef", Op::Payload).unwrap();
	hook.lock().unwrap().done.clear();
	for op in Op::ALL {
		hook.lock().unwrap().fail(op);
		let mut buf = [0u8; 2];
		let result = match op {
			Op::Cut => file.set_len(0),
			Op::Length => file.len().map(|_| ()),
			Op::ReadHeader | Op::ReadEntry | Op::ReadItem => file.read_at(0, &mut buf, op),
			_ => file.write_at(0, b"XY", op),
		};
		let message = result.unwrap_err().to_string();
		assert!(message.contains(&format!("{op:?}")), "{message}");
		hook.lock().unwrap().fail.clear();
	}
	assert_eq!(bytes_of(&path), b"abcdef");
	assert!(hook.lock().unwrap().done.is_empty());
}

#[test]
fn test_fail_after_lets_an_op_succeed_that_many_times_first() {
	let (_dir, path) = temp_path();
	let (mut file, hook) = handle(&path);
	hook.lock().unwrap().fail_after(Op::State, 2);
	file.write_at(0, b"a", Op::State).unwrap();
	file.write_at(1, b"b", Op::State).unwrap();
	assert!(file.write_at(2, b"c", Op::State).is_err());
	assert!(file.write_at(2, b"c", Op::State).is_err());
	// Other ops are not affected.
	file.write_at(2, b"c", Op::Entry).unwrap();
	assert_eq!(bytes_of(&path), b"abc");
}

#[test]
fn test_write_budget_cuts_a_write_at_every_byte_and_kills() {
	for budget in 0..=6 {
		let (_dir, path) = temp_path();
		let (mut file, hook) = handle(&path);
		file.write_at(0, b"......", Op::Payload).unwrap();
		hook.lock().unwrap().write_budget = Some(budget);
		let result = file.write_at(0, b"abcdef", Op::Entry);
		let mut expected = b"abcdef"[..budget].to_vec();
		expected.extend_from_slice(&b"......"[budget..]);
		assert_eq!(bytes_of(&path), expected, "budget {budget}");
		if budget == 6 {
			result.unwrap();
			assert!(!hook.lock().unwrap().killed);
			// The budget is spent: the next write is cut at 0 bytes.
			assert!(file.write_at(0, b"x", Op::State).is_err());
		} else {
			assert!(result.is_err());
		}
		let faults = hook.lock().unwrap();
		assert!(faults.killed, "budget {budget}");
		drop(faults);
		assert_eq!(&bytes_of(&path)[..budget], &b"abcdef"[..budget]);
		// Once killed, everything fails and nothing more reaches the file.
		let before = bytes_of(&path);
		let mut buf = [0u8; 1];
		assert!(file.read_at(0, &mut buf, Op::ReadItem).is_err());
		assert!(file.len().is_err());
		assert!(file.set_len(0).is_err());
		assert!(file.write_at(0, b"zzzzzz", Op::Payload).is_err());
		assert_eq!(bytes_of(&path), before);
	}
}

#[test]
fn test_a_budget_spans_several_writes() {
	let (_dir, path) = temp_path();
	let (mut file, hook) = handle(&path);
	hook.lock().unwrap().write_budget = Some(5);
	file.write_at(0, b"abc", Op::Payload).unwrap();
	assert!(file.write_at(3, b"def", Op::Entry).is_err());
	assert_eq!(bytes_of(&path), b"abcde");
	assert_eq!(hook.lock().unwrap().done, vec![Op::Payload]);
}
