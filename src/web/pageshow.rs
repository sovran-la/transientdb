//! Starting over as a new tab when the page is restored from the back/forward cache.
//!
//! Browsers differ in what happens to a page's Web Locks while it sits in the
//! back/forward cache. Chrome keeps them (and evicts the page if another page requests
//! one), Firefox does not cache a page that holds one, and Safari releases them when it
//! caches the page: another tab may adopt the page's records while it is cached, and once
//! restored the page holds no lock, though nothing tells it so. A store therefore starts
//! over as a new tab when its page is restored, through the same steps as `new`:
//!
//! 1. It releases its old tab's lock, if it still holds it.
//! 2. It takes a new tab id and requests that tab's lock. Until the lock is granted,
//!    nothing is written under the new tab: appends are kept in memory, and written once
//!    it is.
//! 3. It resets its queue in memory, not in IndexedDB, so an `Items` handed out before
//!    matches nothing.
//! 4. Once the lock is granted, it runs an adoption pass. That adopts the old tab's
//!    records like any orphan's, unless another tab adopted them while the page was
//!    cached, in which case they are that tab's to send.
//!
//! An adoption pass in flight when the page was cached adopts nothing into the new queue
//! when it resumes; see [`Shared::generation`] and [`adopt`](super::adopt).

use super::store::warn_memory_only;
use super::{adopt, tab, warn, Shared};
use std::rc::{Rc, Weak};
use wasm_bindgen::prelude::*;
use web_sys::{Event, PageTransitionEvent};

const PAGESHOW: &str = "pageshow";

/// A `pageshow` listener on the window that starts the store over as a new tab when the
/// page is restored from the back/forward cache. Dropping it removes it.
pub(super) struct Listener {
	callback: Closure<dyn FnMut(Event)>,
}

impl Listener {
	/// Listens for as long as the store behind `shared` exists. `None`, after logging, if
	/// there is no window to listen on.
	pub(super) fn start(shared: Weak<Shared>) -> Option<Self> {
		let callback = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
			#[cfg(test)]
			PAGESHOWS.set(PAGESHOWS.get() + 1);
			let restored = event.dyn_ref::<PageTransitionEvent>();
			if !restored.is_some_and(PageTransitionEvent::persisted) {
				return;
			}
			if let Some(shared) = shared.upgrade() {
				start_over(&shared);
			}
		});
		let added = web_sys::window().and_then(|window| {
			let listener = callback.as_ref().unchecked_ref();
			window
				.add_event_listener_with_callback(PAGESHOW, listener)
				.ok()
		});
		if added.is_none() {
			warn(
				"Could not listen for pageshow: a page restored from the back/forward cache \
				 keeps its tab id",
			);
			return None;
		}
		Some(Self { callback })
	}
}

impl Drop for Listener {
	fn drop(&mut self) {
		if let Some(window) = web_sys::window() {
			let listener = self.callback.as_ref().unchecked_ref();
			let _ = window.remove_event_listener_with_callback(PAGESHOW, listener);
		}
	}
}

/// Starts the store over as a new tab, as described in this module's documentation.
///
/// A store whose connection has closed writes nothing more and holds its items in memory
/// only, so it carries on as it is: starting over would lose them.
pub(super) fn start_over(shared: &Rc<Shared>) {
	if shared.live_db().is_none() {
		return;
	}
	shared.generation.set(shared.generation.get() + 1);
	drop(shared.liveness.take());
	let tab = tab::new_id();
	*shared.tab.borrow_mut() = tab.clone();
	shared.queue.borrow_mut().reset();

	let shared = shared.clone();
	wasm_bindgen_futures::spawn_local(async move {
		match shared.hold_liveness(&tab).await {
			Ok(true) => {}
			// Dropped, or started over again meanwhile: nothing more to do for this tab.
			Ok(false) => return,
			Err(why) => {
				if !shared.dropped.get() && *shared.tab.borrow() == tab {
					if let Some(db) = shared.live_db() {
						db.close();
					}
					shared.closed.set(true);
					warn_memory_only(&why);
				}
				return;
			}
		}
		// What was appended while the lock was pending, written under the tab now that
		// it is held.
		let queued: Vec<(u64, String)> = (shared.queue.borrow().items.iter())
			.map(|item| (item.seq, item.text.clone()))
			.collect();
		for (seq, text) in queued {
			shared.write_to_idb(seq, &text);
		}
		adopt::pass(&shared).await;
	});
}

#[cfg(test)]
thread_local! {
	/// How many `pageshow` events a store's listener has seen, for tests.
	pub(super) static PAGESHOWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
