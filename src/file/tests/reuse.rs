//! Slot reuse: removal in every order, then appends into the lowest free slot, with the
//! file not growing while a slot is free and fetch order by sequence throughout.

use super::support::*;

const ITEMS: usize = 5;

#[test]
fn test_removal_in_every_order_then_appends_reuse_the_lowest_free_slot() {
	for order in permutations(ITEMS) {
		// Remove some, never all: removing the last item cuts the file (see shrink).
		for removed in 1..ITEMS {
			let (_dir, path) = temp_path();
			let mut core = open(&path);
			for tag in 0..ITEMS {
				core.append_text(&item(6, tag)).unwrap();
			}
			let gone = &order[..removed];
			for &tag in gone {
				assert!(core.free(tag as u64 + 1).unwrap(), "{order:?}");
				assert_eq!(file_len(&path), len_of(ITEMS as u64), "{order:?}");
			}
			let mut kept: Vec<String> = (0..ITEMS)
				.filter(|tag| !gone.contains(tag))
				.map(|tag| item(6, tag))
				.collect();
			assert_eq!(texts(&mut core), kept, "{order:?}");

			// Each append takes the lowest free slot, whatever order they were freed in.
			let mut free: Vec<u64> = gone.iter().map(|&tag| tag as u64).collect();
			free.sort_unstable();
			for (n, slot) in free.into_iter().enumerate() {
				assert_eq!(core.index.free().next(), Some(slot), "{order:?}");
				core.append_text(&item(6, 100 + n)).unwrap();
				assert_eq!(slots_of(&core).last(), Some(&slot), "{order:?}");
				assert_eq!(file_len(&path), len_of(ITEMS as u64), "{order:?}");
				kept.push(item(6, 100 + n));
			}
			// Fetch order is by sequence, not slot.
			assert_eq!(texts(&mut core), kept, "{order:?}");
			// No slot is free: the next append grows the file by one slot.
			core.append_text(&item(6, 200)).unwrap();
			assert_eq!(file_len(&path), len_of(ITEMS as u64 + 1), "{order:?}");
			kept.push(item(6, 200));
			drop(core);
			assert_eq!(texts(&mut open(&path)), kept, "{order:?}");
		}
	}
}

#[test]
fn test_removing_what_is_not_live_is_ignored_and_writes_nothing() {
	let (_dir, path) = temp_path();
	let (mut core, hook) = open_with(&path, 10);
	core.append_text(&item(6, 0)).unwrap();
	core.append_text(&item(6, 1)).unwrap();
	assert!(core.free(1).unwrap());
	drain(&hook);
	assert!(!core.free(1).unwrap());
	assert!(!core.free(0).unwrap());
	assert!(!core.free(99).unwrap());
	assert!(drain(&hook).is_empty());
	assert_eq!(texts(&mut core), vec![item(6, 1)]);
}

#[test]
fn test_removal_is_one_state_byte() {
	let (_dir, path) = temp_path();
	let (mut core, hook) = open_with(&path, 10);
	core.append_text(&item(6, 0)).unwrap();
	core.append_text(&item(6, 1)).unwrap();
	let before = bytes_of(&path);
	drain(&hook);
	core.free(1).unwrap();
	assert_eq!(drain(&hook), vec![crate::file::faults::Op::State]);
	let after = bytes_of(&path);
	let changed: Vec<usize> = (0..before.len())
		.filter(|&i| before[i] != after[i])
		.collect();
	assert_eq!(changed, vec![layout().entry_offset(0) as usize]);
}

#[test]
fn test_a_removed_item_stays_removed_after_reopening() {
	let (_dir, path) = temp_path();
	let mut core = open(&path);
	for tag in 0..4 {
		core.append_text(&item(6, tag)).unwrap();
	}
	core.free(2).unwrap();
	core.free(4).unwrap();
	drop(core);
	let mut core = open(&path);
	assert_eq!(texts(&mut core), vec![item(6, 0), item(6, 2)]);
	assert_eq!(core.index.free().collect::<Vec<_>>(), vec![1, 3]);
}
