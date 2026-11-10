//! The in-memory queue behind `MemoryStore` and `WebStore`: items as JSON text in
//! sequence order, their claims, and the drop-oldest rule.

use crate::claims::Ledger;
use crate::{Error, Items, Result};
use std::collections::VecDeque;
use std::io;

/// One stored item: its sequence number, unique within its store, and its JSON text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stored {
	pub(crate) seq: u64,
	pub(crate) text: String,
}

/// Items in sequence order, at most `max_items` of them, and which are pending.
pub(crate) struct Queue {
	pub(crate) items: VecDeque<Stored>,
	pub(crate) ledger: Ledger,
	/// The sequence number the next item gets. Sequence numbers start at 1 and are never
	/// reused by this queue, a reset included.
	pub(crate) next_seq: u64,
	/// The last sequence number this queue may give out.
	max_seq: u64,
	max_items: usize,
}

impl Queue {
	/// An empty queue holding at most `max_items` items, giving out sequence numbers up
	/// to `max_seq`. `max_seq` must be below `u64::MAX`, so the number after the last
	/// one given out can be represented.
	pub(crate) fn new(max_items: usize, max_seq: u64) -> Self {
		assert!(max_items > 0 && max_seq < u64::MAX);
		Self {
			items: VecDeque::new(),
			ledger: Ledger::new(),
			next_seq: 1,
			max_seq,
			max_items,
		}
	}

	/// Appends `text` under the next sequence number and, if that takes the queue past
	/// `max_items`, drops the oldest item, available or pending. Returns the new item's
	/// sequence number and the dropped one's.
	///
	/// # Errors
	/// An `Io` error, changing nothing, once sequence numbers are exhausted: they never
	/// wrap, so no two items can share one.
	pub(crate) fn push(&mut self, text: String) -> Result<(u64, Option<u64>)> {
		let seq = self.next_seq;
		if seq > self.max_seq {
			return Err(Error::Io(io::Error::other("sequence numbers exhausted")));
		}
		self.next_seq = seq + 1;
		self.items.push_back(Stored { seq, text });
		Ok((seq, self.drop_oldest_beyond(self.max_items).pop()))
	}

	/// Adds an item read back from storage. Items must be restored in sequence order;
	/// the next sequence number given out is above every restored one.
	#[cfg_attr(not(all(feature = "web", target_arch = "wasm32")), allow(dead_code))]
	pub(crate) fn restore(&mut self, seq: u64, text: String) {
		debug_assert!(self.items.back().map(|last| last.seq) < Some(seq));
		self.items.push_back(Stored { seq, text });
		self.skip_past(seq);
	}

	/// Makes sure no sequence number up to `seq` is given out.
	#[cfg_attr(not(all(feature = "web", target_arch = "wasm32")), allow(dead_code))]
	pub(crate) fn skip_past(&mut self, seq: u64) {
		self.next_seq = self.next_seq.max(seq.saturating_add(1));
	}

	/// Drops the oldest items until at most `max_items` remain; returns their sequence
	/// numbers, oldest first.
	pub(crate) fn drop_oldest_beyond(&mut self, max_items: usize) -> Vec<u64> {
		let excess = self.items.len().saturating_sub(max_items);
		let dropped: Vec<u64> = self.items.drain(..excess).map(|item| item.seq).collect();
		if !dropped.is_empty() {
			let mut claims = self.ledger.lock();
			for &seq in &dropped {
				claims.forget(seq);
			}
		}
		dropped
	}

	/// The oldest available items: at most `count`, totalling at most `max_bytes`.
	pub(crate) fn fetch(&self, count: usize, max_bytes: usize) -> Option<Items> {
		let candidates = self.items.iter().map(|item| (item.seq, item.text.as_str()));
		self.ledger.issue(candidates, count, max_bytes)
	}

	/// Deletes what `items` holds, if this queue issued it since its last reset, and
	/// returns the sequence numbers deleted, oldest first.
	pub(crate) fn remove(&mut self, items: Items) -> Vec<u64> {
		let gone = self.ledger.settle(items);
		if !gone.is_empty() {
			self.items
				.retain(|item| gone.binary_search(&item.seq).is_err());
		}
		gone
	}

	/// Deletes every item and every claim.
	pub(crate) fn reset(&mut self) {
		self.items.clear();
		self.ledger.lock().reset();
	}

	/// Whether a fetch could return something.
	pub(crate) fn has_data(&self) -> bool {
		self.ledger
			.any_available(self.items.iter().map(|item| item.seq))
	}
}

#[cfg(test)]
mod tests;
