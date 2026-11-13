# WebStore across tabs

Status: agreed with Brandon 2026-11-13. Builds on 0.4.0 (`main` at 16df861).

## Problem

Every tab of a page that uses a `WebStore` opens the same IndexedDB database (same
origin, same `database_name`). In 0.4.0 each instance hydrates every stored record and
assigns its own sequence numbers, so:

- every open tab fetches and sends the same stored events (duplicates);
- two tabs can give out the same sequence number, so one tab's `add` fails on a key
  collision (logged, the item stays memory-only) and one tab's `remove` can delete
  another tab's record.

## Behavior wanted

1. Each tab collects its own events and sends its own events. Live tabs do not
   coordinate and never see each other's events.
2. Events left behind by a tab that no longer exists (closed, crashed, discarded,
   reloaded) are **orphans**. A live tab adopts them and sends them as its own.
3. Orphans are adopted when a store opens and then every `orphan_interval`.
4. `max_items` is per tab. Adopted orphans count toward the adopting tab's cap.

Out of scope: workers and service workers, cross-origin sharing, any global order of
events across tabs.

## Design

### Tab identity

- `WebStore::new` generates a tab id: `crypto.randomUUID()` (fall back to 16 bytes from
  `crypto.getRandomValues` as hex). A new id per store instance, so a page reload is a
  new tab and adopts the previous page's events. (Keeping the id across reloads via
  `sessionStorage` is a possible later refinement, not part of this change.)
- Every record carries its tab id.

### Liveness lock

- Each store holds a Web Lock named `transientdb:{database_name}:tab:{tab_id}` for its
  whole life: requested in `new` with a callback that returns a promise that never
  resolves (or resolves when the store is dropped). `new` waits until the lock is granted
  before it writes anything, so a tab's records never exist without its lock held.
- The browser releases the lock when the page goes away, so "the owner of tab id T is
  dead" is exactly "the lock for T can be acquired".
- Web Locks need a secure context (https or localhost) and a reasonably recent browser.
  If `navigator.locks` is unavailable the store runs memory-only (warns once, and
  `persistence_state()` reports `MemoryOnly`): writing records that no tab could ever
  safely adopt would only leak storage.

### Records and keys

- Object store keyPath becomes the compound `["tab", "seq"]`, with an index on `tab`.
  `seq` stays the store's own per-instance sequence number (the queue's), so it is
  unique within a tab and the compound key is unique across tabs. Record:
  `{ tab, seq, text }`.
- Bump `DB_VERSION`; `create_schema` already discards old stores and records.

### Opening

- `new` no longer hydrates every record. A new tab owns nothing when it opens. Opening is:
  connect, take the liveness lock, then run one adoption pass (below) before returning.
- The checks hydrate does today (record shape, text over `max_item_size`, more than
  `max_items`) apply to adopted records instead, with the same warnings and deletes.

### Adoption pass

1. List the distinct tab ids that have records (key cursor on the `tab` index, `nextunique`).
2. For each id other than our own, request its liveness lock with `ifAvailable: true`.
   Not granted: the owner is alive (or another tab is adopting it right now); skip it.
3. Granted: while holding that lock, in one readwrite transaction, read that tab's records
   in `seq` order, delete them, and add each valid one back under our tab with a new
   `seq` from our queue. Restore them into our in-memory queue in that order. Then release
   the lock.
4. After all ids, trim to `max_items` (drop oldest, delete from IndexedDB, warn with the count).

Adopted items get new sequence numbers, so they queue **behind** the tab's own items
and are not the first dropped at the cap. (Open point below.)

The lock held in step 3 is what makes concurrent adoption safe: two tabs racing for the
same orphan cannot both be granted its lock.

### Interval

- `WebConfig` gains a required `orphan_interval: std::time::Duration`. The doc comment
  suggests `Duration::from_secs(30)` and says adoption also runs once when the store opens.
  `new` panics if it is zero, like `max_items = 0`.
- The store schedules adoption itself (e.g. `setInterval`) for as long as it lives and
  cancels it on drop. A pass must not run concurrently with itself, and must not hold any
  borrow of the store's state across an `await` (the sync `DataStore` calls can run between
  its steps).
- In memory-only mode there is no adoption.

### Everything else is unchanged

