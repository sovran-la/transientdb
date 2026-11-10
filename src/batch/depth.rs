//! The nesting limit on items: the deepest item `serde_json` can read back from a batch.

use serde_json::Value;
use std::io;

/// The deepest an item may be nested in arrays and objects (`[]` and `{}` are one level,
/// a scalar none) and still be read back by `serde_json` inside a batch. `serde_json`
/// refuses input that reaches its recursion limit of 128 levels, so it reads at most 127;
/// the batch's own object and `batch` array take two of those. The number is measured,
/// not assumed: `test_max_item_depth_is_what_serde_json_reads_back` finds the boundary.
pub(crate) const MAX_ITEM_DEPTH: usize = 125;

/// The children of an array or object, for walking a `Value` without recursion.
enum Children<'a> {
	Array(std::slice::Iter<'a, Value>),
	Object(serde_json::map::Values<'a>),
}

impl<'a> Children<'a> {
	fn of(value: &'a Value) -> Option<Self> {
		match value {
			Value::Array(items) => Some(Self::Array(items.iter())),
			Value::Object(map) => Some(Self::Object(map.values())),
			_ => None,
		}
	}

	fn next(&mut self) -> Option<&'a Value> {
		match self {
			Self::Array(items) => items.next(),
			Self::Object(values) => values.next(),
		}
	}
}

/// Whether `value` is nested more than `limit` levels deep. Walks it without recursion,
/// keeping at most `limit + 1` levels, and stops at the first level past `limit`.
fn nested_deeper_than(value: &Value, limit: usize) -> bool {
	let mut levels: Vec<Children> = Children::of(value).into_iter().collect();
	while levels.len() <= limit {
		let Some(level) = levels.last_mut() else {
			return false;
		};
		match level.next() {
			Some(child) => levels.extend(Children::of(child)),
			None => {
				levels.pop();
			}
		}
	}
	true
}

/// Drops `value` without recursion, so even a very deeply nested one cannot overflow the
/// stack the way its own `Drop` would.
fn drop_iteratively(value: Value) {
	let mut pending = vec![value];
	while let Some(mut value) = pending.pop() {
		match &mut value {
			Value::Array(items) => pending.append(items),
			Value::Object(map) => pending.extend(std::mem::take(map).into_iter().map(|(_, v)| v)),
			_ => {}
		}
	}
}

/// Returns `item` if it is nested at most [`MAX_ITEM_DEPTH`] levels deep, so it can be
/// read back from any batch it is stored in. Otherwise drops it, without recursion, and
/// returns the error `append` returns for it.
///
/// Runs before anything serializes the item: serializing (and dropping) a `Value`
/// recurses once per level, so a pathologically deep one would overflow the stack.
pub(crate) fn within_depth(item: Value) -> io::Result<Value> {
	if !nested_deeper_than(&item, MAX_ITEM_DEPTH) {
		return Ok(item);
	}
	drop_iteratively(item);
	Err(io::Error::new(
		io::ErrorKind::InvalidInput,
		format!(
			"item is nested more than {MAX_ITEM_DEPTH} levels deep in arrays and objects; the deepest item that can be read back from a batch is nested {MAX_ITEM_DEPTH} levels"
		),
	))
}

/// How a test's nested item is built, level by level from the outside in.
#[cfg(test)]
#[derive(Debug, Clone, Copy)]
pub(crate) enum Shape {
	Arrays,
	Objects,
	/// Arrays and objects in turn, starting with an array.
	Mixed,
}

/// An item nested exactly `depth` levels deep in `shape`, holding `inner` at the bottom
/// (a scalar, or an empty array or object, which is a level itself). Built without
/// recursion, so it can be very deep; pass it to `append`, or drop it with
/// [`drop_iteratively`], never with its own `Drop` when deep.
#[cfg(test)]
pub(crate) fn nested(depth: usize, shape: Shape, inner: Value) -> Value {
	let inner_depth = usize::from(matches!(inner, Value::Array(_) | Value::Object(_)));
	let mut value = inner;
	for level in (0..depth - inner_depth).rev() {
		let array = match shape {
			Shape::Arrays => true,
			Shape::Objects => false,
			Shape::Mixed => level % 2 == 0,
		};
		value = if array {
			Value::Array(vec![value])
		} else {
			Value::Object([("k".to_string(), value)].into_iter().collect())
		};
	}
	value
}

