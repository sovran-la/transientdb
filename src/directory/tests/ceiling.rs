//! Every finished file is at most `max_file_size` bytes on disk, envelope included.

use super::support::*;
use super::*;

#[test]
fn test_envelope_lengths_match_constants() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let store = store(&temp_dir, 100)?;
	assert_eq!(store.opening().len(), OPENING);
	assert_eq!(store.closing_len, CLOSING);
	assert_eq!(store.max_item_size(), 100 - ENVELOPE);
	Ok(())
}

#[test]
fn test_ceiling_holds_for_mixed_sizes() -> Result<()> {
	for max in [100usize, 150, 256, 1000, 4096] {
		let temp_dir = TempDir::new()?;
		let mut store = store(&temp_dir, max)?;
		// `{"i":NNN,"pad":"..."}` is at most 18 bytes plus the padding.
		let pad_limit = max - ENVELOPE - 18;
		let appended: Vec<Value> = (0..200)
			.map(|i| json!({"i": i, "pad": "x".repeat((i * 37) % (pad_limit + 1))}))
			.collect();
		for item in &appended {
			store.append(item.clone())?;
		}

		let files = finished_files(&mut store)?;
		assert!(files.len() > 1, "max={max} should rotate");
		assert_eq!(assert_ceiling(&files, max), appended, "max={max}");
	}
	Ok(())
}

#[test]
fn test_exact_fit_fills_file_to_the_limit() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 200)?;
	// 12 + 60 + 1 + 67 + 60 = 200
	store.append(item_of_len(60))?;
	store.append(item_of_len(67))?;

	let files = finished_files(&mut store)?;
	assert_eq!(batch_counts(&files), vec![2]);
	assert_eq!(files[0].len, 200);
	Ok(())
}

#[test]
fn test_one_byte_over_rotates() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 200)?;
	let second = item_of_len(68);
	store.append(item_of_len(60))?;
	store.append(second.clone())?;

	let files = finished_files(&mut store)?;
	assert_eq!(batch_counts(&files), vec![1, 1]);
	assert_eq!(files[0].len, (ENVELOPE + 60) as u64);
	assert!(files[0].len < 200);
	assert_eq!(files[1].batch()[0], second);
	assert_ceiling(&files, 200);
	Ok(())
}

#[test]
fn test_separators_are_counted() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 200)?;
	// 42 + 43 + 43 = 128 bytes of items is exactly the room left by the envelope,
	// so the three fit only if the two commas were free.
	for len in [42, 43, 43] {
		store.append(item_of_len(len))?;
	}

	let files = finished_files(&mut store)?;
	assert_eq!(batch_counts(&files), vec![2, 1]);
	assert_eq!(files[0].len, (ENVELOPE + 42 + 1 + 43) as u64);
	assert_ceiling(&files, 200);
	Ok(())
}

#[test]
fn test_closing_envelope_is_counted() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 200)?;
	// Opening + 60 + 1 + 100 = 173 bytes written so far, under 200; only the
	// 60 byte closing pushes it over.
	store.append(item_of_len(60))?;
	store.append(item_of_len(100))?;

	let files = finished_files(&mut store)?;
	assert_eq!(batch_counts(&files), vec![1, 1]);
	assert_ceiling(&files, 200);
	Ok(())
}

#[test]
fn test_longer_write_key_rotates_earlier() -> Result<()> {
	let long_key = "k".repeat(40);
	let mut counts = Vec::new();
	for key in [KEY, long_key.as_str()] {
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(config(&temp_dir, 300, key))?;
		let appended: Vec<Value> = (0..20).map(|_| item_of_len(40)).collect();
		for item in &appended {
			store.append(item.clone())?;
		}
		let files = finished_files(&mut store)?;
		assert_eq!(assert_ceiling(&files, 300), appended);
		counts.push(batch_counts(&files));
	}
	// 72 + 41n - 1 <= 300 gives 5 per file; with 32 more envelope bytes, 4.
	assert_eq!(counts[0], vec![5, 5, 5, 5]);
	assert_eq!(counts[1], vec![4, 4, 4, 4, 4]);
	Ok(())
}

#[test]
fn test_multibyte_utf8_is_measured_in_bytes() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 256)?;
	let words = ["🦀", "漢字", "é", "👾💥", "日本語テキスト", "ascii"];
	let appended: Vec<Value> = (0..120)
		.map(|i| json!({"s": words[i % words.len()].repeat(1 + i % 7)}))
		.collect();
	for item in &appended {
		store.append(item.clone())?;
	}
	let chars: usize = appended.iter().map(|v| v.to_string().chars().count()).sum();
	let bytes: usize = appended.iter().map(item_len).sum();
	assert!(bytes > chars, "test data must be multi-byte");

	let files = finished_files(&mut store)?;
	assert_eq!(assert_ceiling(&files, 256), appended);
	Ok(())
}

#[test]
fn test_multibyte_exact_fit_and_overflow() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 100)?;
	// Six crabs (24 bytes) + é (2) + quotes (2) = 28 bytes, the most that fits; 9 chars.
	let fits = json!(format!("{}é", "🦀".repeat(6)));
	assert_eq!(item_len(&fits), 28);
	store.append(fits)?;
	// Seven crabs is 30 bytes but only 9 chars: rejected, counted in bytes.
	let err = store.append(json!("🦀".repeat(7))).unwrap_err();
	assert_invalid_input(&err);

	let files = finished_files(&mut store)?;
	assert_eq!(batch_counts(&files), vec![1]);
	assert_eq!(files[0].len, 100);
	Ok(())
}

#[test]
fn test_tracked_size_matches_bytes_on_disk() -> Result<()> {
	let temp_dir = TempDir::new()?;
	let mut store = store(&temp_dir, 4096)?;
	for i in 0..25 {
		store.append(json!({"i": i, "s": "🦀".repeat(i % 4)}))?;
		let path = store.current_path.clone().expect("open file");
		assert_eq!(fs::metadata(&path)?.len(), store.current_size as u64);
	}

	let tracked = store.current_size;
	let files = finished_files(&mut store)?;
	assert_eq!(files.len(), 1);
	assert_eq!(files[0].len, (tracked + store.closing_len) as u64);
	Ok(())
}
