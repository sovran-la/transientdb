//! Running an adoption pass every `orphan_interval`, for as long as the store lives.

use super::{adopt, Shared};
use std::rc::Weak;
use std::time::Duration;
use wasm_bindgen::prelude::*;

/// A browser interval timer that starts an adoption pass on each tick. Dropping it stops
/// the timer.
pub(super) struct Interval {
	handle: i32,
	_tick: Closure<dyn FnMut()>,
}

impl Interval {
	/// Starts a pass every `every`, as long as the store behind `shared` exists. A tick
	/// while a pass is still running starts nothing. `None`, after logging, if there is
	/// no browser timer.
	pub(super) fn start(shared: Weak<Shared>, every: Duration) -> Option<Self> {
		let tick = Closure::<dyn FnMut()>::new(move || {
			if let Some(shared) = shared.upgrade() {
				wasm_bindgen_futures::spawn_local(async move { adopt::pass(&shared).await });
			}
		});
		let window = web_sys::window();
		let handle = window.and_then(|window| {
			let tick = tick.as_ref().unchecked_ref();
			let ms = millis(every);
			window
				.set_interval_with_callback_and_timeout_and_arguments_0(tick, ms)
				.ok()
		});
		let Some(handle) = handle else {
			super::warn("No browser timer: orphaned items are adopted only when a store opens");
			return None;
		};
		Some(Self {
			handle,
			_tick: tick,
		})
	}
}

impl Drop for Interval {
	fn drop(&mut self) {
		if let Some(window) = web_sys::window() {
			window.clear_interval_with_handle(self.handle);
		}
	}
}

/// `every` as a timer delay: whole milliseconds, at least 1, at most `i32::MAX` (about
/// 24.8 days, the most a browser timer takes).
pub(super) fn millis(every: Duration) -> i32 {
	every.as_millis().clamp(1, i32::MAX as u128) as i32
}