#[cfg(test)]
pub(crate) const SHAPES: [Shape; 3] = [Shape::Arrays, Shape::Objects, Shape::Mixed];

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	/// Whether `serde_json` reads `item` back from inside a batch envelope.
	fn reads_back_in_a_batch(item: &Value) -> bool {
		let mut bytes = b"{\"batch\":[".to_vec();
		bytes.extend(serde_json::to_vec(item).unwrap());
		bytes.extend_from_slice(b"],\"sentAt\":\"s\",\"writeKey\":\"k\"}");
		serde_json::from_slice::<Value>(&bytes).is_ok_and(|batch| batch["batch"][0] == *item)
	}

	#[test]
	fn test_max_item_depth_is_what_serde_json_reads_back() {
		for shape in SHAPES {
			for inner in [json!(1), json!([]), json!({}), json!("s")] {
				let case = format!("{shape:?} around {inner}");
				// Every depth up to the limit reads back; the first one past it does not.
				let deepest = (1..200)
					.take_while(|&depth| {
						reads_back_in_a_batch(&nested(depth, shape, inner.clone()))
					})
					.last()
					.unwrap();
				assert_eq!(deepest, MAX_ITEM_DEPTH, "{case}");
				assert!(
					!reads_back_in_a_batch(&nested(MAX_ITEM_DEPTH + 1, shape, inner.clone())),
					"{case}"
				);
			}
		}
	}

	#[test]
	fn test_within_depth_accepts_up_to_the_limit_and_rejects_past_it() {
		for shape in SHAPES {
			for inner in [json!(null), json!([]), json!({})] {
				for depth in [1, 2, MAX_ITEM_DEPTH - 1, MAX_ITEM_DEPTH] {
					let item = nested(depth, shape, inner.clone());
					assert_eq!(
						within_depth(item.clone()).unwrap(),
						item,
						"{shape:?} {depth}"
					);
				}
				for depth in [MAX_ITEM_DEPTH + 1, MAX_ITEM_DEPTH + 2, 1000] {
					let err = within_depth(nested(depth, shape, inner.clone())).unwrap_err();
					assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{shape:?} {depth}");
				}
			}
		}
		for scalar in [json!(null), json!(1), json!("s"), json!(true)] {
			assert_eq!(within_depth(scalar.clone()).unwrap(), scalar);
		}
	}

	#[test]
	fn test_depth_counts_the_deepest_branch_only() {
		// Wide and shallow is fine however many containers it holds.
		let wide = json!((0..1000).map(|i| json!({"i": [i]})).collect::<Vec<_>>());
		assert!(within_depth(wide).is_ok());
		// One deep branch after many shallow ones is found: inside an outer array, a
		// branch of MAX_ITEM_DEPTH - 1 levels is exactly at the limit, one more is past it.
		let mut items: Vec<Value> = (0..100).map(|i| json!([i])).collect();
		items.push(nested(MAX_ITEM_DEPTH, Shape::Mixed, json!(0)));
		assert!(within_depth(json!(items.clone())).is_err());
		items.pop();
		items.push(nested(MAX_ITEM_DEPTH - 1, Shape::Mixed, json!(0)));
		items.push(json!("after"));
		assert!(within_depth(json!(items)).is_ok());
	}

	#[test]
	fn test_very_deep_item_is_rejected_without_overflowing_the_stack() {
		for shape in SHAPES {
			let err = within_depth(nested(100_000, shape, json!(0))).unwrap_err();
			assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
		}
		// Torn down without recursion by the test itself, too.
		drop_iteratively(nested(100_000, Shape::Mixed, json!({})));
	}

	#[test]
	fn test_too_deep_message_states_the_limit() {
		let err = within_depth(nested(200, Shape::Arrays, json!(0))).unwrap_err();
		assert_eq!(
			err.to_string(),
			"item is nested more than 125 levels deep in arrays and objects; the deepest item that can be read back from a batch is nested 125 levels"
		);
	}

	#[test]
	fn test_nested_builds_the_requested_depth() {
		assert_eq!(nested(1, Shape::Arrays, json!(0)), json!([0]));
		assert_eq!(nested(1, Shape::Arrays, json!([])), json!([]));
		assert_eq!(nested(2, Shape::Objects, json!({})), json!({"k": {}}));
		assert_eq!(nested(3, Shape::Mixed, json!(0)), json!([{"k": [0]}]));
		for depth in 1..10 {
			let item = nested(depth, Shape::Mixed, json!(0));
			assert!(!nested_deeper_than(&item, depth), "{depth}");
			assert!(nested_deeper_than(&item, depth - 1), "{depth}");
		}
	}
}
