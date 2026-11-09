use crate::{batch, DataResult, DataStore, Equivalent};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::any::Any;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Result, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

mod files;

impl Equivalent for PathBuf {
	fn equals(&self, other: &dyn Equivalent) -> bool {
		if let Some(other_path) = other.as_any().downcast_ref::<PathBuf>() {
			self == other_path
		} else {
			false
		}
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}

/// Configuration options for the file-based data store.
///
/// This struct provides the configuration parameters needed to create a new DirectoryStore instance.
/// It controls where and how files are stored, including file naming, size limits, and organization.
///
/// # Examples
/// ```
/// use std::path::PathBuf;
/// use transientdb::DirectoryConfig;
///
/// let config = DirectoryConfig {
///     write_key: "my-store".into(),
///     storage_location: PathBuf::from("/tmp/data"),
///     base_filename: "events".into(),
///     max_file_size: 1024 * 1024, // 1MB
///     max_file_items: Some(8),    // at most 8 items per file
/// };
/// ```
#[derive(Clone)]
pub struct DirectoryConfig {
	/// Key used to identify writes to this store.
	/// This is included in the metadata of each data file created by the store.
	pub write_key: String,
	/// Directory where data files will be stored.
	/// The store will create this directory if it doesn't exist.
	pub storage_location: PathBuf,
	/// Base name for generated files.
	/// Files will be created with the pattern: "{index}-{base_filename}"
	/// where index is an auto-incrementing number. A finished file has `.temp` appended
	/// to that whole name ("5-events.json" becomes "5-events.json.temp"), so
	/// `base_filename` must not itself end in `.temp`.
	///
	/// The store owns exactly the files with those two names and ignores everything else
	/// in `storage_location`, so stores with different `base_filename`s can share one
	/// directory without seeing, counting, finishing, fetching or deleting each other's
	/// files.
	pub base_filename: String,
	/// Maximum size in bytes of a finished data file on disk.
	///
	/// This is a hard ceiling: the whole finished file, including the JSON envelope
	/// (`batch` opening, `sentAt`, `writeKey`) and the commas between items, is never
	/// larger than this. When the next item would push the finished file past it, the
	/// open file is finished first and the item starts a new one. An item too large to
	/// fit even alone in a file is rejected by `append` with `ErrorKind::InvalidInput`.
	///
	/// Must be at least 100 bytes, and must leave room for at least a 1-byte item once
	/// the envelope is counted (the envelope grows with `write_key`'s length).
	pub max_file_size: usize,
	/// Maximum number of items in an individual data file.
	/// Once a file holds this many items, the next append starts a new file.
	/// This limit is independent of `max_file_size`; whichever is reached first rotates the file.
	/// `None` means files are limited by size only. Must not be `Some(0)`.
	pub max_file_items: Option<usize>,
}

/// Type alias for the file validator function
pub type FileValidator = Box<dyn Fn(&Path) -> Result<()> + Send + Sync>;

/// A data store that persists items to files in a directory.
///
/// Files are created with incrementing numerical prefixes and contain batched JSON data.
/// Before each append the store checks whether the item fits in the open file; if the
/// finished file would exceed `max_file_size` bytes or hold more than `max_file_items`
/// items, the open file is finished and the item starts a new one. So no finished file
/// is ever larger than `max_file_size`, envelope included. Items too large for any file
/// are rejected with `ErrorKind::InvalidInput`.
/// Completed files are marked by appending a .temp extension to their name, to indicate
/// they are ready for processing. Only files named for this store's `base_filename` are
/// ever read, finished, fetched or removed; see [`DirectoryConfig::base_filename`].
///
/// Each file contains a JSON object with:
/// - A `batch` array containing the stored items
/// - A `sentAt` timestamp in RFC3339 format
/// - The store's `writeKey`
pub struct DirectoryStore {
	config: DirectoryConfig,
	writer: Option<BufWriter<File>>,
	current_size: usize,
	current_items: usize,
	current_path: Option<PathBuf>,
	/// Length in bytes of `closing()`, which is the same for every timestamp.
	closing_len: usize,
	file_validator: Option<FileValidator>,
	next_index: AtomicU32,
}

impl DirectoryStore {
	const TEMP_EXTENSION: &'static str = "temp";
	const SEPARATOR: &'static [u8] = b",";

