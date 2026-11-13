//! Items come back from IndexedDB, adopted after a reload, exactly as they were appended: each record holds its
//! item as JSON text, so numbers a JavaScript number cannot hold, and floats with no
//! fractional part, survive a reload, in every view of the fetched items.

use super::*;
use crate::item_samples::{assert_exact, awkward_items};

#[wasm_bindgen_test]
async fn test_awkward_items_are_adopted_exactly() {
	let db_name = "test-exact-items-round-trip";
	let Some(mut store) = persisted(db_name, 4096, 1000).await else {
		return;
	};
	let items = awkward_items();
	contract::append_all(&mut store, &items);
	drop(store);

	let mut reopened = WebStore::new(config(db_name, 4096, 1000)).await;
	assert!(reopened.is_persisted());
	assert_exact(&queued(&reopened), &items);
	let fetched = reopened.fetch(100, usize::MAX).unwrap().unwrap();
	assert_exact(&fetched.values(), &items);
	let texts: Vec<String> = items.iter().map(Value::to_string).collect();
	assert_eq!(fetched.iter().collect::<Vec<_>>(), texts);
	assert_eq!(fetched.json(), format!("[{}]", texts.join(",")));
	drop(fetched);
	reopened.reset();
	settle().await;
}
