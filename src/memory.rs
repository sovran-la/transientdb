use crate::{DataResult, DataStore, Equivalent};
use serde_json::json;
use serde_json::Value;
use std::any::Any;
use std::collections::VecDeque;
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

/// Configuration options for the in-memory data store.
///
/// This struct provides the configuration parameters needed to create a new MemoryStore instance.
/// It controls the store's identification, capacity, and fetch size limits.
///
/// # Examples
/// ```
/// use transientdb::{MemoryConfig, MemoryStore};
///
/// let config = MemoryConfig {
///     write_key: "my-store".into(),
///     max_items: 1000,
///     max_fetch_size: 1024 * 1024, // 1MB
/// };
///
/// let store = MemoryStore::new(config);
/// ```
#[derive(Clone)]
pub struct MemoryConfig {
	/// Key used to identify writes to this store.
	/// This is included in the metadata of each batch of data fetched from the store.
	pub write_key: String,
	/// Maximum number of items to store before old items are removed.
	/// Once this limit is reached, adding new items will remove the oldest items to make space.
	pub max_items: usize,
	/// Maximum size in bytes that can be fetched in a single operation.
	/// This prevents memory spikes during fetch operations by limiting the amount of data returned.
	pub max_fetch_size: usize,
}

/// A FIFO data store that keeps items in memory.
///
/// Items are stored until fetched or until max_items is reached, at which point
/// older items are removed to make space for new ones. The store maintains data in a
/// queue structure and supports concurrent access.
///
/// When items are fetched, they are packaged into a JSON batch with metadata including:
/// - A batch array containing the items
/// - A timestamp indicating when the batch was created
/// - The store's write key
pub struct MemoryStore {
	config: MemoryConfig,
	items: VecDeque<Value>,
}

impl MemoryStore {
	/// Creates a new MemoryStore with the specified configuration.
	///
	/// # Panics
	/// * If max_fetch_size is less than 100 bytes
	/// * If max_items is 0
	pub fn new(config: MemoryConfig) -> Self {
		if config.max_fetch_size < 100 {
			panic!("max_fetch_size < 100 bytes? What are you even trying to fetch, empty arrays?");
		}
		if config.max_items == 0 {
			panic!("max_items = 0? So... you want a store that stores nothing? That's what /dev/null is for.");
		}

		Self {
			config,
			items: VecDeque::new(),
		}
	}

	/// Creates a JSON batch object containing the provided items and metadata.
	///
	/// # Arguments
	/// * `items` - Slice of JSON values to include in the batch
	///
	/// # Returns
	/// A JSON value containing:
	/// - A `batch` array of the provided items
	/// - A `sentAt` timestamp in RFC3339 format
	/// - The store's `writeKey`
	fn create_batch(&self, items: &[Value]) -> Value {
		json!({
			"batch": items,
			"sentAt": chrono::Utc::now().to_rfc3339(),
			"writeKey": self.config.write_key
		})
	}

	fn get_item_size(item: &Value) -> usize {
		item.to_string().len()
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

	fn append(&mut self, data: Value) -> Result<()> {
		self.items.push_back(data);

		while self.items.len() > self.config.max_items {
			self.items.pop_front();
		}

		Ok(())
	}

	fn fetch(
		&mut self,
		count: Option<usize>,
		max_bytes: Option<usize>,
	) -> Result<Option<DataResult<Self::Output>>> {
		let max_bytes = max_bytes.unwrap_or(self.config.max_fetch_size);
		let mut accumulated_size = 0;
		let mut num_items = 0;

		// Just look at items without draining
		for item in self.items.iter() {
			let item_size = Self::get_item_size(item);
			if accumulated_size + item_size > max_bytes {
				break;
			}
			if let Some(count) = count {
				if num_items >= count {
					break;
				}
			}
			accumulated_size += item_size;
			num_items += 1;
		}

		if num_items == 0 {
			return Ok(None);
		}

		// Create vectors of items and removable references
		let items: Vec<Value> = self.items.iter().take(num_items).cloned().collect();

		let removable: Vec<Box<dyn Equivalent>> = items
			.iter()
			.map(|item| Box::new(item.clone()) as Box<dyn Equivalent>)
			.collect();

		let batch = self.create_batch(&items);

		Ok(Some(DataResult {
			data: Some(batch),
			removable: Some(removable),
		}))
	}

	fn remove(&mut self, data: &[Box<dyn Equivalent>]) -> Result<()> {
		// Remove items that match the provided equivalents
		self.items
			.retain(|item| !data.iter().any(|removable| removable.equals(item)));
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use crate::memory::{MemoryConfig, MemoryStore};
	use crate::DataStore;
	use serde_json::{json, Value};
	use std::io::Result;

	#[test]
	fn test_basic_operations() -> Result<()> {
		let config = MemoryConfig {
			write_key: "test-key".to_string(),
			max_items: 1000,
			max_fetch_size: 1024,
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
		};

		let mut store = MemoryStore::new(config);

		// Small item ~30-40 bytes
		store.append(json!({"type": "small", "value": "tiny"}))?;

		// Large item ~500 bytes
		store.append(json!({
			"type": "large",
			"value": "x".repeat(400),  // Make it obviously too big
		}))?;

		// First fetch should only get the small item
		if let Some(result) = store.fetch(None, None)? {
			let batch: Value = result.data.unwrap();
			let items = batch["batch"].as_array().unwrap();
			assert_eq!(items.len(), 1, "Should only fetch the small item");
			assert_eq!(items[0]["type"], "small");

			// Remove the fetched item
			if let Some(removable) = result.removable {
				store.remove(&removable)?;
			}
		}

		// Second fetch should get the large item
		if let Some(result) = store.fetch(None, None)? {
			let batch: Value = result.data.unwrap();
			let items = batch["batch"].as_array().unwrap();
			assert_eq!(items.len(), 1, "Should fetch the large item");
			assert_eq!(items[0]["type"], "large");

			// Remove the fetched item
			if let Some(removable) = result.removable {
				store.remove(&removable)?;
			}
		}

		Ok(())
	}

	#[test]
	fn test_memory_store_json_types() -> Result<()> {
		let config = MemoryConfig {
			write_key: "test-key".to_string(),
			max_items: 100,
			max_fetch_size: 1024,
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
		};

		let _store = MemoryStore::new(config);
	}
}
