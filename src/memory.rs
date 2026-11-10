use crate::{batch, DataResult, DataStore, Equivalent};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::json;
use serde_json::Value;
use std::any::Any;
use std::collections::{HashSet, VecDeque};
use std::io::Result;

impl Equivalent for Value {
	fn equals(&self, other: &dyn Equivalent) -> bool {
		if let Some(other_value) = other.as_any().downcast_ref::<Value>() {
			self == other_value
		} else {
			false
		}
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}

/// A queued item and its sequence number, unique within its store.
#[derive(Clone, Debug, PartialEq)]
struct QueuedItem {
	seq: u64,
	value: Value,
}

/// A removable from `MemoryStore::fetch`: the sequence number of one fetched item.
#[derive(Debug, PartialEq)]
struct MemoryKey(u64);

impl Equivalent for MemoryKey {
	fn equals(&self, other: &dyn Equivalent) -> bool {
		other.as_any().downcast_ref::<MemoryKey>() == Some(self)
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}

/// Configuration options for the in-memory data store.
///
/// This struct provides the configuration parameters needed to create a new MemoryStore instance.
/// It controls the store's identification, capacity, and fetch size and item limits.
///
/// # Examples
/// ```
/// use transientdb::{MemoryConfig, MemoryStore};
///
/// let config = MemoryConfig {
///     write_key: "my-store".into(),
///     max_items: 1000,
///     max_fetch_size: 1024 * 1024, // 1MB
///     max_fetch_items: Some(8),    // at most 8 items per fetch
/// };
///
/// let store = MemoryStore::new(config);
/// ```
#[derive(Clone)]
pub struct MemoryConfig {
	/// Key used to identify writes to this store.
	/// This is included in the metadata of each batch of data fetched from the store.
	pub write_key: String,
	/// Total capacity: the maximum number of items held in the store before old items are removed.
	/// Once this limit is reached, adding new items will remove the oldest items to make space.
	/// This does not limit how many items a single fetch returns; see `max_fetch_items`.
	pub max_items: usize,
	/// Maximum size in bytes of the batch a single fetch returns, unless `fetch` is given
	/// its own `max_bytes`.
	///
	/// This is a hard ceiling on the whole batch serialized compactly with
	/// `serde_json::to_string`: envelope (`sentAt`, `writeKey`), every item, and every
	/// separator count. An item too large to fit in a batch by itself under this limit is
	/// rejected by `append` with `ErrorKind::InvalidInput`, so every stored item can be
	/// fetched.
	///
	/// Must be at least 100 bytes, and must leave room for at least a 1-byte item once the
	/// envelope is counted (the envelope grows with `write_key`'s length).
	pub max_fetch_size: usize,
	/// Maximum number of items returned by a single fetch (the size of one batch).
	/// When `fetch` is also given a `count`, the smaller of the two applies.
	/// This is unrelated to store capacity; see `max_items`.
	/// `None` means fetches are limited only by `count` and bytes. Must not be `Some(0)`.
	pub max_fetch_items: Option<usize>,
}

/// A FIFO data store that keeps items in memory.
///
/// Items are stored until a fetched batch is passed to `remove`, or until max_items is
/// reached, at which point older items are removed to make space for new ones. `fetch`
/// itself removes nothing. The store maintains data in a queue structure and supports
/// concurrent access.
///
/// When items are fetched, they are packaged into a JSON batch with metadata including:
/// - A batch array containing the items
/// - A timestamp indicating when the batch was created
/// - The store's write key
///
/// A fetched batch, serialized compactly, is never larger than the byte limit in effect
/// for that fetch.
///
/// Every appended item is a distinct entry, even if it equals another: `remove` deletes
/// exactly the items a fetch returned, never an identical item still waiting.
pub struct MemoryStore {
	config: MemoryConfig,
	items: VecDeque<QueuedItem>,
	/// The sequence number the next appended item gets
	next_seq: u64,
	/// The largest serialized item that fits alone in a batch under `max_fetch_size`,
	/// whatever the timestamp.
	max_item_size: usize,
}

impl MemoryStore {
	/// Creates a new MemoryStore with the specified configuration.
	///
	/// # Panics
	/// * If max_fetch_size is less than 100 bytes
	/// * If max_items is 0
	/// * If max_fetch_items is `Some(0)`
	/// * If max_fetch_size leaves no room for even a 1-byte item once the batch
	///   envelope (which grows with `write_key`) is counted
	pub fn new(config: MemoryConfig) -> Self {
		if config.max_fetch_size < 100 {
			panic!("max_fetch_size < 100 bytes? What are you even trying to fetch, empty arrays?");
		}
		if config.max_items == 0 {
			panic!("max_items = 0? So... you want a store that stores nothing? That's what /dev/null is for.");
		}
		if config.max_fetch_items == Some(0) {
			panic!("max_fetch_items = Some(0)? Fetching nothing, efficiently, forever. Bold.");
		}
		let envelope_len = Self::widest_envelope_len(&config.write_key);
		if envelope_len + 1 > config.max_fetch_size {
			panic!("max_fetch_size is all envelope and no room for an item? That's packaging, not a batch.");
		}

		Self {
			max_item_size: config.max_fetch_size - envelope_len,
			config,
			items: VecDeque::new(),
			next_seq: 0,
		}
	}

