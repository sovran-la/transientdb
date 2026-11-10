//! `end_batch` together with the other limits on a batch: no batch crosses a boundary
//! or exceeds a limit, and a long span is split without crossing into the next.

use super::end_batch::*;
use super::support::*;
use super::*;
use crate::batch::script::{self, Step};

#[test]
fn test_boundary_with_the_item_limit() -> Result<()> {
	let mut store = store_with_limits(None, Some(2));
	append_tags(&mut store, 0..5)?;
	store.end_batch()?;
	append_tags(&mut store, 5..8)?;
	store.end_batch()?;
	append_tags(&mut store, 8..9)?;
	let expected = vec![
		vec![0, 1],
		vec![2, 3],
		vec![4],
		vec![5, 6],
		vec![7],
		vec![8],
	];
	assert_eq!(drained(&mut store, None, None)?, expected);

	// The caller's count is a limit like any other.
	append_tags(&mut store, 0..5)?;
	store.end_batch()?;
	append_tags(&mut store, 5..6)?;
	assert_eq!(
		drained(&mut store, Some(3), None)?,
		vec![vec![0, 1], vec![2, 3], vec![4], vec![5]]
	);
	Ok(())
}

#[test]
fn test_boundary_with_the_byte_ceiling() -> Result<()> {
	// Two 40 byte items fit in a batch under the limit; three do not.
	let limit = envelope(KEY) + 40 * 2 + 1;
	let mut store = store(limit);
	let items: Vec<Value> = (0..7).map(|n| item_of_len(40, n)).collect();
	for item in &items[..5] {
		store.append(item.clone())?;
	}
	store.end_batch()?;
	for item in &items[5..] {
		store.append(item.clone())?;
	}
	let batches = drain(&mut store, None, None)?;
	let sizes: Vec<usize> = batches.iter().map(Vec::len).collect();
	assert_eq!(sizes, vec![2, 2, 1, 2]);
	assert_eq!(batches.concat(), items);

	// The same with a per-call byte limit.
	for item in &items[..3] {
		store.append(item.clone())?;
	}
	store.end_batch()?;
	store.append(items[3].clone())?;
	let batches = drain(&mut store, None, Some(limit))?;
	let sizes: Vec<usize> = batches.iter().map(Vec::len).collect();
	assert_eq!(sizes, vec![2, 1, 1]);
	Ok(())
}

#[test]
fn test_scripted_boundaries_match_the_model() -> Result<()> {
	for (seed, per_batch) in [(1u32, 3usize), (42, 1), (0xdead_beef, 5), (99, 1000)] {
		let mut store = store_with_limits(None, Some(per_batch));
		let steps = script::steps(seed, 300);
		for step in &steps {
			match step {
				Step::Append(n) => store.append(tag(*n))?,
				Step::End => store.end_batch()?,
			}
		}
		assert_eq!(
			drained(&mut store, None, None)?,
			script::expected(&steps, per_batch),
			"seed {seed}"
		);
	}
	Ok(())
}
