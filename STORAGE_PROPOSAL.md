# Storage Layer Proposal: LedgerStore

**Author:** Brandon Sneed
**Date:** March 2026
**Status:** Proposal
**Audience:** Segment Libraries Team

---

## TL;DR

Our current file-based storage (`DirectoryStore`) has a design flaw that makes it
vulnerable to data corruption on app crash. A customer has already hit this in
production. The fix we shipped was a band-aid. This proposal introduces `LedgerStore`,
a new storage backend where the file format itself prevents corruption, rather than
relying on a fragile sequence of operations to complete successfully.

---

## Background

Our analytics SDKs buffer events on disk before sending them to the server in batches.
The component responsible for this is `DirectoryStore`, part of the `TransientDB`
library. It writes events to JSON files, and when it's time to send, it reads those
files and uploads them.

### Why DirectoryStore was designed this way

The original design was driven by a genuinely good insight: the file on disk should
be the exact payload you send over the wire. Our servers accept JSON batches, so
DirectoryStore writes JSON batch files. When it's time to send, there's no
transformation. You just open the file and stream it to the server. One file, one
HTTP request, zero processing.

This had real advantages. The transport layer can call `post_file(path)` and stream
the file bytes directly to the socket. No serialization at send time, no memory
allocation, no assembling a batch from pieces. An engineer debugging a stuck queue
can open any file, see exactly what events are in it, and know precisely what the
server would receive. No special tooling needed. And the mental model is dead simple:
one file = one batch = one HTTP request. What's on disk is what goes on the wire.

These are genuinely good properties, and for the common case (app runs normally, events
flow through, files get sent) the design works well. The problem only shows up when
the uncommon case happens. And on mobile, the uncommon case isn't uncommon at all.

### How it works today

1. **Open a file** and write the JSON preamble: `{ "batch": [`
2. **Append events** as comma-separated JSON objects
3. **Close the file** by writing the footer: `], "sentAt": "...", "writeKey": "..." }`
4. **Rename the file** to `.temp` to signal "this file is ready to send"

The tradeoff we didn't fully appreciate: making the file format match the wire format
means the file has structural requirements (an opening bracket, a closing bracket,
a rename) that create a window of vulnerability. The file is invalid JSON until step
3 completes. If anything interrupts this sequence, the file is corrupt. On mobile,
apps get killed by the OS constantly: backgrounding, memory pressure, thermal limits,
user force-quits. What's "uncommon" on a server is routine on a phone.

---

## The problem

### The customer incident

In early March 2026, a customer on a significant contract reported receiving corrupted
JSON payloads from our iOS SDK. The server was rejecting batches because the JSON was
structurally invalid.

### Root cause investigation

We initially traced the corruption to a race condition in the async/sync queue
interaction, where `fetch()` could execute before in-flight appends completed. We
shipped a targeted fix for the race condition, and later added an `isFinalized()`
check to prevent appending to already-closed files.

The customer came back and reported that the issue persists.

Further investigation revealed that this customer is running multiple analytics
instances configured with the same write key. Our SDKs are supposed to panic in this
configuration to prevent exactly this kind of corruption, but that guard was
inadvertently disabled in a recent release. The result: multiple instances writing to
the same directory, racing to append events, finalize files, and rename them.

This confirms what we suspected during the initial investigation: the file format is
inherently fragile. The race condition was one trigger. Multiple instances are another.
Any time the multi-step file lifecycle gets interrupted or contested by another writer,
corruption is the result.

### Failure points in DirectoryStore

Here are the specific scenarios where DirectoryStore produces corrupt files:

**1. Crash after writing events, before writing the footer**

The file contains `{ "batch": [{"event":"a"},{"event":"b"}`, valid event data, but
the JSON structure is never closed. On next launch, the recovery code tries to finalize
it by appending the footer, but it doesn't know whether the last event was fully
written. If it wasn't, you get malformed JSON.

**2. Crash after writing the footer, before the rename**

The file has valid, complete JSON. But it doesn't have the `.temp` extension yet, so
next launch, the recovery code sees an "unfinished" file and appends a *second* footer.
Now the file has duplicate closing brackets and is invalid.

**3. Crash during append with a partially written event**

A JSON event is half-written. Recovery appends the footer after the partial data.
Result: `{ "batch": [{"event":"a"},{"even` followed by the closing structure. Corrupt.

**4. App killed while file is at the size cap**

The file hit the rotation threshold. The rename to `.temp` didn't happen before the
kill. Next launch, the file gets reopened for more writes, but it was already at (or
over) the size cap. Now we're appending to a file that should have been finalized.

**5. Multiple instances writing to the same directory**

