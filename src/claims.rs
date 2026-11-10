//! The claim ledger: which of a store instance's items are pending, that is, handed out
//! by a `fetch` whose [`Items`] has been neither removed nor released.
//!
//! Each store instance owns one [`Ledger`] and every `Items` it issues holds a clone of
//! it, so an `Items` can give its items back on drop without a reference to the store,
//! and therefore without the store's lock or the `TransientDB` mutex around it. Dropping
//! an `Items` locks only the ledger, briefly; after the store is gone, the `Items` keeps
//! the ledger alive and the release changes nothing anyone can see.
//!
//! Rules that keep this deadlock-free, for every store that uses it:
//!
//! - A store locks the ledger only briefly and calls nothing while holding it.
//! - A store never drops an `Items`, or anything else that locks a ledger, while holding
//!   the ledger lock. [`Ledger::settle`] disarms the `Items` it is given (or, for another
//!   store's, lets it release itself) before it locks.
//!
//! [`reset`](Claims::reset) starts a new epoch. Every `Items` remembers the epoch it was
//! issued in and is ignored once that epoch is over, so one fetched before a reset
//! matches nothing afterwards, even if sequence numbers were ever given out again.

use crate::Items;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// One store instance's claims. Cloning shares them; two ledgers are the same instance's
/// when [`same`](Self::same) says so.
#[derive(Clone, Debug, Default)]
pub(crate) struct Ledger(Arc<Mutex<Claims>>);

/// The pending sequence numbers of one store instance, and the current epoch.
#[derive(Debug, Default)]
pub(crate) struct Claims {
	epoch: u64,
	pending: HashSet<u64>,
}

impl Ledger {
	pub(crate) fn new() -> Self {
		Self::default()
	}

	/// The claims, locked. A panic while the lock was held cannot leave them half
	/// changed (no method below panics midway), so a poisoned lock is used as it is:
	/// an `Items` dropped during a panic must not panic again.
	pub(crate) fn lock(&self) -> MutexGuard<'_, Claims> {
		self.0.lock().unwrap_or_else(PoisonError::into_inner)
	}

	/// Whether `other` is this very ledger, not merely an equal one.
	pub(crate) fn same(&self, other: &Ledger) -> bool {
		Arc::ptr_eq(&self.0, &other.0)
	}

	/// Hands out the first available items of `candidates`, which must be in sequence
	/// order: at most `count` of them, whose text totals at most `max_bytes`, stopping at
	/// the first available item that does not fit. Marks them pending.
	///
	/// `None` if nothing is taken: `count` is 0, nothing is available, or the oldest
	/// available item alone is larger than `max_bytes`.
	pub(crate) fn issue<'a>(
		&self,
		candidates: impl IntoIterator<Item = (u64, &'a str)>,
		count: usize,
		max_bytes: usize,
	) -> Option<Items> {
		if count == 0 {
			return None;
		}
		let mut taken = Vec::new();
		let mut bytes = 0;
		let epoch = {
			let mut claims = self.lock();
			for (seq, text) in candidates {
				if claims.is_pending(seq) {
					continue;
				}
				if taken.len() == count || bytes + text.len() > max_bytes {
					break;
				}
				bytes += text.len();
				taken.push((seq, text.to_owned()));
			}
			for (seq, _) in &taken {
				claims.pending.insert(*seq);
			}
			claims.epoch
		};
		// Built after the lock is released, so nothing that could drop an `Items` runs
		// while it is held.
		(!taken.is_empty()).then(|| Items::new(self.clone(), epoch, taken))
	}

	/// Takes back an `Items` passed to `remove`: un-marks its items and returns the
	/// sequence numbers of those still pending under this ledger in the current epoch,
	/// in order. Those are the items to delete; the rest (dropped since the fetch, or
	/// fetched before a reset) match nothing.
	///
	/// Another store's `Items` is not this ledger's to settle: it is dropped, which
	/// releases it to its own store, before this ledger is locked. Returns nothing.
	pub(crate) fn settle(&self, items: Items) -> Vec<u64> {
		let Some((epoch, seqs)) = items.surrender(self) else {
			return Vec::new();
		};
		let mut claims = self.lock();
		if claims.epoch != epoch {
			return Vec::new();
		}
		seqs.into_iter()
			.filter(|seq| claims.pending.remove(seq))
			.collect()
	}

	/// Whether any of `seqs` is available, that is, not pending.
	pub(crate) fn any_available(&self, seqs: impl IntoIterator<Item = u64>) -> bool {
		let claims = self.lock();
		seqs.into_iter().any(|seq| !claims.is_pending(seq))
	}
}

impl Claims {
	pub(crate) fn is_pending(&self, seq: u64) -> bool {
		self.pending.contains(&seq)
	}

	/// The item `seq` is gone from the store (dropped to make room): it is no longer
	/// pending, and the `Items` holding it will match nothing for it.
	pub(crate) fn forget(&mut self, seq: u64) {
		self.pending.remove(&seq);
	}

	/// Makes `seqs` available again, if they were issued in the current epoch.
	pub(crate) fn release(&mut self, epoch: u64, seqs: impl IntoIterator<Item = u64>) {
		if epoch != self.epoch {
			return;
		}
		for seq in seqs {
			self.pending.remove(&seq);
		}
	}

	/// Nothing is pending, and every `Items` issued so far matches nothing from now on.
	pub(crate) fn reset(&mut self) {
		self.pending.clear();
		self.epoch = self.epoch.wrapping_add(1);
	}

	#[cfg(test)]
	pub(crate) fn pending_count(&self) -> usize {
		self.pending.len()
	}

	#[cfg(test)]
	pub(crate) fn epoch(&self) -> u64 {
		self.epoch
	}

	#[cfg(test)]
	pub(crate) fn claim(&mut self, seq: u64) {
		self.pending.insert(seq);
	}
}

#[cfg(test)]
mod tests;