	/// The `sentAt` value for a batch created at `now`.
	fn sent_at(now: DateTime<Utc>) -> String {
		now.to_rfc3339()
	}

	/// The envelope length with the widest `sentAt` this store can produce.
	///
	/// RFC3339 output drops trailing zero groups from the fractional seconds, so its
	/// width varies; a timestamp with nonzero nanoseconds prints all nine digits.
	fn widest_envelope_len(write_key: &str) -> usize {
		let widest = Utc
			.timestamp_opt(0, 123_456_789)
			.single()
			.expect("valid timestamp");
		batch::envelope_len(&Self::sent_at(widest), write_key)
	}

	/// Creates a JSON batch object containing the provided items and metadata.
	///
	/// # Arguments
	/// * `items` - Slice of JSON values to include in the batch
	/// * `sent_at` - The batch's `sentAt` value, the same one its size was measured with
	///
	/// # Returns
	/// A JSON value containing:
	/// - A `batch` array of the provided items
	/// - A `sentAt` timestamp in RFC3339 format
	/// - The store's `writeKey`
	fn create_batch(&self, items: &[&Value], sent_at: &str) -> Value {
		json!({
			"batch": items,
			"sentAt": sent_at,
			"writeKey": self.config.write_key
		})
	}

	fn get_item_size(item: &Value) -> usize {
		item.to_string().len()
	}

	/// `fetch` with the batch's `sentAt` supplied, so its envelope length is known.
	fn fetch_with_sent_at(
		&mut self,
		count: Option<usize>,
		max_bytes: Option<usize>,
		sent_at: &str,
	) -> Option<DataResult<Value>> {
		// Unlike WebStore, no head item can be too large for max_fetch_size: append
		// rejects such items and nothing else adds to the queue.
		let max_bytes = max_bytes.unwrap_or(self.config.max_fetch_size);
		let count = batch::effective_count(count, self.config.max_fetch_items);
		let envelope_len = batch::envelope_len(sent_at, &self.config.write_key);
		let num_items = batch::leading_items_that_fit(
			self.items
				.iter()
				.map(|item| Self::get_item_size(&item.value)),
			envelope_len,
			max_bytes,
			count,
		);

		if num_items == 0 {
			return None;
		}

		let fetched = self.items.iter().take(num_items);
		let items: Vec<&Value> = fetched.clone().map(|item| &item.value).collect();
		let removable: Vec<Box<dyn Equivalent>> = fetched
			.map(|item| Box::new(MemoryKey(item.seq)) as Box<dyn Equivalent>)
			.collect();

		Some(DataResult {
			data: Some(self.create_batch(&items, sent_at)),
			removable: Some(removable),
		})
	}
}

impl DataStore for MemoryStore {
	type Output = Value;

