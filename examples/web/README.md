# TransientDB Web Example

This example demonstrates using TransientDB with WebStore in a browser environment.

## Prerequisites

- [Rust](https://rustup.rs/)
- [wasm-pack](https://rustwasm.github.io/wasm-pack/installer/)
- A local web server (or use the Python one-liner below)

## Building

From this directory (`examples/web`):

```bash
wasm-pack build --target web
```

This creates a `pkg/` directory with the compiled WASM and JavaScript bindings.

## Running

You need to serve the files over HTTP (browsers block `file://` for WASM modules).

Using Python:
```bash
python3 -m http.server 8080
```

Using Node:
```bash
npx serve .
```

Then open http://localhost:8080 in your browser.

## What it demonstrates

1. **Creating a WebStore** with TransientDB
2. **Checking persistence state** - IndexedDB may be unavailable in private browsing
3. **Sending what the last visit left** - stored events are loaded when the store opens
4. **Appending events** - storing JSON data, and dropping an event too large to ever store
5. **Fetching events** - in batches limited by count and by bytes, each wrapped in the
   request body the page builds around `items.json()`
6. **Removing events** - once a request succeeds
7. **Releasing events** - when a request fails, so a later fetch returns them
8. **Resetting** - clearing all data

## Expected output

On a first visit you should see something like:

```
🦀 TransientDB WebStore Example
================================

📦 Creating WebStore...
✅ IndexedDB available - data will persist across page refreshes
✅ TransientDB created

📭 Nothing left from the last visit

📝 Appending events...
   Added: page_view
   Added: button_click
   Added: form_submit
   Added: page_view
   Dropped an event: item is 10268 bytes serialized, larger than max_item_size = 8192 bytes
📊 Has data: true

📡 Sending while offline...
   Could not send 2 events: the network is down
   Sent 0; still queued: true

📡 Sending once back online...
   POST {"events":[{"event":"page_view","page":"/home"},{"button":"sign_up","event":"button_click"}],"source":"demo-app"}
   POST {"events":[{"event":"form_submit","form":"registration"},{"event":"page_view","page":"/dashboard"}],"source":"demo-app"}
✅ Sent 4 events
📊 Has data after sending: false

🔄 Demo: Adding more events then resetting...
   Added 5 events, has_data: true
   After reset, has_data: false
✅ Reset complete

🎉 Demo complete!

One event is left queued. Refresh the page: if IndexedDB is available,
step 2 sends it.
```

After a refresh, step 2 reports instead:

```
📤 Sending events left from the last visit...
   POST {"events":[{"event":"page_hide","page":"/dashboard"}],"source":"demo-app"}
✅ Sent 1 event from the last visit
```

## Troubleshooting

**"Failed to load WASM"** - Make sure you ran `wasm-pack build --target web` first.

**"Memory-only mode"** - This happens in private/incognito browsing or when the page is loaded in a third-party iframe. IndexedDB is blocked in these contexts, so TransientDB falls back to in-memory storage.

**CORS errors** - Make sure you're serving over HTTP, not opening `index.html` directly as a file.
