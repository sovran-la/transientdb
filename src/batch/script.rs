//! For tests: a deterministic, varied sequence of appends and `end_batch` calls, and the
//! grouping into batches every store must produce from it.

/// One call in a script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
	/// Append the item tagged with this number; tags count up from 0.
	Append(usize),
	/// Call `end_batch`.
	End,
}

/// `len` steps from `seed`, about one in four an `end_batch`, so runs of several
/// boundaries in a row, boundaries at the very start, and long spans all occur.
pub(crate) fn steps(seed: u32, len: usize) -> Vec<Step> {
	let mut state = seed.max(1);
	let mut next = move |below: u32| {
		state ^= state << 13;
		state ^= state >> 17;
		state ^= state << 5;
		state % below
	};
	let mut tag = 0;
	(0..len)
		.map(|_| {
			if next(4) == 0 {
				Step::End
			} else {
				tag += 1;
				Step::Append(tag - 1)
			}
		})
		.collect()
}

/// The tags of each batch, in order: the appends between boundaries, with empty
/// spans dropped, each cut into batches of at most `per_batch` items.
pub(crate) fn expected(steps: &[Step], per_batch: usize) -> Vec<Vec<usize>> {
	let mut spans = vec![Vec::new()];
	for step in steps {
		match step {
			Step::Append(tag) => spans.last_mut().unwrap().push(*tag),
			Step::End => spans.push(Vec::new()),
		}
	}
	spans
		.iter()
		.flat_map(|span| span.chunks(per_batch).map(<[usize]>::to_vec))
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;
	use Step::{Append, End};

	#[test]
	fn test_expected_groups_spans_and_cuts_them() {
		let script = [
			End,
			Append(0),
			Append(1),
			Append(2),
			End,
			End,
			Append(3),
			End,
		];
		assert_eq!(expected(&script, 10), vec![vec![0, 1, 2], vec![3]]);
		assert_eq!(expected(&script, 2), vec![vec![0, 1], vec![2], vec![3]]);
		assert!(expected(&[End, End], 3).is_empty());
	}

	#[test]
	fn test_steps_are_deterministic_and_varied() {
		let script = steps(7, 400);
		assert_eq!(script, steps(7, 400));
		let ends = script.iter().filter(|s| **s == End).count();
		assert!((50..150).contains(&ends), "{ends} boundaries");
		assert!(
			script.windows(2).any(|w| w == [End, End]),
			"no repeated boundary"
		);
		let tags: Vec<usize> = script
			.iter()
			.filter_map(|s| match s {
				Append(tag) => Some(*tag),
				End => None,
			})
			.collect();
		assert_eq!(tags, (0..tags.len()).collect::<Vec<_>>());
	}
}
