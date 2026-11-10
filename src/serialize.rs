//! The JSON text a store keeps for an item, and the checks every store applies to it.
//!
//! A store holds each item as its compact `serde_json` text. `append` produces that text
//! with [`item_text`]; a store that reads text back from storage checks it with
//! [`check_text`], so everything it later hands out in an [`Items`](crate::Items) is
//! well-formed and within the same limits `append` enforces.

use crate::{Error, Result};
use serde_json::Value;
use std::fmt;

mod depth;

pub(crate) use depth::MAX_ITEM_DEPTH;
#[cfg(test)]
pub(crate) use depth::{nested, Shape, SHAPES};

/// The text a store keeps for `item`: checked for depth first, then serialized compactly
/// and checked against `max_item_size`.
///
/// # Errors
/// [`Error::ItemTooDeep`] if `item` is nested more than [`MAX_ITEM_DEPTH`] levels, checked
/// without recursion before anything serializes it; [`Error::ItemTooLarge`] if its text
/// is longer than `max_item_size` bytes.
pub(crate) fn item_text(item: Value, max_item_size: usize) -> Result<String> {
	let item = depth::within_depth(item)?;
	let text = item.to_string();
	if text.len() > max_item_size {
		return Err(Error::ItemTooLarge {
			size: text.len(),
			max: max_item_size,
		});
	}
	Ok(text)
}

/// Why stored text is not an item a store may hand out.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(all(feature = "web", target_arch = "wasm32")), allow(dead_code))]
pub(crate) enum Unfit {
	/// Not one well-formed JSON value.
	Malformed,
	/// Longer than `max_item_size`.
	TooLarge { size: usize, max: usize },
	/// Nested more than [`MAX_ITEM_DEPTH`] levels.
	TooDeep,
}

impl fmt::Display for Unfit {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Malformed => write!(f, "it is not well-formed JSON"),
			Self::TooLarge { size, max } => {
				write!(f, "it is {size} bytes, over max_item_size = {max} bytes")
			}
			Self::TooDeep => write!(f, "it is nested more than {MAX_ITEM_DEPTH} levels deep"),
		}
	}
}

/// Checks text read back from storage: it must be within `max_item_size`, one
/// well-formed JSON value, and no deeper than `append` allows.
#[cfg_attr(not(all(feature = "web", target_arch = "wasm32")), allow(dead_code))]
pub(crate) fn check_text(text: &str, max_item_size: usize) -> std::result::Result<(), Unfit> {
	if text.len() > max_item_size {
		return Err(Unfit::TooLarge {
			size: text.len(),
			max: max_item_size,
		});
	}
	// serde_json refuses anything deeper than its own recursion limit, so parsing cannot
	// overflow the stack, and what it returns is shallow enough to drop normally.
	let value: Value = serde_json::from_str(text).map_err(|_| Unfit::Malformed)?;
	if depth::nested_deeper_than(&value, MAX_ITEM_DEPTH) {
		return Err(Unfit::TooDeep);
	}
	Ok(())
}

#[cfg(test)]
mod tests;
