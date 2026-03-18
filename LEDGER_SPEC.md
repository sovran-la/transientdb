# LedgerStore Specification

**Version:** 0.1.0
**Status:** Draft
**Authors:** Dr. Sneed, Porter
**Date:** 2026-03-12

## Overview

LedgerStore is a crash-safe, append-only binary record store designed for buffering
analytics events on disk and streaming them to the network as JSON batches with minimal
memory overhead.

It replaces DirectoryStore's fragile JSON file format — which requires a multi-step
open/close ceremony that is vulnerable to corruption on crash — with a self-validating
binary framed record format where every record is independently valid at all times.

### Design Goals

1. **Fast** — append-only writes, no seeking, no rewriting.
2. **Ultra-low memory** — write-through to disk. No in-memory event queuing.
3. **Streaming network sends** — finished files can be streamed to the network as
   JSON batches without loading entire files into memory.
4. **Crash-safe by construction** — no finalization ceremony. The file format itself
   is the safety mechanism. Any interruption produces a recoverable state.

### Non-Goals

- LedgerStore does not define or enforce any specific wire protocol. The envelope
  format (header, footer, separator) is configured by the consumer at construction time
  via callbacks. The store itself treats payloads as opaque bytes.
- LedgerStore is not a general-purpose database. It is a FIFO buffer for transient data
  that will be sent over the network and deleted.

---

## Binary Record Format

Each record consists of a fixed 8-byte header followed by a variable-length payload.

```
┌──────────────────┬──────────────────┬─────────────────────────┐
│  payload_length   │     crc32        │        payload          │
│    (4 bytes)      │   (4 bytes)      │   (payload_length bytes)│
│   u32 LE          │   u32 LE         │   raw bytes             │
└──────────────────┴──────────────────┴─────────────────────────┘
```

### Fields

| Field            | Type    | Description                                      |
|------------------|---------|--------------------------------------------------|
| `payload_length` | u32 LE  | Length of the payload in bytes. Must be > 0.      |
| `crc32`          | u32 LE  | CRC-32 (ISO 3309 / ITU-T V.42) of the payload bytes. |
| `payload`        | bytes   | The raw event data. Compact JSON (no embedded newlines). |

### Byte Order

All multi-byte integers are **little-endian**. This is the native byte order on x86,
ARM (in default configuration), and WASM — the three targets that matter.

### CRC-32 Algorithm

Use the ISO 3309 polynomial (`0xEDB88320`, reflected). This is the same algorithm used
by zlib, gzip, PNG, and Ethernet. Every language has a well-tested implementation:

- **Rust:** `crc32fast` crate
- **Swift:** `zlib` via `import zlib` — `crc32(0, bytes, length)`
- **WASM:** same Rust code, compiled to wasm32

### Payload Contract

The payload MUST be compact JSON produced by the platform's standard JSON serializer
with no pretty-printing. This guarantees no embedded newlines within a single record's
payload, which simplifies debugging (hexdump shows clear record boundaries).

The store treats the payload as opaque bytes. It does not parse, validate, or transform
the JSON. The payload is stored exactly as provided and returned exactly as stored.

### Maximum Record Size

A single record's payload is limited to `u32::MAX` bytes (~4 GB). In practice, analytics
events are typically 200 bytes to 2 KB. Implementations SHOULD reject payloads larger
than 1 MB as a safety check (configurable).

---

## File Layout

### Directory Structure

All files live in a single configured directory. Each file is named with a numeric index
prefix followed by the configured base filename:

```
storage_location/
  0-events
  1-events
  2-events
  3-events      ← active file (highest index, current process)
```

### File Naming

Pattern: `{index}-{base_filename}`

- `index`: zero-based, monotonically increasing unsigned integer.
- `base_filename`: configured at store creation (e.g., `"events"`).
- No file extension. No `.temp`, no `.done`, no `.ready`. The file is always valid.

### File Contents

A file is a sequence of zero or more binary records, concatenated with no delimiters
or padding:

```
┌─────────┬─────────┬─────────┬─────────┐
│ Record 0│ Record 1│ Record 2│   ...   │
└─────────┴─────────┴─────────┴─────────┘
```

There is no file header, no file footer, and no file-level metadata. The file is valid
if and only if it contains zero or more complete, CRC-validated records.

---

## Operations

### Configuration

```
LedgerConfig {
    storage_location: path         // Directory for data files
    base_filename: string          // Base name for files (e.g., "events")
    max_file_size: usize           // Soft cap in bytes before rotating (must be >= 100)
    header: () -> bytes            // Callback: envelope header, called at read time
    footer: () -> bytes            // Callback: envelope footer, called at read time
    separator: bytes               // Separator between record payloads (e.g., ",")
}
```