	/// Creates a new DirectoryStore with the specified configuration.
	///
	/// Creates the storage directory if it doesn't exist. The store will initialize
	/// its file index by scanning existing files in the directory.
	///
	/// # Arguments
	/// * `config` - Configuration options for the store
	///
	/// # Errors
	/// Returns an IO error if:
	/// - The storage directory cannot be created
	/// - The directory cannot be read when scanning for existing files
	///
	/// # Panics
	/// * If max_file_size is less than 100 bytes
	/// * If max_file_items is `Some(0)`
	/// * If max_file_size leaves no room for even a 1-byte item once the file's
	///   envelope (which grows with `write_key`) is counted
	/// * If base_filename ends in `.temp`: its open files would carry the name another
	///   store's finished files have, so ownership could not be told from the name
	pub fn new(config: DirectoryConfig) -> Result<Self> {
		if config.max_file_size < 100 {
			panic!("Seriously? max_file_size < 100 bytes? What exactly do you expect to store in there?");
		}
		if config.max_file_items == Some(0) {
			panic!("max_file_items = Some(0)? A file that holds nothing is just a very expensive empty directory.");
		}
		if config
			.base_filename
			.ends_with(&format!(".{}", Self::TEMP_EXTENSION))
		{
			panic!("base_filename ends in \".temp\"? That's the mark of a finished file; a name that looks done before it starts can't be told apart from one that is.");
		}

		let mut store = DirectoryStore {
			config,
			writer: None,
			current_size: 0,
			current_items: 0,
			current_path: None,
			closing_len: 0,
			file_validator: None,
			next_index: AtomicU32::new(0),
		};
		store.closing_len = store.closing(Utc::now()).len();
		if store.opening().len() + store.closing_len + 1 > store.config.max_file_size {
			panic!("max_file_size is all envelope and no room for an item? That's packaging, not storage.");
		}

		fs::create_dir_all(&store.config.storage_location)?;

		// Initialize directory and get max index
		let max_index = store.initialize_directory()?;
		store.next_index.store(max_index + 1, Ordering::SeqCst);

		Ok(store)
	}

	/// Sets a validator function that will be called before finalizing each data file.
	///
	/// The validator is called just before a file is marked as complete (renamed with .temp extension).
	/// If the validator returns an error, the file will not be finalized and the error will be propagated.
	///
	/// # Arguments
	/// * `validator` - Function that takes a file path and returns Ok(()) if the file is valid
	///
	/// # Examples
	/// ```
	/// use std::path::PathBuf;
	/// use std::fs;
	/// use std::io;
	/// use transientdb::{DirectoryConfig, DirectoryStore};
	///
	/// let mut store = DirectoryStore::new(DirectoryConfig {
	///     write_key: "test".into(),
	///     storage_location: PathBuf::from("/tmp/data"),
	///     base_filename: "events".into(),
	///     max_file_size: 1024,
	///     max_file_items: None,
	/// })?;
	///
	/// // Add a validator that checks file size and JSON validity
	/// store.set_file_validator(|path| {
	///     // Verify minimum file size
	///     let metadata = fs::metadata(path)?;
	///     if metadata.len() < 10 {
	///         return Err(io::Error::new(io::ErrorKind::Other, "File too small"));
	///     }
	///
	///     // Verify file contains valid JSON
	///     let content = fs::read_to_string(path)?;
	///     serde_json::from_str::<serde_json::Value>(&content)?;
	///
	///     Ok(())
	/// });
	/// # Ok::<(), std::io::Error>(())
	/// ```
	pub fn set_file_validator<F>(&mut self, validator: F)
	where
		F: Fn(&Path) -> Result<()> + 'static + Send + Sync,
	{
		self.file_validator = Some(Box::new(validator));
	}

	fn next_index(&self) -> u32 {
		self.next_index.fetch_add(1, Ordering::SeqCst)
	}

	/// Bytes written at the start of every data file.
	fn opening(&self) -> &'static [u8] {
		b"{ \"batch\": ["
	}

	/// Bytes written at the end of every data file when it is finished.
	///
	/// Its length does not depend on `sent_at`: the timestamp format is fixed-width.
	/// `write_key` is written as a JSON string, escaped as needed.
	fn closing(&self, sent_at: DateTime<Utc>) -> Vec<u8> {
		let write_key =
			serde_json::to_string(&self.config.write_key).expect("a string always serializes");
		format!(
			"],\"sentAt\":\"{}\",\"writeKey\":{}}}",
			sent_at.format("%Y-%m-%dT%H:%M:%S.%3fZ"),
			write_key
		)
		.into_bytes()
	}

	/// The largest serialized item that fits alone in a file.
	fn max_item_size(&self) -> usize {
		self.config.max_file_size - self.opening().len() - self.closing_len
	}

	/// Whether an item of `item_len` bytes can go into the open file without the
	/// finished file exceeding `max_file_size` or holding more than `max_file_items`.
	fn fits_in_open_file(&self, item_len: usize) -> bool {
		let item_limit_reached = self
			.config
			.max_file_items
			.is_some_and(|max| self.current_items >= max);
		let separator = if self.current_items > 0 {
			Self::SEPARATOR.len()
		} else {
			0
		};
		let finished_size = self.current_size + separator + item_len + self.closing_len;
		!item_limit_reached && finished_size <= self.config.max_file_size
	}