	fn has_data(&self) -> bool {
		!self.items.is_empty()
	}

	fn reset(&mut self) {
		self.items.clear();
	}

	/// Appends an item, dropping the oldest items beyond `max_items`.
	///
	/// # Errors
	/// Returns an `InvalidInput` error, leaving the store unchanged, if the serialized
	/// item cannot fit in a batch by itself under `max_fetch_size`, or if it is nested
	/// more than 125 levels deep in arrays and objects, too deep to read back from a
	/// batch. Returns an error,
	/// leaving the store unchanged, if sequence numbers are exhausted: they never wrap,
	/// so no two items can share one.
	fn append(&mut self, data: Value) -> Result<()> {
		// First: serializing a value nested too deeply could overflow the stack.
		let data = batch::within_depth(data)?;
		let item_size = Self::get_item_size(&data);
		if item_size > self.max_item_size {
			return Err(batch::item_too_large(
				item_size,
				self.max_item_size,
				"max_fetch_size",
				self.config.max_fetch_size,
			));
		}
		let next_seq = self
			.next_seq
			.checked_add(1)
			.ok_or_else(|| std::io::Error::other("MemoryStore sequence numbers exhausted"))?;

		self.items.push_back(QueuedItem {
			seq: self.next_seq,
			value: data,
		});
		self.next_seq = next_seq;

		while self.items.len() > self.config.max_items {
			self.items.pop_front();
		}

		Ok(())
	}

	/// Returns the oldest items as one batch, without removing them.
	///
	/// The batch, serialized compactly, is at most `max_bytes` (or `max_fetch_size` if
	/// `None`) bytes, envelope included, and holds at most `count` and `max_fetch_items`
	/// items. A `max_bytes` too small for the oldest item returns `Ok(None)` even though
	/// the store has data; a fetch at the configured `max_fetch_size` always returns it.
	fn fetch(
		&mut self,
		count: Option<usize>,
		max_bytes: Option<usize>,
	) -> Result<Option<DataResult<Self::Output>>> {
		let sent_at = Self::sent_at(Utc::now());
		Ok(self.fetch_with_sent_at(count, max_bytes, &sent_at))
	}