The `header` and `footer` callbacks are invoked each time a reader is created for a
finished file. This allows dynamic values (e.g., timestamps) to be generated at send
time rather than stored on disk.

Note: protocol-specific fields like `write_key` and `sentAt` belong in these callbacks,
not in the store's configuration. The store treats them as opaque byte sequences.

#### Example: Segment Batch Envelope

```rust
LedgerConfig {
    storage_location: PathBuf::from("/tmp/events"),
    base_filename: "events".into(),
    max_file_size: 475_000,
    header: Box::new(|| br#"{"batch":["#.to_vec()),
    footer: Box::new(move || {
        format!(
            r#"],"sentAt":"{}","writeKey":"{}"}}"#,
            Utc::now().to_rfc3339(),
            write_key
        ).into_bytes()
    }),
    separator: b",".to_vec(),
}
```

#### Example: JSONL (Newline-Delimited JSON)

```rust
LedgerConfig {
    // ...
    header: Box::new(|| vec![]),           // no header
    footer: Box::new(|| vec![]),           // no footer
    separator: b"\n".to_vec(),            // newline between records
}
```

### Startup

On startup (including first-ever launch):

1. Scan `storage_location` for files matching the naming pattern.
2. Determine `next_index` = highest existing index + 1 (or 0 if no files).
3. Do NOT open a file for writing yet — wait for the first `append()`.

That's it. No file contents are read. No validation. No truncation. All existing files
are treated as finished and eligible for `fetch()`.

