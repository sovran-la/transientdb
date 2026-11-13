//! Adoption passes: taking over the records of tabs that no longer exist.
//!
//! A pass lists the tabs that have records, and for each one other than this store's,
//! asks for its liveness lock without waiting. Not granted: the tab is alive, or another
//! tab is adopting it right now, so it is skipped. Granted: while holding the lock, its
//! records are moved under this store's tab in one transaction ([`take_over`]), then the
//! lock is released. Two tabs racing for the same orphan cannot both be granted its lock,
//! and the lock is released only once the move has committed, so the loser finds nothing
//! left. Once every tab has been looked at, the queue is trimmed to `max_items`.
//!
//! A pass never holds a borrow of the queue across an `await`, so the store's sync calls
//! run normally between its steps, and it never runs alongside another pass.

use super::takeover::{distinct_tabs, take_over};
use super::{lock, warn, Shared};
use std::rc::Rc;

/// Clears [`Shared::passing`] when the pass ends, however it ends.
struct Passing<'a>(&'a Shared);

impl Drop for Passing<'_> {
	fn drop(&mut self) {
		self.0.passing.set(false);
	}
}

/// Runs one adoption pass, unless one is running already. Logs what goes wrong, and
/// stops early once the store is dropped or its connection closed.
pub(super) async fn pass(shared: &Rc<Shared>) {
	if shared.passing.replace(true) {
		return;
	}
	let _passing = Passing(shared);
	#[cfg(test)]
	shared.passes.set(shared.passes.get() + 1);
	let Some(db) = shared.live_db().cloned().filter(|_| shared.active()) else {
		return;
	};
	let tabs = match distinct_tabs(&db).await {
		Ok(tabs) => tabs,
		Err(e) => {
			warn(&format!("Looking for orphaned items failed: {e}"));
			return;
		}
	};
	for tab in tabs {
		if !shared.active() {
			return;
		}
		let Some(id) = tab.as_string() else {
			// No store keeps records under anything but a string: not items, and no
			// lock guards them. Deleting them twice is harmless.
			if let Err(e) = take_over(shared, &db, &tab, false).await {
				warn(&format!(
					"Discarding records that are not items failed: {e}"
				));
			}
			continue;
		};
		if id == shared.tab {
			continue;
		}
		let name = lock::name(&shared.config.database_name, &id);
		let held = match lock::acquire(&name, true).await {
			Ok(Some(held)) => held,
			Ok(None) => continue,
			Err(e) => {
				warn(&format!("Requesting the lock of tab {id} failed: {e}"));
				continue;
			}
		};
		if !shared.active() {
			return;
		}
		if let Err(e) = take_over(shared, &db, &tab, true).await {
			warn(&format!("Adopting the items of tab {id} failed: {e}"));
		}
		// Released only now that the move has committed, or aborted.
		drop(held);
	}
	trim(shared);
}

/// Drops the oldest items until the store holds at most `max_items`, deleting them from
/// IndexedDB, and logs how many.
fn trim(shared: &Shared) {
	let max = shared.config.max_items;
	let dropped = shared.queue.borrow_mut().drop_oldest_beyond(max);
	if dropped.is_empty() {
		return;
	}
	warn(&format!(
		"Dropping the {} oldest items: adopting orphaned items took the store past \
		 max_items = {max}",
		dropped.len()
	));
	shared.delete_from_idb(&dropped);
}