	fn start_file_if_needed(&mut self) -> Result<()> {
		if self.writer.is_some() {
			return Ok(());
		}

		let mut index = self.next_index();
		let mut attempts = 0;
		const MAX_ATTEMPTS: u32 = 1000; // Safeguard against infinite loops

		loop {
			let file_path = self
				.config
				.storage_location
				.join(self.unfinished_name(index));

			match OpenOptions::new()
				.write(true)
				.create_new(true)
				.open(&file_path)
			{
				Ok(file) => {
					let mut writer = BufWriter::new(file);
					writer.write_all(self.opening())?;
					self.current_path = Some(file_path);
					self.current_size = self.opening().len();
					self.current_items = 0;
					self.writer = Some(writer);
					return Ok(());
				}
				Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
					// File exists, try next index
					index = self.next_index();
					attempts += 1;

					if attempts >= MAX_ATTEMPTS {
						return Err(io::Error::other(
							"Failed to find available index after maximum attempts",
						));
					}
				}
				Err(e) => return Err(e), // Other errors are propagated
			}
		}
	}

	/// Scans the directory for this store's files, finalizes unfinished ones, and returns
	/// the highest index found
	fn initialize_directory(&self) -> Result<u32> {
		let files = self.owned_files()?;
		for file in files.iter().filter(|f| !f.finished) {
			if let Err(e) = self.recover_file(&file.path) {
				eprintln!("Failed to finalize file {:?}: {}", file.path, e);
				// Continue processing other files even if this one fails
			}
		}
		Ok(files.iter().map(|f| f.index).max().unwrap_or(0))
	}

	/// Finalizes a file an earlier session left without the `.temp` extension.
	///
	/// A file that is already a complete envelope, because its validator rejected it
	/// after the closing was written or the session ended before the rename, is not
	/// closed a second time. Any other file is closed as usual, even one damaged in
	/// some other way (such as cut off inside an item); that is not repaired.
	fn recover_file(&self, path: &Path) -> Result<()> {
		if Self::is_complete_envelope(&fs::read(path)?) {
			self.mark_finished(path)
		} else {
			self.finalize_file(path)
		}
	}

	/// Whether `content` already parses as a finished data file.
	fn is_complete_envelope(content: &[u8]) -> bool {
		serde_json::from_slice::<Value>(content).is_ok_and(|json| {
			json["batch"].is_array() && json["sentAt"].is_string() && json["writeKey"].is_string()
		})
	}

	/// Finalizes a file by completing the JSON structure and renaming with .temp extension
	fn finalize_file(&self, path: &Path) -> Result<()> {
		{
			let mut file = OpenOptions::new().append(true).open(path)?;
			let closing = self.closing(Utc::now());
			debug_assert_eq!(closing.len(), self.closing_len);
			file.write_all(&closing)?;
			file.flush()?;
		}
		self.mark_finished(path)
	}

	/// Validates a closed file, if a validator is set, and renames it with the .temp extension
	fn mark_finished(&self, path: &Path) -> Result<()> {
		// Run validation if configured
		if let Some(validator) = &self.file_validator {
			validator(path)?;
		}

		// Rename to .temp to mark as complete
		fs::rename(path, Self::finished_path(path))?;

		Ok(())
	}

	fn finish_file(&mut self) -> Result<()> {
		let Some(mut writer) = self.writer.take() else {
			return Ok(());
		};
		let current_path = self.current_path.take();

		// The file is closed from here on, even if flushing or finalizing it fails,
		// so the next append must start a fresh file with fresh accounting.
		self.current_size = 0;
		self.current_items = 0;

		writer.flush()?;
		// Drop the writer to close the file
		drop(writer);

		if let Some(current_path) = current_path {
			self.finalize_file(&current_path)?;
		}
		Ok(())
	}

	/// This store's files, oldest first: by numeric index, so "10-events" sorts after
	/// "9-events". Only finished files unless `include_unfinished`.
	fn sorted_files(&self, include_unfinished: bool) -> Result<Vec<PathBuf>> {
		Ok(self
			.owned_files()?
			.into_iter()
			.filter(|f| include_unfinished || f.finished)
			.map(|f| f.path)
			.collect())
	}

	fn up_to_size(&self, max_bytes: usize, files: &[PathBuf]) -> Result<Vec<PathBuf>> {
		let mut result = Vec::new();
		let mut total_size: u64 = 0;

		for file in files {
			if let Ok(metadata) = fs::metadata(file) {
				let size = metadata.len();
				if total_size + size <= max_bytes as u64 {
					result.push(file.clone());
					total_size += size;
				} else {
					break;
				}
			}
		}

		Ok(result)
	}
}

impl DataStore for DirectoryStore {
	type Output = Vec<PathBuf>;

	fn has_data(&self) -> bool {
		// Check if we have an active writer with data
		if self.writer.is_some() {
			return true;
		}

		// Any of this store's files, finished or not, is data
		self.owned_files().is_ok_and(|files| !files.is_empty())
	}

