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
a new storage backend that is **crash-safe by construction** — meaning the file format
itself prevents corruption, rather than relying on a fragile sequence of operations
to complete successfully.

---

## Background

Our analytics SDKs buffer events on disk before sending them to the server in batches.
The component responsible for this is `DirectoryStore`, part of the `TransientDB`
library. It writes events to JSON files, and when it's time to send, it reads those
files and uploads them.

### Why DirectoryStore Was Designed This Way

The original design was driven by a genuinely good insight: **the file on disk should
be the exact payload you send over the wire.** Our servers accept JSON batches, so
DirectoryStore writes JSON batch files. When it's time to send, there's no
transformation — you just open the file and stream it to the server. One file, one
HTTP request, zero processing.

This had real advantages:

- **Zero-cost sends.** The transport layer can call `post_file(path)` and stream the
  file bytes directly to the socket. No serialization at send time, no memory
  allocation, no assembling a batch from pieces.
- **Human-readable on disk.** An engineer debugging a stuck queue can open any file,
  see exactly what events are in it, and know precisely what the server would receive.
  No special tooling needed.
- **Simple mental model.** One file = one batch = one HTTP request. What's on disk is
  what goes on the wire. Easy to reason about, easy to test.

These are genuinely good properties, and for the common case (app runs normally, events
flow through, files get sent) the design works well. The problem only shows up when
the uncommon case happens — and on mobile, the uncommon case isn't uncommon at all.

### How It Works Today

1. **Open a file** and write the JSON preamble: `{ "batch": [`
2. **Append events** as comma-separated JSON objects
3. **Close the file** by writing the footer: `], "sentAt": "...", "writeKey": "..." }`
4. **Rename the file** to `.temp` to signal "this file is ready to send"

The tradeoff we didn't fully appreciate: making the file format match the wire format
means the file has **structural requirements** — an opening bracket, a closing bracket,
a rename — that create a window of vulnerability. The file is **invalid JSON until step
3 completes**. If anything interrupts this sequence, the file is corrupt.  On mobile,
apps get killed by the OS constantly — backgrounding, memory pressure, thermal limits,
user force-quits. What's "uncommon" on a server is routine on a phone.

---

## The Problem

### The Customer Incident

In early March 2026, a customer on a significant contract reported receiving corrupted
JSON payloads from our iOS SDK. The server was rejecting batches because the JSON was
structurally invalid.

### Root Cause Investigation

We traced the corruption to a race condition in the async/sync queue interaction, where
`fetch()` could execute before in-flight appends completed. We shipped a targeted fix
for the race condition itself.

But during the investigation, we identified a deeper, structural problem: **the file
format is inherently fragile**. The race condition was one trigger, but the same class
of corruption can happen any time the multi-step file lifecycle is interrupted.

### Failure Points in DirectoryStore

Here are the specific scenarios where DirectoryStore produces corrupt files:

**1. Crash after writing events, before writing the footer**

The file contains `{ "batch": [{"event":"a"},{"event":"b"}` — valid event data, but
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
kill. Next launch, the file gets reopened for more writes — but it was already at (or
over) the size cap. Now we're appending to a file that should have been finalized.

### Why This Is a Structural Problem

All four scenarios share the same root cause: **the file is invalid until a multi-step
ceremony completes**. Writing the preamble, appending events, writing the footer, and
renaming — all four steps must succeed, in order, without interruption. Any failure
leaves the file in an ambiguous state that recovery code has to guess about.

The fix we shipped (checking `isFinalized()` in `startFileIfNeeded()`) handles
scenario #2 specifically. But it's a patch on a fundamentally fragile design. Every
new edge case we discover will need another patch.

---

## Proposed Solution: LedgerStore

Instead of patching DirectoryStore, we propose a new storage backend called
`LedgerStore` that eliminates the corruption problem by design.

### Core Idea: Binary Framed Records

Instead of wrapping events in a JSON structure that requires opening and closing,
LedgerStore writes each event as an independent, self-validating binary record:

```
┌──────────────┬──────────────┬─────────────────────┐
│ payload size  │  checksum    │  event JSON bytes   │
│  (4 bytes)    │  (4 bytes)   │  (variable length)  │
└──────────────┴──────────────┴─────────────────────┘
```

Each record is:

- **Self-contained** — it has everything needed to validate itself (a length and a
  checksum). No dependency on previous or subsequent records.
- **Written in a single operation** — the 4-byte length + 4-byte checksum + payload
  are assembled into one buffer and written to disk in one call.
- **Independently verifiable** — you can check any record by reading 8 bytes of header,
  reading `size` bytes of payload, and checking the checksum.

The event data inside each record is the same JSON you'd put in a batch today — a
serialized event object. The binary framing is just a thin wrapper that tells us where
each record starts and ends, and whether it's intact.

### How It Solves Each Failure Point

**Crash during write?**
The partially written record doesn't need to be detected or repaired at startup. When
the file is eventually read for sending, the streaming reader validates each record as
it goes: does the header have 8 bytes? Do we have enough payload bytes? Does the
checksum match? The first record that fails any check ends the read. Everything before
it is sent normally. We lose at most one event, and the file gets deleted after a
successful send.

**No closing ceremony to fail.**
There is no preamble, no footer, no rename. The file is a sequence of records,
concatenated back-to-back. It is valid at every point in time — after zero records,
after one record, after a thousand. There is nothing to "finalize."