	/// Removes exactly the items a previous `fetch` returned.
	///
	/// Identical items that were not fetched stay queued. Items already gone (removed
	/// before, or dropped by `max_items` since the fetch) are skipped.
	fn remove(&mut self, data: &[Box<dyn Equivalent>]) -> Result<()> {
		let fetched: HashSet<u64> = data
			.iter()
			.filter_map(|removable| removable.as_any().downcast_ref::<MemoryKey>())
			.map(|key| key.0)
			.collect();
		self.items.retain(|item| !fetched.contains(&item.seq));
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use crate::memory::{MemoryConfig, MemoryStore};
	use crate::DataStore;
	use serde_json::{json, Value};
	use std::io::Result;

	mod ceiling;
	mod depth;
	mod duplicates;
	mod oversized;
	mod repeat;
	mod support;
	mod write_key;

	#[test]
	fn test_basic_operations() -> Result<()> {
		let config = MemoryConfig {
			write_key: "test-key".to_string(),
			max_items: 1000,
			max_fetch_size: 1024,
			max_fetch_items: None,
		};

		let mut store = MemoryStore::new(config);

		// Test empty state
		assert!(!store.has_data());

		// Test append
		let event = json!({"event": "test", "value": 123});
		store.append(event.clone())?;
		assert!(store.has_data());

		// Test fetch - data should still be there after fetch
		if let Some(result) = store.fetch(None, None)? {
			let batch: Value = result.data.unwrap();
			let items = batch["batch"].as_array().unwrap();
			assert_eq!(items.len(), 1);
			assert_eq!(items[0]["value"], 123);

			// Verify items are still in store after fetch
			assert!(store.has_data());

			// Now remove the items
			if let Some(removable) = result.removable {
				store.remove(&removable)?;
				// Verify items were removed
				assert!(!store.has_data());
			} else {
				panic!("Expected removable items but got none");
			}
		} else {
			panic!("Expected data but got none");
		}

		Ok(())
	}

	#[test]
	fn test_fifo_behavior() -> Result<()> {
		let config = MemoryConfig {
			write_key: "test-key".to_string(),
			max_items: 3, // Small limit to test FIFO
			max_fetch_size: 1024,
			max_fetch_items: None,
		};

		let mut store = MemoryStore::new(config);

		// Add more items than max_items
		for i in 0..5 {
			store.append(json!({"index": i}))?;
		}

		// Should only have last 3 items
		assert!(store.has_data());

		// Verify they're the right items (2,3,4)
		if let Some(result) = store.fetch(None, None)? {
			let batch: Value = result.data.unwrap();
			let items = batch["batch"].as_array().unwrap();
			assert_eq!(items.len(), 3);
			assert_eq!(items[0]["index"], 2);
			assert_eq!(items[1]["index"], 3);
			assert_eq!(items[2]["index"], 4);
		}

		Ok(())
	}

	#[test]
	fn test_fetch_limits() -> Result<()> {
		let config = MemoryConfig {
			write_key: "test-key".to_string(),
			max_items: 100,
			max_fetch_size: 1000,
			max_fetch_items: None,
		};

		let mut store = MemoryStore::new(config);

		// Add items with predictable sizes
		for i in 0..10 {
			let padding = "x".repeat(50); // Each item will be roughly ~70 bytes
			store.append(json!({
				"index": i,
				"padding": padding
			}))?;
		}

		// Test count limit
		if let Some(result) = store.fetch(Some(3), None)? {
			let batch: Value = result.data.unwrap();
			let items = batch["batch"].as_array().unwrap();
			assert_eq!(items.len(), 3, "Count limit not respected");
		}

		// Add more items for size limit test
		for i in 0..10 {
			let padding = "x".repeat(50);
			store.append(json!({
				"index": i,
				"padding": padding
			}))?;
		}

		// Test byte limit (200 bytes should get us about 2-3 items)
		if let Some(result) = store.fetch(None, Some(200))? {
			let items = result.data.unwrap();
			let items = items["batch"].as_array().unwrap();
			assert!(items.len() <= 3, "Too many items for byte limit");

			// Each raw item should be under the limit
			let total_raw_size: usize = items.iter().map(MemoryStore::get_item_size).sum();
			assert!(total_raw_size <= 200, "Raw items exceed byte limit");
		}

		Ok(())
	}

	#[test]
	fn test_reset() -> Result<()> {
		let config = MemoryConfig {
			write_key: "test-key".to_string(),
			max_items: 100,
			max_fetch_size: 1000,
			max_fetch_items: None,
		};

		let mut store = MemoryStore::new(config);

		// Add some items
		for i in 0..5 {
			store.append(json!({"index": i}))?;
		}
		// we have 5 items
		assert!(store.has_data());

		// Reset and verify
		store.reset();
		assert!(!store.has_data());

		Ok(())
	}

	#[test]
	fn test_memory_store_max_fetch_size_edge_cases() -> Result<()> {
		let config = MemoryConfig {
			write_key: "test-key".to_string(),
			max_items: 100,
			max_fetch_size: 300, // Reasonable size that's easy to stay under/go over
			max_fetch_items: None,
		};

		let mut store = MemoryStore::new(config);

		// Small item ~30-40 bytes
		store.append(json!({"type": "small", "value": "tiny"}))?;

		// Large item ~430 bytes can never fit in a 300 byte batch, so append rejects it
		// rather than letting it sit unfetchable at the head of the queue.
		let err = store
			.append(json!({
				"type": "large",
				"value": "x".repeat(400),  // Make it obviously too big
			}))
			.unwrap_err();
		assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);

		// First fetch should only get the small item
		let result = store.fetch(None, None)?.expect("small item");
		let batch: Value = result.data.unwrap();
		let items = batch["batch"].as_array().unwrap();
		assert_eq!(items.len(), 1, "Should only fetch the small item");
		assert_eq!(items[0]["type"], "small");
		store.remove(&result.removable.unwrap())?;

		// Nothing is left behind: the large item was never stored.
		assert!(!store.has_data());
		assert!(store.fetch(None, None)?.is_none());

		Ok(())
	}

