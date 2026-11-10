//! Two stores on one file without the registry, as two processes would be: items can be
//! lost or sent twice, but every item read is a whole, valid item one of them appended.

use super::support::*;
use crate::file::core::FileCore;
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
	for seed in 1..=40u64 {
		let (_dir, path) = temp_path();
		let mut rng = XorShift(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
		let mut cores: [FileCore; 2] = [open_with(&path, 8).0, open_with(&path, 8).0];
		let mut appended = HashSet::new();
		let mut seen = 0usize;
		for step in 0..400 {
			let who = rng.below(2) as usize;
			let core = &mut cores[who];
			match rng.below(3) {
				0 => {
					let len = 6 + rng.below(SLOT as u64 - 5) as usize;
					let text = item(len, step * 2 + who);
					core.append_text(&text).unwrap();
					appended.insert(text);
				}
				1 => {
					let count = 1 + rng.below(4) as usize;
					let items = core.read_items(count, usize::MAX, |_| false).unwrap();
					for (sequence, text) in items {
						seen += 1;
						assert!(
							appended.contains(&text),
							"seed {seed} step {step}: {text:?}"
						);
						serde_json::from_str::<serde_json::Value>(&text).unwrap();
						if rng.below(2) == 0 {
							core.free(sequence).unwrap();
						}
					}
				}
				_ => {
					let live: Vec<u64> = core.index.live().map(|(s, _)| s).collect();
					if let Some(&sequence) = live.get(rng.below(live.len().max(1) as u64) as usize)
					{
						core.free(sequence).unwrap();
					}
				}
			}
		}
		assert!(seen > 0, "seed {seed} read nothing");
		// What either left behind is still only whole, appended items.
		drop(cores);
		for text in texts(&mut open_with(&path, 8).0) {
			assert!(appended.contains(&text), "seed {seed}: {text:?}");
		}
	}
}
