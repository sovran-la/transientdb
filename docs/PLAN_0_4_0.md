# transientdb 0.4.0 Plan

**Date:** 2026-11-10
**Companion:** [`FILE_STORE_SPEC.md`](FILE_STORE_SPEC.md)

This plan is the contract for the 0.4.0 work. Where it and the FileStore spec
agree, both apply. Where this plan is silent, the FileStore spec decides. Where
neither decides, make the simplest choice that matches the rest, record it, and
keep going.

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

`TransientDB` wraps a store behind a mutex as it does today, exposes the same
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
  none of them can fail. Every item in an `Items` is well-formed JSON text.
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
| WebStore | what identifies its database today, plus `max_item_size`, `max_items` |

Both limits are required and neither may be 0. `max_fetch_size`,
`max_fetch_items`, `max_file_size`, `max_file_items` and every `envelope` field
are removed.

## Store by store

**FileStore.** As specified in `FILE_STORE_SPEC.md`, all of it, including the
test requirements. Native targets only.

**MemoryStore.** Holds each item as its serialized JSON text, with a sequence
number. Same append checks as FileStore (size, depth), same fetch order, same
drop-oldest rule. `Error::InUse` and `Error::Io` never arise.

**WebStore.** Keeps its present persistence design; only its surface and its
stored record change. Records hold the item's JSON text and its sequence number
and nothing about batches. The database version is raised and records written by
any earlier version are discarded on upgrade, with no migration. Pending is kept
in memory, so after a reload every stored item is available again.

## Also in scope

- Remove `chrono` if nothing uses it once the envelope is gone.
- Replace `examples/directory_example.rs` with a FileStore example, and bring
  the memory and web examples up to date.
- Rewrite `README.md` for the 0.4.0 API. Describe what the crate does now; do
  not narrate what changed.
- Keep `.github/workflows/ci.yml` and `test_all.sh` working, and add FileStore
  to whatever they exercise.
- Follow the existing test layout: tests in child modules under
  `src/<store>/tests/`, integration tests under `tests/`.
- Keep source files near 300 lines or under; split by responsibility.

## Not in scope

- The version number, `release.sh`, `tools/release`, tags and publishing.
- `LEDGER_SPEC.md` and `STORAGE_PROPOSAL.md` at the repository root. Leave them.
- Any other repository, including twilio-data-core2.
- Shrinking a file that still holds items, or compaction.
- Changes to the design. If the spec turns out to be wrong or impossible
  somewhere, implement the closest sound behavior, and record exactly what
  differs and why under "Questions for review" (see below).

## Constraints

- No new dependencies. No Tokio. No async runtime on native targets.
- CRC-32 is implemented in the crate, as the spec describes.
- No buffering of file writes in user space.
- Tests are exhaustive: every operation, every error, every boundary, and the
  failure cases the spec lists. A behavior without a test is not done.
- Nothing written to the repository or to GitHub says or implies that an AI
  wrote it: no co-author trailers, no "generated with" lines, no session links.
  Commit messages and pull request descriptions describe the change and nothing
  else.

## Delivery

Work on branches named `bsneed/<description>`, starting from the branch that
holds this plan. Deliver as a stack of pull requests, each of which passes the
full gate on its own:

1. **The new surface.** The trait, `Items`, `Error`, MemoryStore and WebStore
   changed to it, and DirectoryStore, `Envelope`, `end_batch`, `DataResult` and
   `Equivalent` removed.
2. **FileStore.**
3. **Everything else.** Examples, README, `chrono`, CI.

Different slicing is fine if every pull request in the stack still passes the
gate and none mixes unrelated work.

**The gate** is `./test_all.sh` with no arguments (native tests and the WASM
tests in headless Chrome), `cargo fmt --check`, and every check
`.github/workflows/ci.yml` runs, all passing, with no test deleted, skipped or
weakened to get there. Tests of removed features are removed with the feature.

**Do:** create branches, commit, push, open pull requests, and wait for CI on
them.

**Do not:** merge anything, push to `main`, change the version, run the release
tooling, or publish.

## Questions for review

Nobody is available to answer questions while this work runs. A question does
not stop the work:

1. Decide it the way the rest of the design would, and carry on.
2. Record the question, the choice made and the reason in the pull request
   description of the change it affects, under a heading "Questions for review".

Stop only for something that cannot be undone or that this plan forbids. Then
finish everything that does not depend on it and report what is left.

## Done

- Every pull request in the stack is open, passes the gate locally, and is green
  in CI.
- The last one's description says what was built, how it was verified, and lists
  every question for review in the stack.
- Nothing is merged.