	#[test]
	fn test_memory_store_json_types() -> Result<()> {
		let config = MemoryConfig {
			write_key: "test-key".to_string(),
			max_items: 100,
			max_fetch_size: 1024,
			max_fetch_items: None,
		};

		let mut store = MemoryStore::new(config);

		// Test all JSON types
		store.append(json!(null))?;
		store.append(json!(true))?;
		store.append(json!(42))?;
		store.append(json!(42.5))?;
		store.append(json!("string"))?;
		store.append(json!([1, 2, 3]))?;
		store.append(json!({"key": "value"}))?;

		if let Some(result) = store.fetch(None, None)? {
			let batch: Value = result.data.unwrap();
			let items = batch["batch"].as_array().unwrap();
			assert_eq!(
				items.len(),
				7,
				"All JSON types should be stored and retrieved"
			);
		}

		Ok(())
	}

	#[test]
	#[should_panic(
		expected = "max_fetch_size < 100 bytes? What are you even trying to fetch, empty arrays?"
	)]
	fn test_rejects_tiny_max_fetch_size() {
		let config = MemoryConfig {
			write_key: "test-key".to_string(),
			max_items: 1000,
			max_fetch_size: 50, // Ridiculously small
			max_fetch_items: None,
		};

		let _store = MemoryStore::new(config);
	}