Two analytics instances configured with the same write key share a storage directory.
Both are appending events to the same file. Both try to write the footer. Both try to
rename. The result depends on timing: duplicate footers, interleaved events, partial
overwrites. This is the scenario the customer is currently hitting. The panic guard
that was supposed to prevent it is disabled in the current release.

### Why this is a structural problem

All five scenarios share the same root cause: the file is invalid until a multi-step
ceremony completes. Writing the preamble, appending events, writing the footer, and
renaming, all four steps must succeed, in order, without interruption, and by exactly
one writer. Any failure or contention leaves the file in an ambiguous state that
recovery code has to guess about.

The fix we shipped (checking `isFinalized()` in `startFileIfNeeded()`) handles
scenario #2 specifically. Re-enabling the panic guard would address scenario #5, but
only by crashing the app. Neither fix addresses the underlying fragility. Every new
edge case we discover will need another patch.

---

## Proposed solution: LedgerStore

Instead of patching DirectoryStore, we propose a new storage backend called
`LedgerStore` that eliminates the corruption problem by design.

### Core idea: binary framed records

Instead of wrapping events in a JSON structure that requires opening and closing,
LedgerStore writes each event as an independent, self-validating binary record:

```
┌──────────────┬──────────────┬─────────────────────┐
│ payload size  │  checksum    │  event JSON bytes   │
│  (4 bytes)    │  (4 bytes)   │  (variable length)  │
└──────────────┴──────────────┴─────────────────────┘
```

Each record is self-contained. It has everything needed to validate itself (a length
and a checksum) with no dependency on previous or subsequent records. The 4-byte
length, 4-byte checksum, and payload are assembled into one buffer and written to
disk in one call. And any record can be verified independently by reading 8 bytes
of header, reading `size` bytes of payload, and checking the checksum.

The event data inside each record is the same JSON you'd put in a batch today, a
serialized event object. The binary framing is just a thin wrapper that tells us where
each record starts and ends, and whether it's intact.

### How it solves each failure point

**Crash during write?**
The partially written record doesn't need to be detected or repaired at startup. When
the file is eventually read for sending, the streaming reader validates each record as
it goes: does the header have 8 bytes? Do we have enough payload bytes? Does the
checksum match? The first record that fails any check ends the read. Everything before
it is sent normally. We lose at most one event, and the file gets deleted after a
successful send.

**No closing ceremony to fail.**
There is no preamble, no footer, no rename. The file is a sequence of records,
concatenated back-to-back. It is valid at every point in time, whether it has zero records,
after one record, after a thousand. There is nothing to "finalize."

**No rename means no rename failure.**
Files don't need a `.temp` extension to be "ready." Every file in the directory with
complete records is ready to send. The only thing that distinguishes the active file
(being written to) from finished files is that it has the highest index number, and
only within a single process lifetime. The moment the process ends, normally or via
crash, that file is finished.

This is enforced by a simple, hard rule: **on startup, always start a new file.** We
never reopen a file from a previous session. The startup sequence scans the directory
to find the highest file index, and that's it. No file contents are read, no
validation, no truncation. Every existing file is treated as finished. The next
`append()` creates a brand new file with the next index number.

But what about a file that has a partial trailing record from a crash? It doesn't need
to be fixed at startup. The streaming reader (described below) validates each record as
it reads. If it hits a partial or corrupt record at the end of a file, it simply stops
reading records and completes the batch with whatever valid records came before it. The
server gets a valid batch with N-1 events instead of N. After the server responds 200,
we delete the file, partial record and all. The corruption never needs to be "repaired"
because it never leaves the device.

Why not reopen the last file if it's under the size cap? Because that requires
answering questions: Is the file intact? Did the previous process close it cleanly?
How much space is left? Is there a partial record at the end we need to deal with
before appending? Each question is an edge case. The "always start fresh" rule
eliminates all of them. If it creates a small trailing file with only a few events,
that's fine. It gets sent and deleted on the next flush cycle just like any other file.

**Size cap is a soft boundary.**
When a file exceeds the configured size limit, we just close the handle. The next event
opens a new file. If the app gets killed before that happens, no problem. The startup
rule handles it. We were going to start a new file anyway.

**Multiple instances just work.**
Each instance creates files with exclusive-create semantics. If two instances try to
create the same file index, one succeeds and the other bumps to the next index. Within
a process, the index counter is atomic. Across processes, the filesystem handles the
collision. Each instance ends up with its own files, no interleaving, no contention.
When any instance calls `fetch()`, it picks up all files in the directory (including
those written by other instances) and sends them. Instead of panicking when someone
uses the same write key twice, it just works.

