use crate::queue::Queue;
use crate::{serialize, DataStore, Items, Result};
use serde_json::Value;

/// Configuration for a [`MemoryStore`].
///
/// # Examples
/// ```
/// use transientdb::{MemoryConfig, MemoryStore};
///
/// let store = MemoryStore::new(MemoryConfig {
///     max_item_size: 8 * 1024, // no item over 8 KiB serialized
///     max_items: 1000,         // the 1001st append drops the oldest item
/// });
/// ```
#[derive(Clone, Debug)]
pub struct MemoryConfig {
	/// The largest item the store accepts: its compact `serde_json` text, in bytes.
	/// `append` rejects a larger item with [`Error::ItemTooLarge`](crate::Error). Must not
	/// be 0.
	pub max_item_size: usize,
	/// The most items the store holds at once. Appending past it drops the oldest item,
	/// fetched or not. Must not be 0.
	pub max_items: usize,
}

/// A queue of items held in memory.
///
/// Each item is kept as its compact JSON text, in the order it was appended, until it
/// is passed to `remove` inside an [`Items`], dropped to make room, or reset. Identical
/// items are distinct entries: `remove` deletes exactly the items a fetch handed out,
/// never an equal one still waiting.
///
/// Nothing survives the store: dropping it discards every item. An `Items` outliving
/// its store is harmless; releasing it does nothing.
pub struct MemoryStore {
	config: MemoryConfig,
	queue: Queue,
}

impl MemoryStore {
	/// Creates an empty MemoryStore.
	///
	/// # Panics
	/// * If `max_item_size` is 0
	/// * If `max_items` is 0
	pub fn new(config: MemoryConfig) -> Self {
		if config.max_item_size == 0 {
			panic!("max_item_size = 0? Even `0` takes a byte. There is no item that small.");
		}
		if config.max_items == 0 {
			panic!("max_items = 0? So... you want a store that stores nothing? That's what /dev/null is for.");
		}
		Self {
			// Sequence numbers never wrap; the last one keeps one value spare. Running out
			// takes 2^64 - 2 appends, which cannot happen in practice; if it ever did,
			// `append` would fail with `Io` rather than give two items one number.
			queue: Queue::new(config.max_items, u64::MAX - 1),
			config,
		}
	}
}

impl DataStore for MemoryStore {
	/// Appends an item, dropping the oldest one if the store holds `max_items` already.
	///
	/// # Errors
	/// [`Error::ItemTooDeep`](crate::Error) or [`Error::ItemTooLarge`](crate::Error),
	/// changing nothing, as described on [`DataStore::append`]. No other error: each item
	/// gets the next `u64` sequence number, and at one append per nanosecond those last
	/// more than 580 years, so they cannot run out in practice.
	fn append(&mut self, data: Value) -> Result<()> {
		let text = serialize::item_text(data, self.config.max_item_size)?;
		self.queue.push(text)?;
		Ok(())
	}

	fn fetch(&mut self, count: usize, max_bytes: usize) -> Result<Option<Items>> {
		Ok(self.queue.fetch(count, max_bytes))
	}

	fn remove(&mut self, items: Items) -> Result<()> {
		self.queue.remove(items);
		Ok(())
	}

	fn reset(&mut self) {
		self.queue.reset();
	}

	fn has_data(&self) -> bool {
		self.queue.has_data()
	}
}

#[cfg(test)]
mod tests;
