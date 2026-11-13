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
//!
//! A reset, or the store starting over as a new tab (see [`pageshow`](super::pageshow)),
//! ends the pass: what it would adopt into is gone. If the store started over, the pass
//! begins again, as the new tab's, once that tab's lock is held; the pass the new tab
//! starts itself does nothing while this one runs.

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
/// stops early once the store is dropped, its connection closed, or it is reset.
pub(super) async fn pass(shared: &Rc<Shared>) {
	if shared.passing.replace(true) {
		return;
	}
	let _passing = Passing(shared);
	#[cfg(test)]
	shared.passes.set(shared.passes.get() + 1);
	loop {
		let tab = shared.tab.borrow().clone();
		pass_once(shared, &tab).await;
		// Started over as a new tab meanwhile: adopt for that one. Unless its lock is
		// not held yet, in which case it adopts once it is.
		if *shared.tab.borrow() == tab || !shared.active() {
			return;
		}
	}
}

/// One pass for the tab `own`: stops early once the store is no longer
/// [`active`](Shared::active), or reset or started over since the pass began.
async fn pass_once(shared: &Rc<Shared>, own: &str) {
	let generation = shared.generation.get();
	let current = || shared.active() && shared.generation.get() == generation;
	let Some(db) = shared.live_db().cloned().filter(|_| current()) else {
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
		if !current() {
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
		if id == own {
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
		if !current() {
			return;
		}
		if let Err(e) = take_over(shared, &db, &tab, true).await {
			warn(&format!("Adopting the items of tab {id} failed: {e}"));
		}
		// Released only now that the move has committed, or aborted.
		drop(held);
	}
	if current() {
		trim(shared);
	}
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