If a file has a partial trailing record from a crash, it does not need to be repaired.
The `LedgerReader` validates each record during streaming (see
[Record Validation During Read](#record-validation-during-read)). A corrupt trailing
record simply ends the read early — the batch is sent with N-1 events, the server
responds 200, and the file is deleted.

**Rationale for always starting a new file:** Reopening the highest-index file requires
checking its size, validating whether the previous process closed it cleanly, and handling
edge cases where it's already at the size cap. Starting fresh eliminates all of these
concerns. Small trailing files are not a problem — they get sent and deleted on the next
flush cycle.

### Multi-instance safety

LedgerStore supports multiple instances writing to the same directory concurrently
(e.g., two analytics instances configured with the same API key). This is safe because
of two properties:

1. **In-process:** `next_index` is an atomic integer (`AtomicU32` in Rust, atomic
   property in Swift). Multiple threads within the same process get unique indices
   without locking.

2. **Cross-process:** Files are created with exclusive-create semantics
   (`O_CREAT | O_EXCL` / `create_new(true)`). If two instances race to create the
   same index, one succeeds and the other gets `AlreadyExists`, bumps its index,
   and retries. This is atomic at the filesystem level.

Because each record is self-contained and each instance writes to its own file, there
is no interleaving. Instance A's files and instance B's files coexist in the directory.
`fetch()` returns all of them. They all get sent and deleted independently.

This eliminates the need for a duplicate-instance panic guard. What was previously a
fatal error becomes a supported configuration.

### append(data: bytes) -> Result<()>

1. If no active file is open, create `{next_index}-{base_filename}` using
   exclusive-create (`O_CREAT | O_EXCL`). If the file already exists (another instance
   claimed this index), increment `next_index` and retry. This loop is bounded by
   a maximum attempt count as a safety measure.
2. Serialize the record: `payload_length` (4 LE) + `crc32` (4 LE) + `payload`.
   Build the complete record in a single buffer.
3. Write the entire buffer in a **single `write()` syscall**. This is critical for
   crash safety — a single write produces a prefix on crash, not interleaved garbage.
4. Flush the writer (if buffered).
5. If the file's total size now exceeds `max_file_size`, close the file handle. The
   next `append()` will create a new file.

**Important:** The record MUST be written as a single contiguous `write()`. Do NOT
write the header and payload separately.

### fetch(count?, max_bytes?) -> Result<Option<DataResult<Vec<LedgerReader>>>>

1. If an active file handle is open, flush and close it. This file is now finished
   and eligible for return.
2. Scan and sort all files in the directory by index.
3. Apply `count` limit (max number of files to return).
4. Apply `max_bytes` limit (cumulative file size cap).
5. For each finished file, construct a `LedgerReader` (see [LedgerReader](#ledgerreader)).
   Each reader is self-contained: it owns its file handle, holds pre-generated
   header/footer bytes, and has pre-computed its content length.
6. Return the readers as `data` and the file paths as `removable` references.
7. The next `append()` will create a new file.

The `header()` and `footer()` callbacks are invoked during this step — once per file.
This means timestamps in the footer reflect the moment of fetch, not the moment of send.

### remove(items) -> Result<()>

Delete the specified files from disk. The `items` are the `removable` references
returned by `fetch()` — file paths corresponding to the `LedgerReader`s in `data`.
Called after successful network send (HTTP 200). The readers and removable items
share the same ordering, so the caller can track which sends succeeded and remove
selectively if needed.

### has_data() -> bool

Returns `true` if:
- There is an active file with data written to it, OR
- There are any files in the directory matching the naming pattern.

### reset()

Delete all files in the directory matching the naming pattern. Close any active file handle.

---

## Record Validation During Read

Validation happens inside `LedgerReader` as records are streamed, not at startup.
There is no separate validation pass — each record is validated on demand as
the HTTP client pulls bytes from the reader.

For each record, the reader performs these checks in order:

```
// 1. Can we read a complete header?
if remaining_bytes < 8:
    // Partial header — stop reading, transition to FOOTER
    done

payload_length = read_u32_le(file)
crc32_expected = read_u32_le(file)

// 2. Is the payload length valid?
if payload_length == 0:
    // Zero-length payload — stop reading, transition to FOOTER
    done

// 3. Do we have enough bytes for the full payload?
if remaining_bytes < payload_length:
    // Incomplete payload — stop reading, transition to FOOTER
    done

// 4. Does the checksum match?
payload = read_bytes(file, payload_length)
crc32_actual = compute_crc32(payload)

if crc32_actual != crc32_expected:
    // CRC mismatch — stop reading, transition to FOOTER
    done

// Record is valid — yield payload bytes, advance to next record
```

When validation fails on any record, the reader transitions to the FOOTER phase.
The resulting output is structurally valid (header + records + footer), just shorter
than expected. The file is deleted after a successful send — no truncation or
repair is ever needed.

This approach has a key advantage: **the file on disk is never modified after it is
written.** No truncation, no rewriting, no recovery logic. Files are append-only
during their active lifetime and read-only forever after. This eliminates an entire
category of failure modes around file mutation during recovery.

The same validation logic is used by `content_length()` during its header-scanning
pass. If it encounters a bad record, it stops counting — the pre-computed length
will match the actual bytes the reader produces.

---

## LedgerReader

`LedgerReader` is a self-contained streaming reader constructed by `fetch()`. It
implements the platform's streaming read interface (`Read` in Rust, `InputStream` in
Swift) and synthesizes the configured envelope around the binary records on the fly.

Once constructed, a `LedgerReader` has no dependency on the store. It owns everything
it needs: an open file handle, pre-generated header and footer bytes, the separator,
and its pre-computed content length.

### Construction (inside fetch())

When `fetch()` builds a `LedgerReader` for a finished file, it:

1. Opens the file for reading.
2. Invokes `config.header()` and `config.footer()`, storing the resulting bytes.
3. Pre-computes the content length (see below).
4. Initializes the state machine in the HEADER phase.

### content_length() -> u64

Returns the pre-computed total byte count the reader will produce. Computed at
construction time by:

1. Using the stored `header` and `footer` byte lengths.
2. Scanning all record headers to sum payload lengths.
3. Adding: header length + footer length + total payload bytes +
   separator length × (num_records - 1).

This scan reads only 8-byte headers and seeks forward by `payload_length` per record.
Payloads are not read. The file is then seeked back to the beginning for streaming.

### State Machine (impl Read)

```
┌─────────┐    ┌────────────────┐    ┌─────────┐    ┌────────┐
│  HEADER  │───►│  RECORD_PAYLOAD │───►│  FOOTER  │───►│  DONE  │
│  (bytes) │    │  (loop per record) │  │  (bytes) │    │        │
└─────────┘    └────────────────┘    └─────────┘    └────────┘
```

1. **HEADER**: Yield stored header bytes.
2. **RECORD_PAYLOAD**: For each record in the file:
   a. Read the 8-byte header. Validate CRC.
   b. If not the first record, yield separator bytes.
   c. Yield the payload bytes directly (they are already the stored content).
   d. Repeat until no more records.
3. **FOOTER**: Yield stored footer bytes.
4. **DONE**: Return EOF / 0 bytes.

Peak memory is one record buffer (~2 KB typical) regardless of file size.

### Error Handling

If the reader encounters a corrupt record (CRC mismatch, partial header) during
streaming, it SHOULD:

1. Stop yielding record payloads.
2. Immediately transition to FOOTER phase.
3. The resulting output will contain fewer records than expected but will be
   structurally valid (header + partial records + footer).

The caller can detect this by comparing actual bytes read vs. `content_length()`.

### Usage with HTTP

The reader plugs directly into HTTP clients that accept `impl Read`:

```rust
// Full lifecycle through TransientDB
let db = TransientDB::new(LedgerStore::new(config)?);
// append side
db.append(json!({"event": "page_view"}))?;
db.append(json!({"event": "button_click"}))?;

// send side
if let Some(result) = db.fetch(None, None)? {
    let readers = result.data.unwrap();
    let removable = result.removable.unwrap();

    for reader in readers {
        request
            .set("Content-Length", &reader.content_length().to_string())
            .send(reader)?;  // reader implements Read
    }

    // all sent successfully — delete the files
    db.remove(&removable)?;
}
```

Peak memory during the entire HTTP send is one record buffer (~2 KB) regardless of
file size or number of files. Twelve 475 KB files are sent sequentially with the
same ~2 KB footprint. The caller never touches the store directly — everything flows
through `TransientDB`.

---

## Crash Safety Analysis

### Crash During append()

**Scenario:** App crashes mid-write of a record.

**On-disk state:** The active file contains N complete records followed by a prefix of
the (N+1)th record — which could be partial header, partial CRC, or partial payload.

**Recovery:** None needed at startup. On next launch, the file is treated as finished.
When `LedgerReader` streams it for sending, it validates each record and stops at the
partial one. The batch is sent with N events. The server responds 200. The file is
deleted. One event lost. The file is never modified.

### Crash After File Rotation (close)

**Scenario:** Active file exceeded size cap. File handle was closed. App crashes before
next append creates a new file.

**On-disk state:** The closed file contains complete, valid records. No new file exists yet.

**Recovery:** Next startup sees the file as highest-index. Starts a new file at
highest + 1. The closed file is returned by the next `fetch()`. Zero events lost.

### Crash During Startup

**Scenario:** App crashes during startup while scanning the directory.

**On-disk state:** Unchanged — startup only reads directory entries, never modifies files.

**Recovery:** Next startup scans the directory again. Idempotent. No file mutation means
no partial state to recover from.

### Crash During remove() (Post-Send Deletion)

**Scenario:** HTTP 200 received. App crashes while deleting sent files.

**On-disk state:** Some files deleted, some still exist.

**Recovery:** The surviving files will be returned by the next `fetch()` and re-sent.
The server must handle duplicate delivery (idempotent ingestion). This is already a
requirement for analytics systems.

---

## Implementation Notes

### Rust

- CRC-32: `crc32fast` crate (hardware-accelerated on x86/ARM).
- File I/O: `std::fs::File` with `BufWriter`. Flush on close.
- `LedgerReader`: implements `std::io::Read`. Owns its `File` handle, `Vec<u8>` for
  header/footer/separator, and `u64` content length. Constructed inside `fetch()`.
- `DataStore` impl: `type Output = Vec<LedgerReader>`.
- Callbacks: `header` and `footer` are `Box<dyn Fn() -> Vec<u8> + Send + Sync>`.
- No async. No tokio. `std::thread` + channels if concurrency is needed.
- Single write: build complete record in a `Vec<u8>`, then `writer.write_all(&buf)`.

### Swift

- CRC-32: `import zlib` — `crc32(0, bytes, length)`.
- File I/O: `FileHandle` for append. `Data` for record construction.
- `LedgerReader`: implements `InputStream` (or a custom streaming protocol). Owns its
  file handle, pre-generated header/footer `Data`, and pre-computed content length.
- Callbacks: `header` and `footer` are `() -> Data` closures.
- Single write: build complete record as `Data`, then `fileHandle.write(data)`.

### WASM

LedgerStore is NOT applicable to WASM targets. Browser environments should continue
to use `MemoryStore` or `WebStore` (IndexedDB). Binary file I/O is not available in
the browser sandbox.

---

## Migration from DirectoryStore

LedgerStore is a new store type, not a replacement for DirectoryStore. Both can coexist
in the same crate. Consumers choose which store to use at configuration time.

For consumers migrating from DirectoryStore:

1. On startup, check for existing DirectoryStore files (`.temp` extension).
2. Process them using the old format (read as JSON, send, delete).
3. Switch to LedgerStore for all new writes.
4. Once all legacy files are drained, DirectoryStore code can be removed.

---

## Future Considerations

- **Compression:** Records could be individually compressed (e.g., zstd per record)
  with a format version byte prepended to the file. Not in v1.
- **Encryption:** Per-record encryption using AES-256-GCM with a nonce in the header.
  Would require extending the header format. Not in v1.
- **Batched writes:** Multiple events written as a single `write()` call for higher
  throughput. The record format supports this naturally — just concatenate multiple
  records in one buffer. Not in v1 but trivial to add.
