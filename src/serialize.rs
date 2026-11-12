//! The JSON text a store keeps for an item, and the checks `append` applies to it.
//!
//! A store holds each item as its compact `serde_json` text, produced by [`item_text`].
//! An item is checked once, here, when it is appended; a store trusts what it wrote when
//! it reads it back, so what is fetched is what was stored.

use crate::{Error, Result};
use serde_json::Value;

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

#[cfg(test)]
mod tests;
