# FileStore Specification

**Status:** Implemented in 0.4.0
**Date:** 2026-11-10
**Supersedes:** `LEDGER_SPEC.md` and `STORAGE_PROPOSAL.md` (March 2026)

Unsettled points are collected under [Open questions](#open-questions).

This is a store for transient data. Where keeping queued items would cost
complexity, the store discards them and logs it.

## Summary

FileStore is a disk-backed queue that keeps every item in one file. The file is
a header followed by slots. Every slot is the same size, so any free slot holds
any item. A removed item's slot is reused by a later append, and the file grows
by one slot only when no slot is free. Nothing in the file is ever moved,
rewritten, finished or renamed, so there is nothing to recover or compact.

It replaces DirectoryStore. It stores items and returns items. It knows nothing
about how the caller sends them: there is no envelope, no wire format and no
batch file.

## Goals

1. One file per store.
2. Append, fetch and remove only. Removal in any order.
3. Safe by construction when the process is killed at any point: no recovery pass.
4. Bounded memory: a fetch reads only as many items as the caller asks for.
5. Disk use that follows the queue: the file is as large as the queue has been
   deep, not as large as it is allowed to get.
6. No new runtime, no async, and no new dependency of any kind.

## Non-goals

- Not a general-purpose database. No queries, updates, priorities or retry counts.
- No envelope, serialization of a request body, or streaming to the network.
  Those belong to the caller.
- Not for `wasm32`; browsers keep WebStore.
- No migration from DirectoryStore files.
- No compaction. The file gets smaller only when the store is empty (see
  [Shrinking](#shrinking)).
- Durability across power loss is not guaranteed (see [Durability](#durability)).

## Configuration

```rust
pub struct FileConfig {
    /// The store's file. Its parent directory is created if missing.
    pub path: PathBuf,
    /// Whose data this is: a write key, for example. It is recorded in the
    /// file, and a file recorded under a different id is discarded at open.
    pub id: String,
    /// Largest serialized item, in bytes. This is the slot size, and it is
    /// part of the file's layout.
    pub max_item_size: usize,
    /// The most items the store holds at once. Appending past it drops the
    /// oldest item. Not part of the file's layout: it can change between runs.
    pub max_items: usize,
}
```

The file is never larger than:

```
256 + (32 + max_item_size) * max_items
```

Example: `max_item_size = 8192`, `max_items = 2560` allows up to 21,053,696
bytes. A queue that has never been more than 50 items deep is a 411,456 byte
file.

`new` panics, in the manner of the other stores, if `max_item_size` or `max_items`
is 0, if `max_item_size` is larger than `u32::MAX`, or if `id` is empty or longer
than 224 bytes.

The id says whose data the file holds; the path says where it is. Opening the
same path with a new id is how a caller says "what was queued here is no longer
mine to send": the store discards it at once. The store never looks at any
other file.

## File layout

All integers are little-endian.

```
offset 0                      header    256 bytes
offset 256                    slot 0    32 + slot_size bytes
offset 256 + (32 + slot_size) slot 1
...
```

Slot `i` starts at `256 + (32 + slot_size) * i`: a 32 byte entry, then
`slot_size` bytes for the payload. The number of slots is not recorded. It is
whatever the file's length holds:

```
slots = (file length - 256) / (32 + slot_size)      rounded down
```

Bytes past the last whole slot are not a slot. They are overwritten when the
file next grows.

### Header (256 bytes)

| Bytes | Field | Notes |
|---|---|---|
| 0..8 | magic | `TDBFILE\0` |
| 8..12 | format version | `1` |
| 12..16 | slot size | bytes |
| 16..20 | reserved | zero |
| 20..24 | id length | bytes, `1..=224` |
| 24..248 | id | the config's `id` as UTF-8, zero padded |
| 248..252 | reserved | zero |
| 252..256 | CRC-32 | of bytes 0..252 |

The id is stored as given, not hashed, so two ids can never be mistaken for one
another and the file says in plain text whose it is.

The header is written when the file is created and never modified.

### Slot entry (32 bytes)

| Bytes | Field | Notes |
|---|---|---|
| 0 | state | `0` free, `1` live |
| 1..4 | reserved | zero |
| 4..8 | length | payload bytes, `1..=slot_size` |
| 8..16 | sequence | `u64`, unique per item, starts at 1, never reused |
| 16..20 | CRC-32 | of sequence, length, then the payload bytes |
| 20..32 | reserved | zero |

The CRC covers the entry's own fields as well as the payload, so an entry and a
payload that do not belong together never validate. The state byte is outside the
CRC so that removal is a single one-byte write.

A slot's position says nothing about its age. Order is by sequence number only.

Both CRCs are CRC-32 as used by zlib and PNG (polynomial `0xEDB88320`,
reflected). It is implemented in the crate as a table lookup, about twenty
lines, and tested against the standard check value: the CRC of the ASCII text
`123456789` is `0xCBF43926`.

## Item states (internal)

The store keeps each item in one of three states. They are how the store keeps
its own books. Nothing in the API names them, and a caller never has to think
about them: it fetches, then removes or releases what it fetched.

| State | Meaning | Leaves it by |
|---|---|---|
| Available | Stored, and nobody is working on it. | `fetch` (to pending), being dropped when the store is full |
| Pending | Handed out by a `fetch` that has not been resolved. | `remove` (gone), `release` (back to available) |
| Gone | Removed or dropped. Its slot is free. | |

Only available items are returned by `fetch`. What the caller sees of this is
one plain fact: two fetches never hand it the same item, so several requests
can be in flight without it tracking what it already has.

Pending is kept in memory only. On disk a slot is free or live, nothing else:
fetching and releasing write nothing. A process cannot have a request in flight
across a restart, so every live item is available again at open, and a kill
while items are pending needs no handling.

## Operations

### Create

The file is created with `create_new` and its header is written. That is all:
there are no slots yet, and nothing is synced or renamed.

### Open

1. Take the path (see [One store per file](#one-store-per-file)).
2. Read and check the header. If it is not exactly the header this build would
   write for this config, the file is deleted, logged, and created again (see
   below).
3. Work out the number of slots from the file's length and read each slot's
   entry.
4. Any entry whose state is not `0` or `1`, or that is live with a length outside
   `1..=slot_size`, is treated as free and its state byte cleared. So is a live
   entry whose sequence is already live in a lower slot.
5. `next sequence` = highest sequence in any entry that is free or live with a
   length in `1..=slot_size`, + 1. The sequence of any other entry is as damaged
   as the rest of it and does not count.
6. If more items are live than `max_items` allows (the limit was lowered), the
   oldest are dropped until they fit, and that is logged.
7. If no item is live and the file holds any slot, the file is cut back to its
   header (see [Shrinking](#shrinking)).

No payload is read at open. Every entry is read before anything is written, so
an error reading one leaves the file as it was. The writes of steps 4, 6 and 7
are logged if they fail, not returned: the slots they concern are free either
way, and the next open sees the same.

Sequence numbers never wrap or repeat. If the entries leave none to hand out
(only a damaged file can), every `append` fails with `Io`, writing nothing,
until the store is reset or a store opens the file with no live item. Either
way no entry is left and nothing handed out can match a new item, so sequences
start again from 1.

#### A file that does not match

An existing file is discarded, with every item in it, when any of these differs
from what the store would create:

- its length, the magic or the header CRC (the file is damaged, cut short, or
  is not a FileStore file),
- the format version, older or newer,
- the id,
- the slot size (`max_item_size`),
- any reserved byte, or any byte of the id's padding, that is not zero.

The store logs one line naming the file and the reason, deletes it, and creates
a fresh one. `new` does not fail for this. There is no migration and no attempt
to read the old items.

`max_items` is not on that list. Raising it lets the file grow further; lowering
it is handled by step 6 above. Neither discards the file.

### append

1. Check the item and serialize it compactly, as every store does: nested too
   deeply, `ItemTooDeep`; longer than `max_item_size`, `ItemTooLarge`. Nothing
   is written. This comes before the file is touched, so a bad item gets the
   same error while the store cannot use its file (see
   [One store per file](#one-store-per-file)), and a deep one is taken apart
   without recursion.
2. Choose a slot:
   - if `max_items` items are already live, the oldest item's slot (see
     [When the store is full](#when-the-store-is-full));
   - otherwise the free slot with the lowest index;
   - otherwise a new slot at the end of the file.
3. Write the payload into the slot. A new slot's payload is written
   zero-padded to the full slot size.
4. Write the slot's entry with state `1`. **This write is the commit point.**

If step 3 fails, the entry was never touched and the slot is still free. If step 4
fails, the store writes state `0` and returns the error. Nothing is rolled back
because nothing else changed. When the slot was the oldest item's, that item is
gone either way.

Growing needs no separate step. The padded payload write extends the file by one
whole slot, so the file always ends on a slot boundary and the new slot is
counted at the next open, however short the item. Until step 4 the slot's entry
reads as zeros, that is, as free.

Using the lowest free slot first keeps the file as short as the queue has been
deep.

`max_item_size` is the only size the store enforces. An item that must fit a
request body of some size is the caller's to check.

### fetch(count: usize, max_bytes: usize)

Returns the oldest available items, in sequence order, and marks them pending:

- at most `count` items,
- whose serialized lengths total at most `max_bytes`.

Both limits are required. There is no "no limit": the store has no batch size
of its own to fall back on, and an unbounded fetch could read the whole file
into memory. The caller states how much it wants every time.

The result is `Result<Option<Items>>`:

- `Ok(Some(items))`: at least one item. An `Items` is never empty.
- `Ok(None)`: the store was read without trouble and there is nothing to hand
  over. Nothing is available, `count` is 0, or `max_bytes` is smaller than the
  oldest available item. This is not an error.
- `Err(_)`: the store could not be read.

The store does not group items. Which items may share a request is a rule of
the caller's protocol.

`max_bytes` counts item bytes only. Whatever the caller wraps around or puts
between the items is the caller's to account for.

Fetching removes nothing from disk. The items stay stored, but are not returned
by another fetch until they are released. This replaces the 0.3.0 contract,
under which a later fetch returned the same items again.

Each item is checked as it is read: its CRC, that it is valid UTF-8, and that it
is well-formed JSON. An item that fails any of these is freed, logged and
skipped, so a damaged item cannot be fetched forever. This is the one place a
bad item is caught. Everything in an `Items` is known to be good, which is why
none of its methods can fail.

Fetch returns `Items`: the fetched items, in order, and the store's claim on
them, in one value.

```rust
pub struct Items { /* private: issuing store, and each item's sequence and text */ }

impl Items {
    /// How many items. Never 0.
    pub fn len(&self) -> usize;
    /// Total bytes of the items' text, as counted against `max_bytes`.
    pub fn byte_len(&self) -> usize;

    // The items, in whichever form the caller wants them. Oldest first.

    /// One JSON array holding every item: `[{...},{...}]`.
    /// Built by joining the stored text; nothing is parsed.
    pub fn json(&self) -> String;
    /// Every item parsed. Cannot fail: fetch already checked each one.
    pub fn values(&self) -> Vec<serde_json::Value>;
    /// Each item's own JSON text, exactly as it was serialized at append.
    pub fn iter(&self) -> impl Iterator<Item = &str>;

    /// Gives the items back: they will be returned by a later fetch.
    pub fn release(self);
}
```

An `Items` holds each item as the JSON text that was stored. The three ways of
reading it are views of that one text, so asking for one costs nothing for the
others, and more can be added later without changing what is stored.

- `json()` is for a caller that drops the items into a request body:
  `{"events":` + `items.json()` + `}`. Its length is `byte_len()`, plus one
  comma between each pair of items, plus 2.
- `values()` is for a caller that needs to look inside the items.
- `iter()` is for a caller that formats them some other way: one per line, say.

None of these wraps the items in anything. A key, an object around the array,
a timestamp: those are the caller's.

There is no separate list of removables and no `DataResult`. The only way to get
an `Items` is to fetch one, and it cannot be divided: it is removed whole or
released whole.

Every `Items` ends one of two ways: it is passed to `remove`, or it is released.
Dropping an `Items` releases it, so an early return or a panic on the send path
cannot leave items pending forever. `release()` is the explicit spelling of the
same thing.

### remove(items: Items)

Takes an `Items` back and removes what is in it. If it was issued by this store
instance, then for each item: if an entry is live with its sequence, write state
`0` and clear its pending mark. Anything else is ignored and is not an error: an
item already dropped, or another store's `Items`, which is released back to that
store. Every item is tried. A failed write is logged, and that item stays stored
and becomes available again; once every item has been tried, `remove` returns
the first failure as `Io`.

`remove` consumes the `Items`, so the same fetch cannot be removed twice.

If the removal leaves no live item, the file is cut back to its header (see
[Shrinking](#shrinking)).

A caller that sends several requests at once calls `fetch` once per request.
Each fetch returns different items. When a request succeeds its `Items` is
passed to `remove`; when it fails its `Items` is released and those items are
returned by a later fetch.

### end_batch

There is none. FileStore has no batch boundaries.

### reset

Cuts the file back to its header, which removes every slot at once, and clears
the in-memory state, pending marks included. An `Items` fetched before the reset
matches nothing afterwards: removing or releasing it does nothing. If the cut
fails, that is logged and nothing changes. A store that cannot use its file does
nothing.

### has_data

True if any item is available, that is, if a fetch could return something.
Pending items do not count. No payload is read for it, so a damaged item counts
until a fetch finds it and frees it.

## Errors

Every fallible call returns the crate's own error, so a caller can tell the
cases apart and act on each, instead of inspecting an `io::ErrorKind`.

```rust
#[non_exhaustive]
pub enum Error {
    /// `append`: the serialized item is larger than `max_item_size`.
    ItemTooLarge { size: usize, max: usize },
    /// `append`: the item is nested too deeply to be read back.
    ItemTooDeep,
    /// `append`, `fetch`, `remove`: another store in this process is using
    /// this file, so this one did nothing.
    InUse,
    /// Anything the filesystem reported.
    Io(std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
```

What a caller does with each: `ItemTooLarge` and `ItemTooDeep` mean this item
will never be stored, so drop it or report it. `InUse` means two stores were
made for one file, which is a mistake in the caller's setup: use the other one.
`Io` is the only one worth retrying, unless sequence numbers are exhausted (see
[Open](#open)).

Only the store's own operations return a `Result`. Nothing on `Items` does.

Things the store handles itself are not errors: a file that does not match, a
damaged item, a full store. Those are logged.

## When the store is full

When `max_items` items are live, `append` drops the oldest item: the live item
with the lowest sequence is overwritten by the new one. This is not configurable,
and `append` never fails for reaching the limit.

The drop is the same two writes as any append, into the oldest item's slot. If
the process is killed between them, the old entry no longer matches the new
payload, fails its CRC and is freed at the next fetch: the old item is gone and
the new one was never acknowledged.

The oldest item is dropped whether it is available or pending. A pending item
has already been handed to the caller, which may still send it. When its `Items`
is removed or released later, that item matches nothing and is ignored.

An append that has to grow the file can still fail, if the disk is full. That is
an I/O error from `append`, and nothing is dropped for it.

## Shrinking

The file grows as the queue deepens and is cut back in exactly one case: when no
item is live. Then every slot is free, so the file is truncated to its 256 byte
header. That happens after a `remove`, a failed append or a freed damaged item
takes the last live item, on `reset`, and at open when no item is live. A
pending item is still live, so nothing is cut while a request is in flight.

This is one `set_len` call. It moves nothing, and whether or not it completes,
the file is valid: before it, every slot is free; after it, there are none. If
it fails, that is logged and the file stays as it was.

A queue that drains, which is the normal case after every flush, therefore gives
all its disk back. A queue that never fully drains keeps the file at its
deepest; that is the deferred work below.

## One store per file

A file is used by one store at a time. The store does not try to make two
stores on one file work together. It makes the second one do nothing, and say so.

**Within a process.** The crate keeps a process-wide set of the canonical paths
in use, so a relative path, one through `..` and one through a symbolic link all
name the same store. A hard link is a different path to the same file, and is
not detected. A store that finds its path free takes it, opens the file as
described under Open, and holds the path until it is dropped. A store that finds
its path taken is still created, and `new` succeeds, but it does not touch the file: it
does not open it, check its header, or discard it for not matching. On that
store:

- `append`, `fetch` and `remove` return `Error::InUse` and do nothing,
- `has_data` is false,
- `reset` does nothing.

Each of those calls first looks again. If the path has since been given up, the
store takes it, opens the file, and carries on as normal from then on.

**Across processes it is a rule for the caller, not enforced.** An app and its
extension, say, must not open the same file. If they do, each works from its own
idea of which slots are free, so items can be overwritten or sent twice. No
malformed item is ever handed out, because fetch checks every item it returns,
but items can be lost or duplicated.

No OS file lock is taken. Three reasons, the last two checked on 2026-11-10:

- A lock would only turn a caller's mistake into an error; it would not make
  sharing work.
- iOS terminates an app that is suspended while holding a file lock on a file in
  a shared app-group container (termination code `0xdead10cc`).
- `std::fs::File::try_lock` is stable from Rust 1.89, but was reported in May
  2026 as not implemented on Android, so using it would mean a new dependency or
  platform-specific code.

Nothing about locking is written to the file either: a lock byte would survive
a kill and have to be guessed about on the next launch.

## Suspension and backgrounding

A mobile OS can freeze the process at any instruction and later kill it without
resuming it. To the file that is a kill at that instruction, which the layout
already covers. The store has no lifecycle hooks and does not close or reopen
the file around backgrounding. Four rules keep that true:

- **No buffering in user space.** Every write goes straight to the OS. When
  `append` returns `Ok`, the bytes are with the kernel and outlive the process.
  A buffered writer would hold accepted items in memory that a kill discards.
- **No lock is held**, so the termination described above does not apply.
- **An I/O error changes nothing.** If the OS refuses a read or a write, the call
  returns `Error::Io` and the store is as it was, ready for the next call. The
  one exception is an append into a full store, which may have dropped the
  oldest item. This matters where the file's data-protection class makes it
  unreadable while the device is locked: calls fail for that time and work again
  after.
- **Nothing is cached that the file could contradict** except which slots are
  live, and only this process changes that.

Where the file lives is the caller's choice, and it matters:

- Not a temporary or cache directory. The OS may delete those while the app is
  suspended, and writes to a deleted file succeed and go nowhere.
- Preferably the app's own container. Whether iOS objects to a plain open
  handle, without a lock, on a file in a shared app-group container during
  suspension is not verified. Apple's documentation names locks; at least one
  third-party write-up says open handles too.
- With a data-protection class that allows access whenever the app can run.

## Durability

Nothing is ever synced.

| Event | Result |
|---|---|
| Killed during a payload write | Entry never written; slot is free. The item was never acknowledged. |
| Killed during an entry write | Entry is torn; state is `0`, or the CRC fails and the item is freed at fetch. |
| Killed while the file is growing | The file ends in part of a slot. Those bytes are not a slot and are overwritten later. |
| Killed during remove | The state byte is `0` or `1`. At worst the item is sent again. |
| Killed while items are pending | Nothing was written for pending. They are available at the next open. |
| Killed during reset, or while shrinking | The file was cut back or it was not. Either is a valid file. |
| Killed during create | The header is short or fails its CRC. The file does not match and is created again. |
| Power loss | Recent appends and removes may be lost. An entry without its payload fails its CRC and is freed; a payload without its entry is invisible. |
| Damage to one slot | That item is lost. No other item is affected. |

A plain process kill loses nothing that `append` returned `Ok` for.

## Relationship to the other stores

MemoryStore and WebStore have the same surface as FileStore: the same
`DataStore` trait, the same two limits, and the same `Items`, `Error` and
`Result`. [`PLAN_0_4_0.md`](PLAN_0_4_0.md) describes each of them.

## Testing

The tests cover, in addition to the usual behavior tests:

- Fault injection on every read and write (header, payload, entry, state byte,
  cut, the file's length, entries, items, open, create, delete), through
  `src/file/faults.rs`.
- A kill at every byte boundary of an append, simulated by truncating the write,
  both into an existing slot and into a new one at the end of the file.
- Slot reuse: removal in every order, then appends, checking that the lowest free
  slot is used, that the file does not grow while a slot is free, and fetch order
  by sequence.
- A full store: drop order, a kill between the two writes of a drop, and
  removing an item that was dropped after it was fetched.
- `max_items` raised and lowered between runs, with items stored.
- Shrinking: the file is exactly its header after the last live item is removed,
  dropped or freed and after reset; it is not cut while any item is live or
  pending; the next append grows it again from slot 0; a failed cut changes
  nothing.
- Several fetches removed in every order; removing another store's `Items`.
- Pending: consecutive fetches return disjoint items; release, drop, and a panic
  while holding an `Items` each make them available again, in sequence order;
  nothing is written for any of it; pending does not survive reopening.
- A file that does not match, once for each reason listed under Open, and a file
  of every length from 0 to two whole slots.
- Two stores on one path in one process, including by a relative and an absolute
  spelling of it: the second is created, returns `InUse` from `append`, `fetch`
  and `remove`, reports no data, and leaves the file untouched even when its
  config would not match; after the first is dropped, its next call takes the
  file and works.
- An I/O error on every read and write leaves the store usable, with the same
  items, for the next call.
- Two stores on one file without the registry (as two processes would be),
  interleaving appends, fetches and removes: every fetched item is a whole,
  valid item that was appended by one of them.
- CRC-32 against the standard check value.

## Open questions

None at present.

Deferred, not open: cutting free slots off the end of a file that still holds
items, and any compaction beyond that.
