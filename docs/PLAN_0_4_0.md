# transientdb 0.4.0 Plan

**Date:** 2026-11-10
**Companion:** [`FILE_STORE_SPEC.md`](FILE_STORE_SPEC.md)

This plan records the design of 0.4.0. The FileStore spec covers FileStore in
detail; where this plan is silent, the spec decides.

## What 0.4.0 is

Three stores with one surface: append an item, fetch items, remove items.

- **FileStore** is new. It replaces DirectoryStore.
- **MemoryStore** and **WebStore** are changed to the same surface.
- **DirectoryStore, `Envelope`, `end_batch`, `DataResult`, `Equivalent`** and the
  `Output` associated type are removed. They are not deprecated or kept behind a
  feature. Nothing but this project uses the crate.

The stores hold items and hand items back. They know nothing about how a caller
sends them.

## The trait

```rust
pub trait DataStore {
    fn append(&mut self, data: serde_json::Value) -> Result<()>;
    fn fetch(&mut self, count: usize, max_bytes: usize) -> Result<Option<Items>>;
    fn remove(&mut self, items: Items) -> Result<()>;
    fn reset(&mut self);
    fn has_data(&self) -> bool;
}
```

`TransientDB` wraps a store behind a mutex as it did in 0.3.0, exposes the same
five operations, and loses its type parameter.

`Items`, `Error` and `Result` are crate-level types shared by all three stores.
Their definitions and behavior are in the FileStore spec, in the sections
"fetch", "remove" and "Errors". In particular, for every store:

- `fetch` takes two required limits and returns `Ok(None)` when there is nothing
  to hand over. An `Items` is never empty.
- Fetched items are pending: no later fetch returns them until the `Items` is
  released. `remove` takes the `Items` by value. Dropping an `Items` releases
  it; `release()` is the explicit form. Pending is kept in memory only.
- `Items` offers `len`, `byte_len`, `json`, `values`, `iter` and `release`, and
  none of them can fail.
- An item is checked when it is appended; what is fetched is what was stored.
  No store parses an item's JSON again when it reads the item back.
- `has_data` is true only if a fetch could return something.
- `max_bytes` counts item bytes only.
- A store that is full drops its oldest item on append. This is not
  configurable.
- Item states (available, pending, gone) are internal. Nothing in the public API
  or its documentation names them.

`Items` must be `Send` on native targets, so a caller can fetch on one thread
and remove on another. Releasing must not need a reference to the store, and
must not deadlock when an `Items` is dropped while the store's mutex is held, or
after the store itself has been dropped.

## Configuration

All three stores take the same two limits, so an item one store accepts, every
store accepts:

| Store | Config |
|---|---|
| FileStore | `path`, `id`, `max_item_size`, `max_items` |
| MemoryStore | `max_item_size`, `max_items` |
| WebStore | `database_name`, `max_item_size`, `max_items` |

Both limits are required and neither may be 0. `max_fetch_size`,
`max_fetch_items`, `max_file_size`, `max_file_items` and every `envelope` field
are removed.

## Store by store

**FileStore.** As specified in `FILE_STORE_SPEC.md`, all of it, including its
testing section. Native targets only.

**MemoryStore.** Holds each item as its serialized JSON text, with a sequence
number. Same append checks as FileStore (size, depth), same fetch order, same
drop-oldest rule. `Error::InUse` and `Error::Io` never arise.

**WebStore.** Keeps its 0.3.0 persistence design; only its surface and its
stored record change. Records hold the item's JSON text and its sequence number
and nothing about batches. The database version is raised and records written by
any earlier version are discarded on upgrade, with no migration. Pending is kept
in memory, so after a reload every stored item is available again.

## Also in scope

- `chrono` is removed: nothing uses it once the envelope is gone.
- `examples/directory_example.rs` is replaced with a FileStore example, and the
  memory and web examples are brought up to date.
- `README.md` is rewritten for the 0.4.0 API. It describes what the crate does
  now, not what changed.
- `.github/workflows/ci.yml` and `test_all.sh` exercise FileStore along with
  the rest.
- Tests follow the existing layout: child modules under `src/<store>/tests/`,
  integration tests under `tests/`.
- Source files stay near 300 lines or under, split by responsibility.

## Not in scope

- The version number, `release.sh`, `tools/release`, tags and publishing.
- `LEDGER_SPEC.md` and `STORAGE_PROPOSAL.md` at the repository root.
- Any other repository, including twilio-data-core2.
- Shrinking a file that still holds items, or compaction.

## Constraints

- No new dependencies. No Tokio. No async runtime on native targets.
- CRC-32 is implemented in the crate, as the spec describes.
- No buffering of file writes in user space.
- Tests are exhaustive: every operation, every error, every boundary, and the
  failure cases the spec lists.