	fn reset(&mut self) {
		// The open file is about to be deleted, so discard it rather than finalizing it.
		// Otherwise later appends land in an unlinked file and the next fetch fails.
		self.writer = None;
		self.current_path = None;
		self.current_size = 0;
		self.current_items = 0;

		if let Ok(files) = self.sorted_files(true) {
			let _ = self.remove(
				&files
					.iter()
					.map(|p| Box::new(p.clone()) as Box<dyn Equivalent>)
					.collect::<Vec<_>>(),
			);
		}
	}

	/// Appends an item to the open file, rotating first if the item would not fit.
	///
	/// A finished file is never larger than `max_file_size` bytes, envelope and
	/// separators included, and never holds more than `max_file_items` items.
	///
	/// # Errors
	/// Returns an `InvalidInput` error, without touching any file, if the serialized
	/// item is too large to fit even alone in a fresh file. Also propagates IO errors
	/// and file validator errors from finishing the previous file.
	fn append(&mut self, data: Value) -> Result<()> {
		let item = serde_json::to_vec(&data)?;

		// Reject before touching anything: an item that cannot fit in any file must not
		// rotate the open file or create a new one.
		let max_item_size = self.max_item_size();
		if item.len() > max_item_size {
			return Err(batch::item_too_large(
				item.len(),
				max_item_size,
				"max_file_size",
				self.config.max_file_size,
			));
		}

		if self.writer.is_some() && !self.fits_in_open_file(item.len()) {
			self.finish_file()?;
		}
		self.start_file_if_needed()?;

		let writer = self
			.writer
			.as_mut()
			.ok_or_else(|| io::Error::other("No active writer"))?;
		if self.current_items > 0 {
			writer.write_all(Self::SEPARATOR)?;
			self.current_size += Self::SEPARATOR.len();
		}
		writer.write_all(&item)?;
		writer.flush()?;

		self.current_size += item.len();
		self.current_items += 1;
		Ok(())
	}

	fn fetch(
		&mut self,
		count: Option<usize>,
		max_bytes: Option<usize>,
	) -> Result<Option<DataResult<Self::Output>>> {
		if self.writer.is_some() {
			self.finish_file()?;
		}

		let mut files = self.sorted_files(false)?;

		if let Some(max_bytes) = max_bytes {
			files = self.up_to_size(max_bytes, &files)?;
		}

		if let Some(count) = count {
			files.truncate(count);
		}

		if files.is_empty() {
			return Ok(None);
		}

		let removable = files
			.iter()
			.map(|p| Box::new(p.clone()) as Box<dyn Equivalent>)
			.collect::<Vec<_>>();

		Ok(Some(DataResult {
			data: Some(files),
			removable: Some(removable),
		}))
	}

