//! Tab ids: each `WebStore` instance's own, under which its records are kept.

use js_sys::{Function, Reflect};
use wasm_bindgen::{JsCast, JsValue};

/// A new tab id: `crypto.randomUUID()`, or, where that is missing (an older browser),
/// 16 random bytes as hex.
pub(super) fn new_id() -> String {
	random_uuid().unwrap_or_else(random_hex)
}

fn random_uuid() -> Option<String> {
	let crypto = Reflect::get(&js_sys::global(), &JsValue::from_str("crypto")).ok()?;
	let random_uuid = Reflect::get(&crypto, &JsValue::from_str("randomUUID")).ok()?;
	let random_uuid: Function = random_uuid.dyn_into().ok()?;
	random_uuid.call0(&crypto).ok()?.as_string()
}

/// 16 bytes from `crypto.getRandomValues`, as 32 lowercase hex digits; from
/// `Math.random` if even that is missing.
pub(super) fn random_hex() -> String {
	let mut bytes = [0u8; 16];
	if getrandom::getrandom(&mut bytes).is_err() {
		for byte in &mut bytes {
			*byte = (js_sys::Math::random() * 256.0) as u8;
		}
	}
	bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::collections::HashSet;
	use wasm_bindgen_test::wasm_bindgen_test;

	fn is_uuid(id: &str) -> bool {
		let groups: Vec<&str> = id.split('-').collect();
		let lengths: Vec<usize> = groups.iter().map(|g| g.len()).collect();
		lengths == [8, 4, 4, 4, 12]
			&& groups
				.iter()
				.all(|g| g.chars().all(|c| c.is_ascii_hexdigit()))
	}

	#[wasm_bindgen_test]
	fn test_ids_are_uuids_where_the_browser_has_random_uuid() {
		let id = new_id();
		assert!(is_uuid(&id), "{id}");
	}

	#[wasm_bindgen_test]
	fn test_the_fallback_is_32_hex_digits() {
		let id = random_hex();
		assert_eq!(id.len(), 32);
		assert!(id
			.chars()
			.all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
	}

	#[wasm_bindgen_test]
	fn test_ids_do_not_repeat() {
		let ids: HashSet<String> = (0..1000).map(|_| new_id()).collect();
		assert_eq!(ids.len(), 1000);
		let hex: HashSet<String> = (0..1000).map(|_| random_hex()).collect();
		assert_eq!(hex.len(), 1000);
	}
}
