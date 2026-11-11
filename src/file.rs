//! FileStore: a disk-backed queue that keeps every item in one file.
//!
//! The file is a 256 byte header followed by slots, all one size, so any free slot holds
//! any item. A removed item's slot is reused by a later append, and the file grows by one
//! slot only when no slot is free. Nothing in the file is ever moved, rewritten or
//! renamed, so there is nothing to recover or compact. `docs/FILE_STORE_SPEC.md` is the
//! full specification.
//!
//! - [`crc`], [`header`], [`entry`] and [`layout`]: the file format.
//! - [`handle`]: the open file, unbuffered, with [`faults`] for tests.
//! - [`registry`]: the paths in use by stores in this process.
//! - [`index`]: which slots are live, with which items, and which are free.
//! - [`core`], [`scan`], [`write`] and [`read`]: what the store does to its file.
//! - [`store`]: [`FileStore`] itself, which adds the registry and the claim ledger.

mod config;
mod core;
mod crc;
mod entry;
mod faults;
mod handle;
mod header;
mod index;
mod layout;
mod read;
mod registry;
mod scan;
mod store;
mod write;

pub use config::FileConfig;
pub use store::FileStore;

#[cfg(test)]
mod tests {
	mod append;
	mod config;
	mod crc;
	mod entry;
	mod fetch;
	mod full;
	mod handle;
	mod header;
	mod index;
	mod io_errors;
	mod kill;
	mod layout;
	mod mismatch;
	mod open;
	mod registry;
	mod reuse;
	mod shrink;
	mod store;
	mod support;
	mod two_cores;
}
