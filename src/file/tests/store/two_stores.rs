//! Two stores on one file without the registry, as two processes would be: items can be
//! lost or sent twice, but every item fetched is a whole, valid item one of them appended.
//!
//! The second store reaches the file through a hard link: another path, so the registry
//! does not see one file twice, and the same file.

use super::*;
use crate::{DataStore, Items};
use std::collections::HashSet;

/// A small deterministic generator, so a failure can be replayed by its seed.
struct XorShift(u64);

impl XorShift {
	fn next(&mut self) -> u64 {
		self.0 ^= self.0 << 13;
		self.0 ^= self.0 >> 7;
		self.0 ^= self.0 << 17;
		self.0
	}

	fn below(&mut self, n: u64) -> u64 {
		self.next() % n
	}
}

#[test]
fn test_interleaved_appends_fetches_and_removes_only_yield_whole_appended_items() {
	for seed in 1..=30u64 {
		let (dir, path) = temp_path();
		let link = dir.path().join("link.tdb");
		let mut stores = vec![store(&path, 8)];
		std::fs::hard_link(&path, &link).unwrap();
		stores.push(store(&link, 8));
		assert!(
			stores[1].core().is_some(),
			"the link is not the registry's path"
		);
		let mut rng = XorShift(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
		let mut appended = HashSet::new();
		let mut held: Vec<(usize, Items)> = Vec::new();
		let mut fetched = 0usize;
		for step in 0..400usize {
			let who = rng.below(2) as usize;
			match rng.below(4) {
				0 | 1 => {
					let item = serde_json::json!({ "s": step, "w": who });
					stores[who].append(item.clone()).unwrap();
					appended.insert(item);
				}
				2 => {
					let count = 1 + rng.below(4) as usize;
					if let Some(items) = stores[who].fetch(count, usize::MAX).unwrap() {
						for value in items.values() {
							fetched += 1;
							assert!(
								appended.contains(&value),
								"seed {seed} step {step}: {value}"
							);
						}
						held.push((who, items));
					}
				}
				_ => {
					if !held.is_empty() {
						let (owner, items) =
							held.swap_remove(rng.below(held.len() as u64) as usize);
						// Removed by its own store, by the other one, or released.
						match rng.below(3) {
							0 => stores[owner].remove(items).unwrap(),
							1 => stores[1 - owner].remove(items).unwrap(),
							_ => items.release(),
						}
					}
				}
			}
		}
		assert!(fetched > 0, "seed {seed} fetched nothing");
		drop(held);
		drop(stores);
		// What they left behind is still only whole, appended items.
		for value in reopened(&path) {
			assert!(appended.contains(&value), "seed {seed}: {value}");
		}
	}
}
