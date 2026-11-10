//! Tests of the envelope: its exact bytes, its `Value` form, its timestamp and its
//! validation.

use super::*;
use serde_json::json;

mod bytes;
mod validation;
mod value;

/// A fixed finishing time, so expected bytes are reproducible.
fn at() -> DateTime<Utc> {
	DateTime::parse_from_rfc3339("2026-11-09T12:34:56.789Z")
		.unwrap()
		.to_utc()
}

fn opening(envelope: &Envelope) -> String {
	String::from_utf8(envelope.opening()).unwrap()
}

fn closing(envelope: &Envelope) -> String {
	String::from_utf8(envelope.closing(at())).unwrap()
}

/// `opening + items + closing`, items separated by commas, as `DirectoryStore` writes it.
fn file_bytes(envelope: &Envelope, items: &[Value]) -> String {
	let items: Vec<String> = items.iter().map(Value::to_string).collect();
	format!(
		"{}{}{}",
		opening(envelope),
		items.join(","),
		closing(envelope)
	)
}