- `append`, `fetch`, `remove`, `has_data` behave as in 0.4.0, on this tab's items only.
  The claim ledger, `Items`, release-on-drop and sync-issued IndexedDB writes stay as they
  are: only the owning tab ever has an item in flight.
- `remove` deletes by `[tab, seq]`.
- `reset` clears this tab's queue and **every** record in the database (all tabs), as
  0.4.0's `clear` does, for logout or opt-out. Other live tabs keep what they already hold
  in memory. (Open point below.)

## Flushing on tab close (consumer contract, document on `WebStore`)

A send started while the page is closing cannot report back, so the store cannot learn
it succeeded. Agreed contract for the last batch:

```rust
let body = items.json();
store.remove(items)?;   // the IndexedDB delete is issued during this call
send_beacon(&body);
```

If the beacon fails, that batch is lost (at most once, last batch only). If the delete
does not commit during unload, the batch is adopted later and sent again. Recommend the
main flush on `visibilitychange` to `hidden`, while the page is still alive.

## Tests (wasm, real Chrome and Firefox)

- Two instances, one database: no shared fetch, no key collision on concurrent appends,
  each sees only its own items.
- Orphans: instance A appends and is dropped (lock released); B adopts at its next pass;
  A's records are gone and re-keyed under B; order within A is preserved.
- Live owner: B does not adopt from A while A holds its lock.
- Racing adopters: B and C adopt concurrently; every orphan ends up in exactly one of them.
- Cap: adoption past `max_items` trims oldest and deletes them from IndexedDB.
- Interval: adoption runs again after `orphan_interval`; stops after drop.
- Invalid and oversized adopted records are discarded and deleted, with warnings.
- Upgrade from the 0.4.0 record format discards old records.
- No `navigator.locks`: memory-only with a warning.
- Close-flush sequence: `json()`, then `remove`, then drop the store; another instance
  never adopts the removed batch.
- All existing contract checks still pass for WebStore.

## Open points (decide before release)

- **Adopted items behind or ahead?** Behind is simplest (new seqs). Ahead would make them
  the oldest, sent first and dropped first at the cap, but needs the queue to accept items
  in front.
- **`reset` scope.** All tabs' records (as above) or this tab's only.
- **bfcache.** Some browsers keep pages that hold Web Locks out of the back/forward cache.
  Verify in Chrome, Firefox and Safari what the liveness lock costs a host page's bfcache
  eligibility before shipping. (Measured, and handled: see "bfcache, measured" and
  "bfcache restore, as implemented" below.)

## Implementation notes

Recorded when the spec was implemented (branch `bsneed/webstore-tabs`).

- **Open points, as implemented.** Adopted items go behind the tab's own items (new
  seqs). `reset` clears every record in the database, as 0.4.0 did.
- **Web Locks binding.** `web-sys` 0.3 gates `LockManager` behind
  `--cfg web_sys_unstable_apis`, which every dependent crate would have to set, so
  `src/web/lock.rs` binds `navigator.locks.request` directly with `wasm-bindgen`.
- **Adoption transaction.** One tab's records are read (`getAll` and `getAllKeys` on the
  `tab` index) and rewritten inside the keys request's success handler, so the
  transaction never depends on promise microtasks staying inside it. The handler only
  reserves the items' sequence numbers; the items are queued in memory, in sequence order,
  once the transaction has committed. Until then nothing can fetch, remove or drop them,
  so if it aborts (a failed add, or a commit-time failure such as a full disk), nothing
  was handed out, the records are still the orphan's, and a later pass adopts them once.
  A reset or restore before the commit queues nothing.
- **Reset during a pass.** A reset issued after a move's transaction was created, but
  before its reads return, makes the move adopt nothing; the reset's clear, ordered after
  it, deletes the records.
- **Over the cap mid-pass.** Between a move and the trim that ends the pass, the queue can
  hold more than `max_items`; `Queue::push` therefore reports every item it drops, so an
  append in that window deletes all of them from IndexedDB.
- **`max_items` is a soft limit** (agreed with Brandon 2026-11-13). The store trims to
  it, but can hold more for a while: during a pass, as above, and after a pass cut short
  because the connection closed for another page's upgrade, which skips the trim. The
  next append then drops the excess, in memory only, without the trim's warning.
