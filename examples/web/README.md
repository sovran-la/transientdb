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
3. **Appending events** - storing JSON data
4. **Fetching events** - retrieving batches with optional limits
5. **Removing events** - cleaning up after processing
6. **Resetting** - clearing all data

## Expected output

You should see something like:

```
🦀 TransientDB WebStore Example
================================

📦 Creating WebStore...
✅ IndexedDB available - data will persist across page refreshes
✅ TransientDB created

📝 Appending events...
   Added: page_view
   Added: button_click
   Added: form_submit
   Added: page_view
✅ Appended 4 events

📊 Has data: true

📥 Fetching first 2 events...
   writeKey: demo-app
   sentAt: 2024-01-01T10:00:00.000Z
   items: 2
     - page_view
     - button_click
✅ Removed fetched events

📥 Fetching remaining events...
   Remaining items: 2
     - form_submit
     - page_view
✅ Removed remaining events

📊 Has data after removal: false

🔄 Demo: Adding more events then resetting...
   Added 5 events, has_data: true
   After reset, has_data: false
✅ Reset complete

🎉 Demo complete!

Try refreshing the page - if IndexedDB is available,
any unsent events would persist across refreshes.
```

## Troubleshooting

**"Failed to load WASM"** - Make sure you ran `wasm-pack build --target web` first.

**"Memory-only mode"** - This happens in private/incognito browsing or when the page is loaded in a third-party iframe. IndexedDB is blocked in these contexts, so TransientDB falls back to in-memory storage.

**CORS errors** - Make sure you're serving over HTTP, not opening `index.html` directly as a file.