### How events get sent

The events still need to arrive at the server as JSON batches. Here's how that works
without loading entire files into memory:

When the SDK is ready to send (flush), it calls `fetch()`. This:

1. Closes the active file (so pending events are included)
2. For each finished file, constructs a `LedgerReader`, a streaming adapter that
   reads the binary records and produces a JSON batch on the fly

The LedgerReader is like a translator sitting between the file and the network. As
the HTTP client pulls bytes from it, it outputs:

```
{"batch":[ → event 1 JSON → , → event 2 JSON → , → ... → ],"sentAt":"...","writeKey":"..."}
```

It reads one record at a time from the file, strips the 8-byte header, and passes the
JSON payload straight through. Peak memory usage is one event (~1-2 KB), regardless of
how large the file is.

The JSON batch envelope (`batch`, `sentAt`, `writeKey`) is configured on the store
via callbacks, so the storage layer stays generic while still producing the exact wire
format the server expects.

After the server responds with a 200, we delete the file. If the send fails, the file
stays and we retry next cycle.

### Startup / recovery

There is no recovery step. Startup is:

1. Scan the directory for files
2. Find the highest index number
3. Set the next index to highest + 1

That's it. No reading file contents. No validating records. No truncating. No checking
for `.temp` extensions. No looking for footers. No `isFinalized()` checks. If a file
has a partial trailing record from a crash, the streaming reader will handle it at send
time. It simply stops at the bad record, sends everything before it, and the file gets
deleted after a successful 200.

### What doesn't change

The `TransientDB` API is the same. `append()`, `fetch()`, `remove()`, `has_data()`,
`reset()`, all the same methods, same patterns. Code that uses `TransientDB` today
only changes which store it creates at init time.

The wire format is the same. The server still receives JSON batches with `batch`,
`sentAt`, and `writeKey`. The transformation just happens at send time instead of
write time.

The file lifecycle is the same. Write events, flush, send, delete. Same flow,
just crash-safe now.

---

## Comparison

| Concern | DirectoryStore | LedgerStore |
|---|---|---|
| File validity | Invalid until footer is written | Always valid |
| Crash during write | Corrupt file, ambiguous recovery | Reader skips bad record at send time, everything else delivered |
| Rename required? | Yes (`.temp` signals readiness) | No, all files are always ready |
| Recovery logic | Multiple heuristics, edge cases | None. Startup scans filenames only, reader validates at send time |
| Files modified after write? | Yes (footer appended, file renamed) | Never. Append-only while active, read-only forever after |
| Memory at send time | Full file loaded as JSON | One event at a time (~2 KB) |
| Wire format coupling | Baked into file format | Configured at send time via callbacks |
| Max data loss on crash | Entire file (potentially hundreds of events) | One event |
| Multiple instances, same directory | Panic (or silent corruption if guard disabled) | Supported. Atomic file creation, no interleaving |

---

## Migration path

LedgerStore is a new store type, not a modification to DirectoryStore. Both will
coexist in the codebase during transition.

1. On startup, check for any existing DirectoryStore files (`.temp` extension)
2. Send and delete them using the current JSON format to drain the legacy queue
3. All new writes go to LedgerStore
4. Once legacy files are fully drained (typically one app session), DirectoryStore
   code becomes unused

This means zero data loss during migration. Events written under the old format
still get sent, and new events benefit from crash safety immediately.

---

## Implementation plan

LedgerStore will be implemented in both the Rust core (`twilio-data-core`) and Swift
(`analytics-swift` / `twilio-data-swift`). A detailed specification covering the binary
format, validation algorithm, and streaming reader is available in `LEDGER_SPEC.md` in
the transientdb repository.

The implementation is self-contained. It's a new file alongside the existing store
implementations, with no changes to the `TransientDB` wrapper or the `DataStore`
interface.

---

## Open questions

1. **Should we backport to Kotlin?** The Kotlin SDK uses a similar DirectoryStore
   pattern. The same vulnerability exists there.
2. **Checksum algorithm:** The spec proposes CRC-32 (same as used by gzip/PNG/Ethernet).
   Fast, well-tested, available in every language. Any concerns?
3. **Timeline:** The customer's continued corruption has been traced to multiple
   analytics instances sharing the same write key. The duplicate-instance panic
   guard that should have prevented this was inadvertently disabled in a recent
   release. We can re-enable the panic, but that just crashes the app instead of
   corrupting data. Neither outcome is acceptable. LedgerStore turns this from a
   fatal misconfiguration into a supported use case. Given the active customer
   impact and the fact that we now have two classes of confirmed corruption (race
   conditions and multi-instance writes), this should be prioritized.