	#[test]
	#[should_panic(
		expected = "max_items = 0? So... you want a store that stores nothing? That's what /dev/null is for."
	)]
	fn test_rejects_zero_max_items() {
		let config = MemoryConfig {
			write_key: "test-key".to_string(),
			max_items: 0, // Why even bother?
			max_fetch_size: 1024,
			max_fetch_items: None,
		};

		let _store = MemoryStore::new(config);
	}

	fn item_limit_store(max_fetch_items: Option<usize>) -> MemoryStore {
		MemoryStore::new(MemoryConfig {
			write_key: "test-key".to_string(),
			max_items: 1000,
			max_fetch_size: 1024 * 1024,
			max_fetch_items,
		})
	}

	fn append_indexed(store: &mut MemoryStore, range: std::ops::Range<i64>) -> Result<()> {
		for i in range {
			store.append(json!({"index": i}))?;
		}
		Ok(())
	}

	/// Fetches and returns the indexes in the batch, asserting removable matches the batch.
	fn fetch_indexes(
		store: &mut MemoryStore,
		count: Option<usize>,
		max_bytes: Option<usize>,
	) -> Result<Vec<i64>> {
		let Some(result) = store.fetch(count, max_bytes)? else {
			return Ok(Vec::new());
		};
		let batch = result.data.unwrap();
		let items = batch["batch"].as_array().unwrap();
		assert_eq!(items.len(), result.removable.unwrap().len());
		Ok(items.iter().map(|v| v["index"].as_i64().unwrap()).collect())
	}

	#[test]
	fn test_fetch_item_limit_returns_oldest_n_in_order() -> Result<()> {
		let mut store = item_limit_store(Some(4));
		append_indexed(&mut store, 0..10)?;
		assert_eq!(fetch_indexes(&mut store, None, None)?, vec![0, 1, 2, 3]);
		Ok(())
	}

	#[test]
	fn test_fetch_item_limit_fewer_than_n_returns_all() -> Result<()> {
		let mut store = item_limit_store(Some(8));
		append_indexed(&mut store, 0..3)?;
		assert_eq!(fetch_indexes(&mut store, None, None)?, vec![0, 1, 2]);
		Ok(())
	}

	#[test]
	fn test_fetch_item_limit_combines_with_count() -> Result<()> {
		let mut store = item_limit_store(Some(5));
		append_indexed(&mut store, 0..20)?;

		// count below the limit wins
		assert_eq!(fetch_indexes(&mut store, Some(2), None)?, vec![0, 1]);
		// count equal to the limit
		assert_eq!(fetch_indexes(&mut store, Some(5), None)?.len(), 5);
		// count above the limit is clamped silently
		assert_eq!(
			fetch_indexes(&mut store, Some(100), None)?,
			vec![0, 1, 2, 3, 4]
		);
		Ok(())
	}

	#[test]
	fn test_fetch_item_limit_byte_limit_wins() -> Result<()> {
		let mut store = item_limit_store(Some(8));
		for i in 0..20 {
			store.append(json!({"index": i, "padding": "x".repeat(50)}))?;
		}
		let item_size = MemoryStore::get_item_size(&json!({"index": 0, "padding": "x".repeat(50)}));

		// Room for exactly 3 items and their 2 separators by bytes, envelope included, fewer
		// than the item limit. The widest envelope leaves the 3 fitting whatever the
		// timestamp, and is far less than a fourth item short.
		let envelope = MemoryStore::widest_envelope_len("test-key");
		let indexes = fetch_indexes(&mut store, None, Some(envelope + item_size * 3 + 2))?;
		assert_eq!(indexes, vec![0, 1, 2]);

		// Byte limit large enough for all 20: the item limit still caps it.
		let indexes = fetch_indexes(&mut store, None, Some(item_size * 20))?;
		assert_eq!(indexes.len(), 8);
		Ok(())
	}

	#[test]
	fn test_fetch_item_limit_walks_store_in_batches() -> Result<()> {
		let mut store = item_limit_store(Some(8));
		append_indexed(&mut store, 0..20)?;

		let mut sizes = Vec::new();
		let mut seen = Vec::new();
		while let Some(result) = store.fetch(None, None)? {
			let batch = result.data.unwrap();
			let items = batch["batch"].as_array().unwrap().clone();
			let removable = result.removable.unwrap();
			assert_eq!(items.len(), removable.len());
			sizes.push(items.len());
			seen.extend(items.iter().map(|v| v["index"].as_i64().unwrap()));
			store.remove(&removable)?;
		}

		assert_eq!(sizes, vec![8, 8, 4]);
		assert_eq!(seen, (0..20).collect::<Vec<_>>());
		assert!(!store.has_data());
		Ok(())
	}

	#[test]
	fn test_fetch_no_item_limit_unchanged() -> Result<()> {
		let mut store = item_limit_store(None);
		append_indexed(&mut store, 0..50)?;
		assert_eq!(
			fetch_indexes(&mut store, None, None)?,
			(0..50).collect::<Vec<_>>()
		);
		assert_eq!(fetch_indexes(&mut store, Some(7), None)?.len(), 7);
		Ok(())
	}

	#[test]
	fn test_fetch_item_limit_of_one() -> Result<()> {
		let mut store = item_limit_store(Some(1));
		append_indexed(&mut store, 0..3)?;
		assert_eq!(fetch_indexes(&mut store, None, None)?, vec![0]);
		assert_eq!(fetch_indexes(&mut store, Some(10), None)?, vec![0]);
		Ok(())
	}

	#[test]
	fn test_fetch_item_limit_with_max_items_drop_oldest() -> Result<()> {
		let mut store = MemoryStore::new(MemoryConfig {
			write_key: "test-key".to_string(),
			max_items: 10,
			max_fetch_size: 1024 * 1024,
			max_fetch_items: Some(4),
		});
		append_indexed(&mut store, 0..15)?;

		// Capacity dropped 0..5; the fetch limit still applies to what remains.
		assert_eq!(store.items.len(), 10);
		assert_eq!(fetch_indexes(&mut store, None, None)?, vec![5, 6, 7, 8]);
		Ok(())
	}

	#[test]
	#[should_panic(
		expected = "max_fetch_items = Some(0)? Fetching nothing, efficiently, forever. Bold."
	)]
	fn test_rejects_zero_max_fetch_items() {
		let _store = item_limit_store(Some(0));
	}
}
