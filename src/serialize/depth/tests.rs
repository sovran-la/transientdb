use super::*;
use serde_json::json;

/// Whether `serde_json` reads `item` back from a request body made the documented way:
/// `{"events":` + the items' JSON array + `}`, here with a second, ordinary item beside
/// it. Whatever the caller's key, the items sit two levels deep.
fn reads_back_in_a_request_body(item: &Value) -> bool {
	let array = format!("[{},{}]", item, json!({"plain": [1]}));
	let body = format!("{{\"events\":{array}}}");
	serde_json::from_str::<Value>(&body).is_ok_and(|body| body["events"][0] == *item)
}

/// Whether `serde_json` reads `item` back from its own text, as `Items::values` does.
fn reads_back_alone(item: &Value) -> bool {
	serde_json::from_str::<Value>(&item.to_string()).is_ok_and(|back| back == *item)
}

#[test]
fn test_max_item_depth_is_what_serde_json_reads_back() {
	for shape in SHAPES {
		for inner in [json!(1), json!([]), json!({}), json!("s")] {
			let case = format!("{shape:?} around {inner}");
			// Every depth up to the limit reads back; the first one past it does not.
			let deepest = (1..200)
				.take_while(|&depth| {
					reads_back_in_a_request_body(&nested(depth, shape, inner.clone()))
				})
				.last()
				.unwrap();
			assert_eq!(deepest, MAX_ITEM_DEPTH, "{case}");
			assert!(
				!reads_back_in_a_request_body(&nested(MAX_ITEM_DEPTH + 1, shape, inner.clone())),
				"{case}"
			);
			// On its own, as `values()` parses it, the deepest accepted item has room to spare.
			assert!(
				reads_back_alone(&nested(MAX_ITEM_DEPTH, shape, inner.clone())),
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
				assert!(matches!(err, Error::ItemTooDeep), "{shape:?} {depth}");
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
		assert!(matches!(err, Error::ItemTooDeep));
	}
	// Torn down without recursion by the test itself, too.
	drop_iteratively(nested(100_000, Shape::Mixed, json!({})));
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
