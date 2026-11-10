//! Failures to inject into a store's file, for tests.
//!
//! Under `cfg(test)` a [`Hook`] is a `Faults` shared, through an `Arc<Mutex<_>>`, by a
//! test and the store it opens. Every read and write the store makes, and every file it
//! creates or deletes, first asks it whether to fail. A write budget cuts a write short
//! after a given number of bytes, the way a kill would, and from then on everything
//! fails: the process is gone. Outside tests a `Hook` is empty and never fails.

use std::io;

/// One kind of thing the store does to its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Op {
	/// Writing the header of a new file.
	Header,
	/// Writing an item's payload into its slot.
	Payload,
	/// Writing a slot's entry: the commit point of an append.
	Entry,
	/// Writing a slot's state byte: freeing it.
	State,
	/// Cutting the file back to its header, on shrinking or reset.
	Cut,
	/// Reading the file's length.
	Length,
	/// Reading the header.
	ReadHeader,
	/// Reading one slot's entry at open.
	ReadEntry,
	/// Reading one item, entry and payload, at fetch.
	ReadItem,
	/// Opening the existing file.
	Open,
	/// Creating the file.
	Create,
	/// Deleting a file that does not match.
	Delete,
}

#[cfg(test)]
impl Op {
	/// Every kind, for tests that try each one.
	pub(crate) const ALL: [Op; 12] = [
		Op::Header,
		Op::Payload,
		Op::Entry,
		Op::State,
		Op::Cut,
		Op::Length,
		Op::ReadHeader,
		Op::ReadEntry,
		Op::ReadItem,
		Op::Open,
		Op::Create,
		Op::Delete,
	];
}

/// Nothing to inject outside tests.
#[cfg(not(test))]
#[derive(Debug, Clone, Default)]
pub(crate) struct Hook;

#[cfg(not(test))]
pub(crate) fn check(_hook: &Hook, _op: Op) -> io::Result<()> {
	Ok(())
}

/// Outside tests a write is never cut: all `len` bytes may go to the file.
#[cfg(not(test))]
pub(crate) fn spend(_hook: &Hook, len: usize) -> Result<usize, usize> {
	Ok(len)
}

#[cfg(not(test))]
pub(crate) fn record(_hook: &Hook, _op: Op) {}

/// The error a write cut short by a kill returns, to a store that will never see it.
pub(crate) fn killed() -> io::Error {
	io::Error::other("the process was killed during this write")
}

#[cfg(test)]
pub(crate) use injection::*;

#[cfg(test)]
mod injection {
	use super::Op;
	use std::collections::HashMap;
	use std::io;
	use std::sync::{Arc, Mutex};

	#[derive(Debug, Default)]
	pub(crate) struct Faults {
		/// For each kind that is to fail, how many more times it succeeds first. A kind fails
		/// every time once its count reaches 0, until it is taken out.
		pub(crate) fail: HashMap<Op, usize>,
		/// Bytes that may still reach the file; `None` never cuts a write. The write that
		/// would pass it writes only what is left, and then the store is killed.
		pub(crate) write_budget: Option<usize>,
		/// Whether a write was cut by the budget. From then on, everything fails.
		pub(crate) killed: bool,
		/// Every write, cut and creation or deletion that reached the file, in order.
		pub(crate) done: Vec<Op>,
	}

	/// The faults a test shares with the store it opens.
	pub(crate) type Hook = Arc<Mutex<Faults>>;

	/// Fails if the test has made `op` fail.
	pub(crate) fn check(hook: &Hook, op: Op) -> io::Result<()> {
		hook.lock().unwrap().check(op)
	}

	/// How many of `len` bytes a write may put in the file: see [`Faults::spend`].
	pub(crate) fn spend(hook: &Hook, len: usize) -> Result<usize, usize> {
		hook.lock().unwrap().spend(len)
	}

	/// Notes that `op` reached the file.
	pub(crate) fn record(hook: &Hook, op: Op) {
		hook.lock().unwrap().done.push(op);
	}

	pub(crate) fn injected(op: Op) -> io::Error {
		io::Error::other(format!("injected {op:?} failure"))
	}

	impl Faults {
		pub(crate) fn shared() -> Hook {
			Arc::default()
		}

		/// Makes every `op` from now on fail.
		pub(crate) fn fail(&mut self, op: Op) {
			self.fail.insert(op, 0);
		}

		/// Lets `op` succeed `times` more times, then makes it fail.
		pub(crate) fn fail_after(&mut self, op: Op, times: usize) {
			self.fail.insert(op, times);
		}

		/// Whether `op` may go ahead. Counts it down if it is set to fail later.
		pub(crate) fn check(&mut self, op: Op) -> io::Result<()> {
			if self.killed {
				return Err(super::killed());
			}
			match self.fail.get_mut(&op) {
				Some(0) => Err(injected(op)),
				Some(left) => {
					*left -= 1;
					Ok(())
				}
				None => Ok(()),
			}
		}

		/// How many of `len` bytes a write may put in the file before the store is killed:
		/// `Ok(len)` for all of them, `Err(n)` for only the first `n`. Spends the budget.
		pub(crate) fn spend(&mut self, len: usize) -> Result<usize, usize> {
			let Some(budget) = self.write_budget else {
				return Ok(len);
			};
			if len <= budget {
				self.write_budget = Some(budget - len);
				return Ok(len);
			}
			self.write_budget = Some(0);
			self.killed = true;
			Err(budget)
		}
	}
}
