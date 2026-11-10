//! The JSON object every store wraps a batch of items in.
//!
//! Every byte of the envelope comes from here: what `DirectoryStore` writes, what all
//! three stores count against their byte limits, and what recovery compares against.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use std::collections::HashSet;

/// The shape of the JSON object a batch of items is wrapped in.
///
/// A batch is one JSON object: the items in an array under `items_key`, then each of
/// `fields`, then, if `timestamp_key` is set, the time the batch was finished.
/// [`Envelope::default()`] is `{"items":[...]}` and nothing else.
///
/// ```
/// use serde_json::json;
/// use transientdb::Envelope;
///
/// // {"events":[...]}
/// let renamed = Envelope::new("events");
///
/// // {"events":[...],"source":"sensor-7","sentAt":"2026-11-10T17:04:05.123Z"}
/// let tagged = Envelope::new("events")
///     .field("source", "sensor-7")
///     .timestamp("sentAt");
///
/// // The builder is shorthand for the public fields.
/// assert_eq!(
///     tagged,
///     Envelope {
///         items_key: "events".into(),
///         fields: vec![("source".into(), json!("sensor-7"))],
///         timestamp_key: Some("sentAt".into()),
///     }
/// );
/// assert_eq!(Envelope::default(), Envelope::new("items"));
/// ```
///
/// Field values are written exactly as given, compactly serialized by `serde_json`, and
/// can be any JSON. The timestamp is UTC in the fixed-width format
/// `%Y-%m-%dT%H:%M:%S.%3fZ` (`2026-11-10T17:04:05.123Z`, always 24 bytes), so for a given
/// envelope every batch's envelope is the same size, and the stores' byte limits count it
/// exactly.
///
/// Each store's `new` panics if `items_key`, a field key or the timestamp key is empty, if
/// any two of those keys are the same, or if the envelope does not read back as JSON (a
/// field value nested too deeply for `serde_json`, for example).
///
/// Changing the envelope between sessions is not supported: `DirectoryStore` files already
/// closed under the old one are delivered as they were written, and unfinished ones are
/// closed with the new envelope or discarded.
#[derive(Clone, Debug, PartialEq)]
pub struct Envelope {
	/// Name of the array that holds the items.
	pub items_key: String,
	/// Other top-level keys and their values, written exactly as given.
	pub fields: Vec<(String, Value)>,
	/// If set, the time the batch was finished is written under this key.
	pub timestamp_key: Option<String>,
}

impl Default for Envelope {
	/// `{"items":[...]}`: no fields and no timestamp.
	fn default() -> Self {
		Self::new("items")
	}
}

/// The format of the batch timestamp: UTC, millisecond precision, fixed width.
const TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S.%3fZ";

/// The timestamp written for a batch finished at `at`, in the fixed-width format.
pub(crate) fn timestamp(at: DateTime<Utc>) -> String {
	at.format(TIMESTAMP_FORMAT).to_string()
}

/// Appends `key` as a JSON string, then `:`.
fn push_key(out: &mut Vec<u8>, key: &str) {
	out.extend(serde_json::to_vec(key).expect("a string always serializes"));
	out.push(b':');
}

impl Envelope {
	/// An envelope with the items under `items_key`, and no fields or timestamp.
	pub fn new(items_key: impl Into<String>) -> Self {
		Self {
			items_key: items_key.into(),
			fields: Vec::new(),
			timestamp_key: None,
		}
	}

	/// Adds a field, written after the items and any fields added before it.
	pub fn field(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
		self.fields.push((key.into(), value.into()));
		self
	}

	/// Writes the time the batch was finished under `key`, after every field.
	pub fn timestamp(mut self, key: impl Into<String>) -> Self {
		self.timestamp_key = Some(key.into());
		self
	}

	/// The bytes before the first item: `{"<items_key>":[`.
	pub(crate) fn opening(&self) -> Vec<u8> {
		let mut out = vec![b'{'];
		push_key(&mut out, &self.items_key);
		out.push(b'[');
		out
	}

	/// The bytes after the last item, for a batch finished at `finished_at`: `]`, each
	/// field, the timestamp if configured, `}`. Its length does not depend on the time.
	pub(crate) fn closing(&self, finished_at: DateTime<Utc>) -> Vec<u8> {
		let mut out = vec![b']'];
		for (key, value) in &self.fields {
			out.push(b',');
			push_key(&mut out, key);
			out.extend(serde_json::to_vec(value).expect("a Value always serializes"));
		}
		if let Some(key) = &self.timestamp_key {
			out.push(b',');
			push_key(&mut out, key);
			out.extend(
				serde_json::to_vec(&timestamp(finished_at)).expect("a string always serializes"),
			);
		}
		out.push(b'}');
		out
	}

	/// Length in bytes of the opening and the closing together: the size of a batch with
	/// no items, however it is serialized compactly.
	pub(crate) fn byte_len(&self) -> usize {
		self.opening().len() + self.closing(DateTime::UNIX_EPOCH).len()
	}

	/// The batch as a `Value`: `items` under the items key, each field under its key, and
	/// the timestamp for `finished_at` if configured. Its compact serialization has the
	/// same length as `opening`, the items and their separators, and `closing`.
	pub(crate) fn batch(&self, items: Vec<Value>, finished_at: DateTime<Utc>) -> Value {
		let mut map = Map::new();
		map.insert(self.items_key.clone(), Value::Array(items));
		for (key, value) in &self.fields {
			map.insert(key.clone(), value.clone());
		}
		if let Some(key) = &self.timestamp_key {
			map.insert(key.clone(), Value::String(timestamp(finished_at)));
		}
		Value::Object(map)
	}

	/// Panics if the envelope cannot describe a batch that reads back: an empty key, a
	/// key used twice, or a batch that `serde_json` cannot parse.
	pub(crate) fn validate(&self) {
		if self.items_key.is_empty() {
			panic!("Envelope items_key is empty? The items need somewhere to live.");
		}
		if self.fields.iter().any(|(key, _)| key.is_empty()) {
			panic!(
				"An envelope field with an empty key? `\"\":` is legal JSON and a terrible idea."
			);
		}
		if self.timestamp_key.as_deref() == Some("") {
			panic!("Envelope timestamp_key is empty? Name it, or leave it None.");
		}
		let mut seen = HashSet::new();
		let keys = std::iter::once(&self.items_key)
			.chain(self.fields.iter().map(|(key, _)| key))
			.chain(self.timestamp_key.as_ref());
		for key in keys {
			if !seen.insert(key) {
				panic!("Envelope key {key:?} is used twice? A JSON object only keeps one of them.");
			}
		}
		let empty = serde_json::to_string(&self.batch(Vec::new(), DateTime::UNIX_EPOCH))
			.expect("a Value always serializes");
		if serde_json::from_str::<Value>(&empty).is_err() {
			panic!(
				"This envelope doesn't read back as JSON? Then nobody can read the batches either."
			);
		}
	}
}

#[cfg(test)]
pub(crate) mod samples;
#[cfg(test)]
mod tests;