	fn remove(&mut self, data: &[Box<dyn Equivalent>]) -> Result<()> {
		for item in data {
			if let Some(path) = item.as_any().downcast_ref::<PathBuf>() {
				if let Err(e) = fs::remove_file(path) {
					eprintln!("Failed to remove file {:?}: {}", path, e);
				}
			}
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::{DirectoryConfig, DirectoryStore};
	use crate::DataStore;
	use serde_json::json;
	use serde_json::Value;
	use std::fs;
	use std::io;
	use std::io::Result;
	use tempfile::TempDir;

	mod ceiling;
	mod duplicates;
	mod lifecycle;
	mod oversized;
	mod ownership;
	mod recovery;
	mod support;
	mod write_key;

	#[test]
	fn test_directory_store_basic_operations() -> Result<()> {
		let temp_dir = TempDir::new()?;

		let config = DirectoryConfig {
			write_key: "test-key".to_string(),
			storage_location: temp_dir.path().to_owned(),
			base_filename: "events".to_string(),
			max_file_size: 1024,
			max_file_items: None,
		};

		let mut store = DirectoryStore::new(config)?;

		// Test append
		let event = serde_json::json!({
			"event": "test",
			"properties": {
				"value": 123
			}
		});
		store.append(event.clone())?;
		store.append(event.clone())?;

		// Force file completion
		if let Some(result) = store.fetch(None, None)? {
			assert!(result.data.is_some());
			let files = result.data.unwrap();
			assert_eq!(files.len(), 1);

			// Verify file content
			let content = fs::read_to_string(&files[0])?;
			assert!(content.contains("\"event\":\"test\""));
			assert!(content.contains("\"writeKey\":\"test-key\""));
		} else {
			panic!("Expected data but got none");
		}

		Ok(())
	}

	#[test]
	fn test_recovers_unfinished_files() -> Result<()> {
		let temp_dir = TempDir::new()?;

		// Create initial store instance
		let config = DirectoryConfig {
			write_key: "test-key".to_string(),
			storage_location: temp_dir.path().to_owned(),
			base_filename: "events".to_string(),
			max_file_size: 1024,
			max_file_items: None,
		};

		// First store instance - create some unfinished files
		{
			let mut store = DirectoryStore::new(config.clone())?;

			// Add some entries but don't finish_file()
			store.append(json!({"event": "test1", "value": 123}))?;
			store.append(json!({"event": "test2", "value": 456}))?;

			// Let store drop without explicit finish
		}

		// Create a finished file to ensure we don't touch it
		{
			let mut store = DirectoryStore::new(config.clone())?;
			store.append(json!({"event": "finished", "value": 789}))?;
			store.finish_file()?; // Explicitly finish this one
		}

		// Create new store instance which should trigger recovery
		let mut store = DirectoryStore::new(config)?;

		// Verify we can read all the data
		if let Some(result) = store.fetch(None, None)? {
			let files = result.data.unwrap();
			assert!(!files.is_empty(), "Should have recovered files");

			// Read and verify contents of all files
			for path in files {
				let content = fs::read_to_string(&path)?;
				let json: Value = serde_json::from_str(&content)?;

				// Verify structure
				assert!(json.get("batch").is_some(), "Should have batch field");
				assert!(json.get("sentAt").is_some(), "Should have sentAt field");
				assert_eq!(
					json.get("writeKey").and_then(Value::as_str),
					Some("test-key"),
					"Should have correct writeKey"
				);

				// All files should have .temp extension now
				assert_eq!(
					path.extension().and_then(|ext| ext.to_str()),
					Some("temp"),
					"All files should be finalized"
				);
			}
		}

		// Verify we can still append new data
		store.append(json!({"event": "new", "value": 999}))?;
		store.finish_file()?;

		Ok(())
	}

	#[test]
	fn test_file_rotation() -> Result<()> {
		let temp_dir = TempDir::new()?;

		let config = DirectoryConfig {
			write_key: "test-key".to_string(),
			storage_location: temp_dir.path().to_owned(),
			base_filename: "events".to_string(),
			// Each 79 byte item plus the 72 byte envelope: two items fit per file, not three.
			max_file_size: 250,
			max_file_items: None,
		};

		let mut store = DirectoryStore::new(config)?;

		// Add multiple events that should span files
		for i in 0..5 {
			let event = serde_json::json!({
				"event": "test",
				"index": i,
				"data": "some longer data to help hit size limit...."
			});
			store.append(event)?;
		}

		// Fetch all files
		if let Some(result) = store.fetch(None, None)? {
			let files = result.data.unwrap();
			assert!(files.len() > 1, "Expected multiple files due to size limit");
			assert_eq!(files.len(), 3);

			// Verify each file is properly formatted
			for file in &files {
				assert!(fs::metadata(file)?.len() <= 250, "File over max_file_size");
				let content = fs::read_to_string(file)?.trim().to_string();
				assert!(content.starts_with("{ \"batch\": ["));
				assert!(content.ends_with("}"));
				assert!(content.contains("\"writeKey\":\"test-key\""));

				// Validate JSON structure
				let parsed: serde_json::Value = serde_json::from_str(&content)?;
				assert!(parsed.get("batch").is_some(), "Missing 'batch' field");
				assert!(parsed.get("writeKey").is_some(), "Missing 'writeKey' field");
				assert!(parsed.get("sentAt").is_some(), "Missing 'sentAt' field");
			}
		} else {
			panic!("Expected data but got none");
		}

		Ok(())
	}

	#[test]
	fn test_fetch_with_size_limit() -> Result<()> {
		let temp_dir = TempDir::new()?;
		let config = DirectoryConfig {
			write_key: "test-key".to_string(),
			storage_location: temp_dir.path().to_owned(),
			base_filename: "events".to_string(),
			max_file_size: 200,
			max_file_items: None,
		};

		let mut store = DirectoryStore::new(config)?;

		// Create several files of known size
		for i in 0..5 {
			let event = serde_json::json!({
				"event": "test",
				"index": i,
				"data": "padding data...."
			});
			store.append(event)?;
		}

		// Fetch with byte limit
		if let Some(result) = store.fetch(None, Some(250))? {
			let files = result.data.unwrap();
			let total_size: u64 = files.iter().map(|f| fs::metadata(f).unwrap().len()).sum();
			assert!(total_size <= 250, "Fetch exceeded size limit");
		}

		Ok(())
	}

	#[test]
	fn test_file_cleanup() -> Result<()> {
		let temp_dir = TempDir::new()?;
		let config = DirectoryConfig {
			write_key: "test-key".to_string(),
			storage_location: temp_dir.path().to_owned(),
			base_filename: "events".to_string(),
			max_file_size: 200,
			max_file_items: None,
		};

		let mut store = DirectoryStore::new(config)?;

		// Add some events
		let event = serde_json::json!({"event": "test"});
		store.append(event)?;

		// Fetch and then remove
		if let Some(result) = store.fetch(None, None)? {
			let files = result.data.unwrap();
			let removable = result.removable.unwrap();

			// Verify files exist
			for file in &files {
				assert!(file.exists());
			}

			// Remove the files
			store.remove(&removable)?;

			// Verify files are gone
			for file in &files {
				assert!(!file.exists());
			}
		}

		Ok(())
	}

	#[test]
	fn test_file_validator() -> Result<()> {
		let temp_dir = TempDir::new()?;
		let config = DirectoryConfig {
			write_key: "test-key".to_string(),
			storage_location: temp_dir.path().to_owned(),
			base_filename: "events".to_string(),
			max_file_size: 1024,
			max_file_items: None,
		};

		let mut store = DirectoryStore::new(config)?;

		// Set a validator that ensures files are larger than 10 bytes
		store.set_file_validator(|path| {
			let metadata = fs::metadata(path)?;
			if metadata.len() < 10 {
				return Err(io::Error::other("File too small"));
			}
			Ok(())
		});

		let event = serde_json::json!({"event": "test"});
		store.append(event)?;

		// Fetch should trigger validation
		let result = store.fetch(None, None)?;
		assert!(result.is_some());

		Ok(())
	}

	#[test]
	fn test_fetch_limits() -> Result<()> {
		let temp_dir = TempDir::new()?;
		let config = DirectoryConfig {
			write_key: "test-key".to_string(),
			storage_location: temp_dir.path().to_owned(),
			base_filename: "events".to_string(),
			// Small size to force multiple files: one 72 byte item plus the 72 byte
			// envelope per file.
			max_file_size: 150,
			max_file_items: None,
		};

		let mut store = DirectoryStore::new(config)?;

		// Create 10 files with predictable sizes
		for i in 0..10 {
			let event = serde_json::json!({
				"event": "test",
				"index": i,
				"data": "padding data to make files bigger..."
			});
			store.append(event)?;
		}

		// Test count limit only
		if let Some(result) = store.fetch(Some(3), None)? {
			let files = result.data.unwrap();
			assert_eq!(files.len(), 3, "Count limit not respected");
		}

		// Test byte limit only
		if let Some(result) = store.fetch(None, Some(250))? {
			let files = result.data.unwrap();
			let total_size: u64 = files.iter().map(|f| fs::metadata(f).unwrap().len()).sum();
			assert!(total_size <= 250, "Byte limit not respected");
		}

		// Test both count and byte limits
		if let Some(result) = store.fetch(Some(5), Some(200))? {
			let files = result.data.unwrap();
			assert!(
				files.len() <= 5,
				"Count limit not respected in combined test"
			);

			let total_size: u64 = files.iter().map(|f| fs::metadata(f).unwrap().len()).sum();
			assert!(
				total_size <= 200,
				"Byte limit not respected in combined test"
			);
		}

		Ok(())
	}

	#[test]
	#[should_panic(
		expected = "Seriously? max_file_size < 100 bytes? What exactly do you expect to store in there?"
	)]
	fn test_rejects_tiny_max_file_size() {
		let temp_dir = TempDir::new().unwrap();
		let config = DirectoryConfig {
			write_key: "test-key".to_string(),
			storage_location: temp_dir.path().to_owned(),
			base_filename: "events".to_string(),
			max_file_size: 50, // Try to use a ridiculously small size
			max_file_items: None,
		};

		// This should panic
		let _store = DirectoryStore::new(config).unwrap();
	}

