//! Recovery follows the configured envelope: it closes unfinished leftovers with it,
//! renames complete ones (a JSON object whose items key holds a non-empty array)
//! untouched, and deletes leftovers that do not start with its opening.

use super::support::*;
use super::*;
use crate::envelope::samples::is_timestamp;

/// A custom envelope with fields and a timestamp.
fn custom() -> Envelope {
	Envelope::new("events")
		.field("source", "sensor-7")
		.field("n", json!([1, {"k": null}]))
		.timestamp("sentAt")
}

fn custom_store(dir: &TempDir, max_file_size: usize) -> Result<DirectoryStore> {
	DirectoryStore::new(config(dir, max_file_size, custom()))
}

/// The bytes of every fetched file.
fn fetch_bytes(store: &mut DirectoryStore) -> Result<Vec<Vec<u8>>> {
	let files = store
		.fetch(None, None)?
		.map(|r| r.data.unwrap())
		.unwrap_or_default();
	files.iter().map(fs::read).collect()
}

#[test]
fn test_unfinished_leftover_is_closed_with_the_configured_envelope() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let items = vec![json!({"a": 1}), json!("two")];
	let unfinished = unclosed_with(&custom(), &items);
	let path = write_leftover(&temp_dir, 1, &unfinished)?;

	let mut store = custom_store(&temp_dir, 4096)?;
	let files = fetch_bytes(&mut store)?;
	assert_eq!(files.len(), 1);
	let text = String::from_utf8(files[0].clone()).unwrap();
	let expected = format!(
		"{}{}",
		String::from_utf8(unfinished).unwrap(),
		r#"],"source":"sensor-7","n":[1,{"k":null}],"sentAt":""#
	);
	assert!(text.starts_with(&expected), "{text}");
	assert_eq!(text.len(), expected.len() + 24 + 2, "{text}");
	assert!(text.ends_with("\"}"));
	let json: Value = serde_json::from_str(&text)?;
	assert_eq!(json["events"], json!(items));
	assert!(is_timestamp(&json["sentAt"]));
	assert!(finished(&path).exists());
	Ok(())
}

#[test]
fn test_complete_leftover_under_a_custom_envelope_is_renamed_untouched() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let items = vec![json!({"i": 0, "brace": "}"}), json!("],\"sentAt\":")];
	let bytes = closed_with(&custom(), &items);
	let path = write_leftover(&temp_dir, 1, &bytes)?;

	for session in 0..3 {
		let mut store = custom_store(&temp_dir, 4096)?;
		let files = fetch_bytes(&mut store)?;
		// Renamed with its bytes untouched: never closed a second time.
		assert_eq!(files, vec![bytes.clone()], "session {session}");
		assert_eq!(files_on_disk(&temp_dir)?, vec![finished(&path)]);
	}
	Ok(())
}

#[test]
fn test_complete_leftover_with_other_trailing_fields_is_delivered_as_written() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let items = vec![json!(1), json!({"two": 2})];
	// Closed under an envelope with the same items key but other fields and no
	// timestamp, as an earlier session configured differently would have left it.
	let earlier = Envelope::new("events")
		.field("source", "sensor-1")
		.field("extra", true);
	let bytes = closed_with(&earlier, &items);
	let path = write_leftover(&temp_dir, 1, &bytes)?;

	let mut store = custom_store(&temp_dir, 4096)?;
	let files = fetch_bytes(&mut store)?;
	assert_eq!(files, vec![bytes.clone()]);
	assert_eq!(fs::read(finished(&path))?, bytes);
	assert!(!path.exists());
	Ok(())
}

#[test]
fn test_leftover_with_another_items_key_is_deleted() -> Result<()> {
	let items = vec![json!(1), json!(2)];
	let other = Envelope::new("items").field("source", "sensor-7");
	for bytes in [unclosed_with(&other, &items), closed_with(&other, &items)] {
		let temp_dir = TempDir::new()?;
		let path = write_leftover(&temp_dir, 1, &bytes)?;
		let mut store = custom_store(&temp_dir, 4096)?;
		assert!(store.fetch(None, None)?.is_none());
		assert!(!path.exists() && !finished(&path).exists());
		assert!(files_on_disk(&temp_dir)?.is_empty());
	}
	Ok(())
}

#[test]
fn test_complete_leftover_with_an_empty_items_array_is_deleted() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let path = write_leftover(&temp_dir, 1, &closed_with(&custom(), &[]))?;
	let mut store = custom_store(&temp_dir, 4096)?;
	assert!(store.fetch(None, None)?.is_none());
	assert!(!path.exists() && !finished(&path).exists());
	Ok(())
}

#[test]
fn test_oversized_complete_leftover_is_cut_down_under_a_custom_envelope() -> Result<()> {
	let envelope_len = custom().opening().len() + custom().closing(fixed_time()).len();
	let items: Vec<Value> = (0..6).map(|_| item_of_len(40)).collect();
	let bytes = closed_with(&custom(), &items);
	assert_eq!(bytes.len(), envelope_len + 6 * 40 + 5);
	// Room for exactly three once closed; a byte less leaves two; too small for one.
	let three = envelope_len + 3 * 40 + 2;
	for (max, kept) in [(three, 3), (three - 1, 2), (envelope_len + 39, 0)] {
		let temp_dir = TempDir::new()?;
		let path = write_leftover(&temp_dir, 1, &bytes)?;
		let mut store = custom_store(&temp_dir, max.max(100))?;
		let files = fetch_bytes(&mut store)?;
		if kept == 0 {
			assert!(files.is_empty(), "max={max}");
			assert!(files_on_disk(&temp_dir)?.is_empty(), "max={max}");
			continue;
		}
		assert_eq!(files.len(), 1, "max={max}");
		let file = &files[0];
		assert!(file.len() <= max, "max={max}");
		let kept_bytes = unclosed_with(&custom(), &items[..kept]);
		assert!(file.starts_with(&kept_bytes), "max={max}");
		assert_eq!(
			file.len(),
			kept_bytes.len() + custom().closing(fixed_time()).len()
		);
		let json: Value = serde_json::from_slice(file)?;
		assert_eq!(json["events"], json!(items[..kept]));
		assert_eq!(json["source"], "sensor-7");
		assert!(finished(&path).exists());
	}
	Ok(())
}