**No rename means no rename failure.**
Files don't need a `.temp` extension to be "ready." Every file in the directory with
complete records is ready to send. The only thing that distinguishes the active file
(being written to) from finished files is that it has the highest index number — and
only within a single process lifetime. The moment the process ends (normally or via
crash), that file is finished.

This is enforced by a simple, hard rule: **on startup, always start a new file.** We
never reopen a file from a previous session. The startup sequence scans the directory
to find the highest file index, and that's it — no file contents are read, no
validation, no truncation. Every existing file is treated as finished. The next
`append()` creates a brand new file with the next index number.

But what about a file that has a partial trailing record from a crash? It doesn't need
to be fixed at startup. The streaming reader (described below) validates each record as
it reads. If it hits a partial or corrupt record at the end of a file, it simply stops
reading records and completes the batch with whatever valid records came before it. The
server gets a valid batch with N-1 events instead of N. After the server responds 200,
we delete the file — partial record and all. The corruption never needs to be "repaired"
because it never leaves the device.

Why not reopen the last file if it's under the size cap? Because that requires
answering questions: Is the file intact? Did the previous process close it cleanly?
How much space is left? Is there a partial record at the end we need to deal with
before appending? Each question is an edge case. The "always start fresh" rule
eliminates all of them. If it creates a small trailing file with only a few events,
that's fine — it gets sent and deleted on the next flush cycle just like any other file.

**Size cap is a soft boundary.**
When a file exceeds the configured size limit, we just close the handle. The next event
opens a new file. If the app gets killed before that happens, no problem — the startup
rule handles it. We were going to start a new file anyway.

### How Events Get Sent

The events still need to arrive at the server as JSON batches. Here's how that works
without loading entire files into memory:

When the SDK is ready to send (flush), it calls `fetch()`. This:

1. Closes the active file (so pending events are included)
2. For each finished file, constructs a **LedgerReader** — a streaming adapter that
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

### Startup / Recovery

There is no recovery step. Startup is:

1. Scan the directory for files
2. Find the highest index number
3. Set the next index to highest + 1

That's it. No reading file contents. No validating records. No truncating. No checking
for `.temp` extensions. No looking for footers. No `isFinalized()` checks. If a file
has a partial trailing record from a crash, the streaming reader will handle it at send
time — it simply stops at the bad record, sends everything before it, and the file gets
deleted after a successful 200.

### What Doesn't Change

- **The `TransientDB` API is the same.** `append()`, `fetch()`, `remove()`, `has_data()`,
  `reset()` — all the same methods, same patterns. Code that uses `TransientDB` today
  only changes which store it creates at init time.
- **The wire format is the same.** The server still receives JSON batches with `batch`,
  `sentAt`, and `writeKey`. The transformation just happens at send time instead of
  write time.
- **The file lifecycle is the same.** Write events → flush → send → delete. Same flow,
  just crash-safe now.

---

## Comparison

| Concern | DirectoryStore | LedgerStore |
|---|---|---|
| File validity | Invalid until footer is written | Always valid |
| Crash during write | Corrupt file, ambiguous recovery | Reader skips bad record at send time, everything else delivered |
| Rename required? | Yes (`.temp` signals readiness) | No — all files are always ready |
| Recovery logic | Multiple heuristics, edge cases | None — startup scans filenames only, reader validates at send time |
| Files modified after write? | Yes (footer appended, file renamed) | Never — append-only while active, read-only forever after |
| Memory at send time | Full file loaded as JSON | One event at a time (~2 KB) |
| Wire format coupling | Baked into file format | Configured at send time via callbacks |
| Max data loss on crash | Entire file (potentially hundreds of events) | One event |

---

## Migration Path

LedgerStore is a **new store type**, not a modification to DirectoryStore. Both will
coexist in the codebase during transition.

1. On startup, check for any existing DirectoryStore files (`.temp` extension)
2. Send and delete them using the current JSON format — drain the legacy queue
3. All new writes go to LedgerStore
4. Once legacy files are fully drained (typically one app session), DirectoryStore
   code becomes unused

This means **zero data loss during migration** — events written under the old format
still get sent, and new events benefit from crash safety immediately.

---

## Implementation Plan

LedgerStore will be implemented in both the Rust core (`twilio-data-core`) and Swift
(`analytics-swift` / `twilio-data-swift`). A detailed specification covering the binary
format, validation algorithm, and streaming reader is available in `LEDGER_SPEC.md` in
the transientdb repository.

The implementation is self-contained — it's a new file alongside the existing store
implementations, with no changes to the `TransientDB` wrapper or the `DataStore`
interface.

---

## Open Questions

1. **Should we backport to Kotlin?** The Kotlin SDK uses a similar DirectoryStore
   pattern. The same vulnerability exists there.
2. **Checksum algorithm:** The spec proposes CRC-32 (same as used by gzip/PNG/Ethernet).
   Fast, well-tested, available in every language. Any concerns?
3. **Timeline:** The customer has come back and confirmed that the `isFinalized()`
   fix did not resolve their issue. This means there is at least one more failure
   path we haven't identified or patched. We can keep playing whack-a-mole with
   individual crash scenarios, or we can replace the foundation. Every patch we
   write is more code that exists solely to work around a file format that can't
   protect itself. LedgerStore eliminates the entire category of problem. Given
   the active customer impact, this should be prioritized.
