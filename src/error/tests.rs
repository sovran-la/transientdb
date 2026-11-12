use super::*;
use std::error::Error as _;

#[test]
fn test_display_text_of_every_variant() {
	assert_eq!(
		Error::ItemTooLarge { size: 500, max: 64 }.to_string(),
		"item is 500 bytes serialized, larger than max_item_size = 64 bytes"
	);
	assert_eq!(
		Error::ItemTooDeep.to_string(),
		"item is nested more than 125 levels deep in arrays and objects"
	);
	assert_eq!(
		Error::InUse.to_string(),
		"another store in this process is using this file"
	);
	let io = Error::Io(io::Error::new(io::ErrorKind::NotFound, "no such file"));
	assert_eq!(io.to_string(), "I/O error: no such file");
}

#[test]
fn test_only_io_has_a_source() {
	let io = Error::Io(io::Error::new(io::ErrorKind::PermissionDenied, "locked"));
	let source = io.source().expect("Io has a source");
	let inner = source.downcast_ref::<io::Error>().expect("an io::Error");
	assert_eq!(inner.kind(), io::ErrorKind::PermissionDenied);
	assert_eq!(inner.to_string(), "locked");

	assert!(Error::ItemTooLarge { size: 2, max: 1 }.source().is_none());
	assert!(Error::ItemTooDeep.source().is_none());
	assert!(Error::InUse.source().is_none());
}

#[test]
fn test_from_io_error_keeps_it_whole() {
	let err: Error = io::Error::new(io::ErrorKind::WriteZero, "disk full").into();
	match err {
		Error::Io(e) => {
			assert_eq!(e.kind(), io::ErrorKind::WriteZero);
			assert_eq!(e.to_string(), "disk full");
		}
		other => panic!("expected Io, got {other:?}"),
	}
}

#[test]
fn test_question_mark_converts_io_errors() {
	fn fails() -> Result<()> {
		Err(io::Error::other("boom"))?;
		Ok(())
	}
	assert!(matches!(fails(), Err(Error::Io(e)) if e.to_string() == "boom"));
}

#[test]
fn test_debug_names_the_variant_and_fields() {
	assert_eq!(
		format!("{:?}", Error::ItemTooLarge { size: 3, max: 2 }),
		"ItemTooLarge { size: 3, max: 2 }"
	);
	assert_eq!(format!("{:?}", Error::ItemTooDeep), "ItemTooDeep");
	assert_eq!(format!("{:?}", Error::InUse), "InUse");
	assert!(format!("{:?}", Error::Io(io::Error::other("x"))).starts_with("Io("));
}

#[test]
fn test_error_is_send_sync_and_static() {
	fn assert_bounds<T: std::error::Error + Send + Sync + 'static>() {}
	assert_bounds::<Error>();
	// Usable as a boxed error, as `?` in a caller's `Box<dyn Error>` function needs.
	let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(Error::InUse);
	assert_eq!(boxed.to_string(), Error::InUse.to_string());
}