- **Delivery is at least once.** Live tabs never send each other's items, but a send in
  flight when a page is cached or dies can repeat once its items are adopted; the server
  dedupes, by an id each item carries (Segment's `messageId`).
- **Records under a non-string tab key** (only another writer could store one) are
  discarded without a lock: no store's lock guards them, and deleting twice is harmless.
- **Sequence numbers exhausted while adopting**: the rest of that orphan is left in place
  for another tab.
- **Missing `navigator.locks`** warns once per store opened (not once per page).

### bfcache, measured

Pages served from `localhost`, driven by WebDriver (headless): load page A, navigate to
B, `history.back()`, and record `pageshow.persisted` (and Chrome's `notRestoredReasons`).
An `unload` listener is the negative control. No web sources could be consulted when this
was written, so these are measurements only.

| Page A holds                          | Chrome 157       | Firefox 160      |
|---------------------------------------|------------------|------------------|
| nothing                               | restored         | restored         |
| an `unload` listener (control)        | not restored     | not restored     |
| a Web Lock                            | restored         | **not restored** |
| an idle IndexedDB connection          | restored         | restored         |
| a Web Lock and an IndexedDB connection| restored         | **not restored** |
| a Web Lock that B requests (`ifAvailable`) | **evicted** (`weblocks-contention`); B granted | not restored; B granted |

So in Firefox every page with a persisted `WebStore` loses the back/forward cache. In
Chrome it keeps it unless another page of the origin requests its lock, which an adoption
pass does only when the cached page has items stored.

Safari 26.2, measured 2026-11-13 the same way (WebDriver, `localhost`): it caches a page
that holds a Web Lock, but **releases the page's locks when it enters the cache**. Another
page's `ifAvailable` request for the lock is granted while the page is cached, and after
the restore the lock is still gone (`navigator.locks.query()` shows nothing held), though
the promise the page's lock callback returned never resolved. Nothing tells the page.
iOS browsers are all WebKit, so presumably the same. With the store as first implemented,
a restored page went on writing under its old tab id without its lock, while other tabs
adopted those records and the page also sent them: ongoing duplicates.

Chrome 157, for comparison: a cached page keeps its lock; a contending `ifAvailable`
request is denied and the cached page evicted, and a restored page still holds its lock.
Firefox 160 never caches a page holding a Web Lock (`pagehide` with `persisted` false).

### bfcache restore, as implemented

Agreed with Brandon 2026-11-13: a page restored from the back/forward cache starts over
as a new tab, through the same steps as `new`. Nothing is done on `pagehide`.

- A store opened persisted listens for `pageshow` on the window and stops on drop; a
  memory-only store does not listen. On `pageshow` with `persisted` true
  (`src/web/pageshow.rs`): it releases its old liveness lock if it still holds it (Chrome
  keeps it; in Safari releasing it again is harmless), takes a new tab id and requests its
  lock, and resets its queue in memory only (the `Items` epoch, as `reset` does, without
  clearing IndexedDB). Once the new lock is granted, it writes what was appended meanwhile
  under the new tab and runs an adoption pass, which adopts its own old records unless
  another tab took them while it was cached.
- Nothing is written under a tab id whose lock the store does not hold:
  `Shared::liveness` is `None` from the restore until the grant, and appends, deletes and
  adoption passes write only while it is `Some`. A grant that arrives after the store was
  dropped, or after it started over again, is released at once.
- `Shared::generation` (formerly `resets`) counts resets and restores. A move whose reads
  return in a later generation adopts nothing, and a pass stops once the generation
  changes. A pass interrupted by a restore starts again for the new tab, once its lock is
  held; the new tab's own pass does nothing while it runs.
- A store whose connection has closed (another page's upgrade) ignores a restore: it
  holds its items in memory only, and starting over would lose them.
- If the new lock's request fails, the store closes its connection and goes memory-only,
  with a warning, as `new` would.
- Items appended between the restore and the grant queue ahead of the page's re-adopted
  old items: adopted items go behind, as always.
- Tests drive the listener with synthetic `PageTransitionEvent`s in Chrome and Firefox,
  and simulate Safari by releasing a store's lock behind its back (`Held::lose`). Safari
  itself is verified separately.