	#[test]
	#[should_panic(
		expected = "Seriously? max_file_size < 100 bytes? What exactly do you expect to store in there?"
	)]
	fn test_rejects_zero_max_file_size() {
		let temp_dir = TempDir::new().unwrap();
		let config = DirectoryConfig {
			write_key: "test-key".to_string(),
			storage_location: temp_dir.path().to_owned(),
			base_filename: "events".to_string(),
			max_file_size: 0, // Try to use zero
			max_file_items: None,
		};

		// This should panic
		let _store = DirectoryStore::new(config).unwrap();
	}

	fn item_limit_config(dir: &TempDir, max_file_items: Option<usize>) -> DirectoryConfig {
		DirectoryConfig {
			write_key: "test-key".to_string(),
			storage_location: dir.path().to_owned(),
			base_filename: "events".to_string(),
			max_file_size: 1024 * 1024,
			max_file_items,
		}
	}

	/// Parses a finished file, checks the envelope, and returns its `batch` array.
	fn read_batch(path: &std::path::Path) -> Result<Vec<Value>> {
		let content = fs::read_to_string(path)?;
		assert!(
			content.starts_with("{ \"batch\": ["),
			"Bad envelope start: {content}"
		);
		assert!(!content.contains("[,"), "Leading comma: {content}");
		assert!(!content.contains(",,"), "Doubled comma: {content}");
		let parsed: Value = serde_json::from_str(&content)?;
		assert!(parsed.get("sentAt").and_then(Value::as_str).is_some());
		assert_eq!(
			parsed.get("writeKey").and_then(Value::as_str),
			Some("test-key")
		);
		Ok(parsed["batch"].as_array().expect("batch array").clone())
	}

	/// Fetches every finished file and returns the item count of each, in file order.
	fn fetch_counts(store: &mut DirectoryStore) -> Result<Vec<usize>> {
		let Some(result) = store.fetch(None, None)? else {
			return Ok(Vec::new());
		};
		let files = result.data.unwrap();
		assert_eq!(files.len(), result.removable.unwrap().len());
		files.iter().map(|f| Ok(read_batch(f)?.len())).collect()
	}

	fn append_indexed(store: &mut DirectoryStore, range: std::ops::Range<usize>) -> Result<()> {
		for i in range {
			store.append(json!({"event": "test", "index": i}))?;
		}
		Ok(())
	}

	#[test]
	fn test_item_limit_exact_n_single_file() -> Result<()> {
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(item_limit_config(&temp_dir, Some(5)))?;
		append_indexed(&mut store, 0..5)?;
		assert_eq!(fetch_counts(&mut store)?, vec![5]);
		Ok(())
	}

	#[test]
	fn test_item_limit_n_plus_one_two_files() -> Result<()> {
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(item_limit_config(&temp_dir, Some(5)))?;
		append_indexed(&mut store, 0..6)?;
		assert_eq!(fetch_counts(&mut store)?, vec![5, 1]);
		Ok(())
	}

	#[test]
	fn test_item_limit_multiples_and_remainders() -> Result<()> {
		for n in [2usize, 3, 5] {
			for k in 1..=4usize {
				for r in 0..n {
					let temp_dir = TempDir::new()?;
					let mut store = DirectoryStore::new(item_limit_config(&temp_dir, Some(n)))?;
					append_indexed(&mut store, 0..k * n + r)?;

					let mut expected = vec![n; k];
					if r > 0 {
						expected.push(r);
					}
					let counts = fetch_counts(&mut store)?;
					assert_eq!(counts, expected, "n={n} k={k} r={r}");
					assert!(counts.iter().all(|&c| c > 0 && c <= n));
				}
			}
		}
		Ok(())
	}

	#[test]
	fn test_item_limit_of_one() -> Result<()> {
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(item_limit_config(&temp_dir, Some(1)))?;
		append_indexed(&mut store, 0..7)?;
		assert_eq!(fetch_counts(&mut store)?, vec![1; 7]);
		Ok(())
	}

	#[test]
	fn test_item_limit_eight_with_twenty_items() -> Result<()> {
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(item_limit_config(&temp_dir, Some(8)))?;
		append_indexed(&mut store, 0..20)?;
		assert_eq!(fetch_counts(&mut store)?, vec![8, 8, 4]);
		Ok(())
	}

	#[test]
	fn test_item_limit_preserves_append_order() -> Result<()> {
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(item_limit_config(&temp_dir, Some(3)))?;
		append_indexed(&mut store, 0..11)?;

		let files = store.fetch(None, None)?.unwrap().data.unwrap();
		let mut indexes = Vec::new();
		for file in &files {
			for item in read_batch(file)? {
				indexes.push(item["index"].as_u64().unwrap() as usize);
			}
		}
		assert_eq!(indexes, (0..11).collect::<Vec<_>>());
		Ok(())
	}

	#[test]
	fn test_no_item_limit_keeps_small_items_in_one_file() -> Result<()> {
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(item_limit_config(&temp_dir, None))?;
		append_indexed(&mut store, 0..500)?;
		assert_eq!(fetch_counts(&mut store)?, vec![500]);
		Ok(())
	}

	#[test]
	fn test_size_limit_trips_before_item_limit() -> Result<()> {
		let temp_dir = TempDir::new()?;
		let mut config = item_limit_config(&temp_dir, Some(8));
		config.max_file_size = 200;
		let mut store = DirectoryStore::new(config)?;

		// Each item is 53 bytes and the envelope 72, so two items (179 bytes with the
		// separator) fit under 200 and a third (233) does not, long before the item
		// limit of 8 is reached.
		for i in 0..9 {
			store.append(json!({"index": i, "data": "padding to reach the size limit."}))?;
		}
		let counts = fetch_counts(&mut store)?;
		assert_eq!(counts, vec![2, 2, 2, 2, 1]);
		assert!(counts.iter().all(|&c| c <= 8));
		Ok(())
	}

	#[test]
	fn test_item_limit_trips_before_size_limit() -> Result<()> {
		let temp_dir = TempDir::new()?;
		let mut config = item_limit_config(&temp_dir, Some(3));
		config.max_file_size = 10_000;
		let mut store = DirectoryStore::new(config)?;

		for i in 0..10 {
			store.append(json!({"index": i, "data": "padding to reach the size limit."}))?;
		}
		let counts = fetch_counts(&mut store)?;
		assert_eq!(counts, vec![3, 3, 3, 1]);
		Ok(())
	}

	#[test]
	fn test_item_limit_rotation_is_lazy() -> Result<()> {
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(item_limit_config(&temp_dir, Some(4)))?;
		append_indexed(&mut store, 0..4)?;

		// The file that just reached the limit stays open: one unfinished file, nothing finished.
		let on_disk = store.sorted_files(true)?;
		assert_eq!(on_disk.len(), 1);
		assert_ne!(
			on_disk[0].extension().and_then(|e| e.to_str()),
			Some(DirectoryStore::TEMP_EXTENSION)
		);
		assert!(store.sorted_files(false)?.is_empty());

		// Fetch closes it; still exactly one file, and no empty file was created.
		assert_eq!(fetch_counts(&mut store)?, vec![4]);
		assert_eq!(store.sorted_files(true)?.len(), 1);

		// Nothing appended since: the next fetch must not produce an empty file.
		let files = store.fetch(None, None)?.unwrap().data.unwrap();
		assert_eq!(files.len(), 1);
		Ok(())
	}

	#[test]
	fn test_item_count_resets_after_fetch() -> Result<()> {
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(item_limit_config(&temp_dir, Some(5)))?;

		append_indexed(&mut store, 0..4)?;
		let result = store.fetch(None, None)?.unwrap();
		store.remove(&result.removable.unwrap())?;

		append_indexed(&mut store, 4..9)?;
		assert_eq!(fetch_counts(&mut store)?, vec![5]);
		Ok(())
	}

	#[test]
	fn test_item_count_resets_after_reset() -> Result<()> {
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(item_limit_config(&temp_dir, Some(5)))?;

		append_indexed(&mut store, 0..4)?;
		store.reset();
		assert!(!store.has_data());
		assert!(store.sorted_files(true)?.is_empty());

		append_indexed(&mut store, 4..9)?;
		let files = store.fetch(None, None)?.unwrap().data.unwrap();
		assert_eq!(files.len(), 1);
		let batch = read_batch(&files[0])?;
		let indexes: Vec<u64> = batch.iter().map(|v| v["index"].as_u64().unwrap()).collect();
		assert_eq!(indexes, vec![4, 5, 6, 7, 8]);
		Ok(())
	}

	#[test]
	fn test_item_count_starts_at_zero_after_recovery() -> Result<()> {
		let temp_dir = TempDir::new()?;
		{
			let mut store = DirectoryStore::new(item_limit_config(&temp_dir, Some(3)))?;
			append_indexed(&mut store, 0..2)?;
			// Dropped with an unfinished file holding 2 items.
		}

		let mut store = DirectoryStore::new(item_limit_config(&temp_dir, Some(3)))?;
		append_indexed(&mut store, 2..5)?;
		// Recovered file keeps its 2 items; the new store fills a fresh file with 3.
		assert_eq!(fetch_counts(&mut store)?, vec![2, 3]);
		Ok(())
	}

	#[test]
	fn test_item_limit_files_are_valid_json() -> Result<()> {
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(item_limit_config(&temp_dir, Some(2)))?;
		store.append(json!(null))?;
		store.append(json!("string, with comma"))?;
		store.append(json!([1, 2, 3]))?;
		store.append(json!({"key": "value"}))?;
		store.append(json!(42))?;

		let files = store.fetch(None, None)?.unwrap().data.unwrap();
		assert_eq!(files.len(), 3);
		for file in &files {
			assert_eq!(
				file.extension().and_then(|e| e.to_str()),
				Some(DirectoryStore::TEMP_EXTENSION)
			);
			// read_batch asserts the envelope and parses the JSON.
			assert!(!read_batch(file)?.is_empty());
		}
		Ok(())
	}

	#[test]
	fn test_validator_runs_on_item_limit_rotation() -> Result<()> {
		use std::sync::atomic::{AtomicUsize, Ordering};
		use std::sync::Arc;

		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(item_limit_config(&temp_dir, Some(2)))?;
		let validated = Arc::new(AtomicUsize::new(0));
		let counter = validated.clone();
		store.set_file_validator(move |path| {
			let content = fs::read_to_string(path)?;
			serde_json::from_str::<Value>(&content)?;
			counter.fetch_add(1, Ordering::SeqCst);
			Ok(())
		});

		append_indexed(&mut store, 0..5)?;
		// Two files were closed by item-limit rotation; the third is still open.
		assert_eq!(validated.load(Ordering::SeqCst), 2);

		assert_eq!(fetch_counts(&mut store)?, vec![2, 2, 1]);
		assert_eq!(validated.load(Ordering::SeqCst), 3);
		Ok(())
	}

	#[test]
	fn test_validator_failure_on_item_limit_rotation_propagates() -> Result<()> {
		let temp_dir = TempDir::new()?;
		let mut store = DirectoryStore::new(item_limit_config(&temp_dir, Some(2)))?;
		store.set_file_validator(|_| Err(io::Error::other("rejected")));

		append_indexed(&mut store, 0..2)?;
		let err = store.append(json!({"index": 2})).unwrap_err();
		assert_eq!(err.to_string(), "rejected");
		Ok(())
	}

	#[test]
	#[should_panic(
		expected = "max_file_items = Some(0)? A file that holds nothing is just a very expensive empty directory."
	)]
	fn test_rejects_zero_max_file_items() {
		let temp_dir = TempDir::new().unwrap();
		let _store = DirectoryStore::new(item_limit_config(&temp_dir, Some(0))).unwrap();
	}
}
