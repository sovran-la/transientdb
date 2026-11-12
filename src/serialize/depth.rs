//! The nesting limit on items: the deepest item `serde_json` can read back once a caller
//! has put it in a request body.

use crate::{Error, Result};
use serde_json::Value;

/// The deepest an item may be nested in arrays and objects (`[]` and `{}` are one level,
/// a scalar none) and still be read back by `serde_json` from a request body built the
/// way [`Items::json`](crate::Items::json) is meant to be used, the array inside the
/// caller's object: `{"events":[...]}`. `serde_json` refuses input that reaches its recursion limit of 128 levels, so
/// it reads at most 127; the caller's object and the items array take two of those. The
/// number is measured, not assumed: `test_max_item_depth_is_what_serde_json_reads_back`
/// finds the boundary.
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
pub(crate) fn drop_iteratively(value: Value) {
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
/// read back from a request body it is sent in. Otherwise drops it, without recursion,
/// and returns [`Error::ItemTooDeep`].
///
/// Runs before anything serializes the item: serializing (and dropping) a `Value`
/// recurses once per level, so a pathologically deep one would overflow the stack.
pub(crate) fn within_depth(item: Value) -> Result<Value> {
	if !nested_deeper_than(&item, MAX_ITEM_DEPTH) {
		return Ok(item);
	}
	drop_iteratively(item);
	Err(Error::ItemTooDeep)
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
mod tests;
