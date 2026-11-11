# transientdb 0.4.0 delivery: state

- **Contract:** `docs/PLAN_0_4_0.md` and `docs/FILE_STORE_SPEC.md` at 12db575, on branch `bsneed/file-store`.
- **Gate:** `gate.sh` in this directory, run on the exact head of each PR. It runs:
  - fmt, check, clippy (lib and all targets), doc;
  - lib tests, doctests, integration tests;
  - `cargo +1.83.0 check` as the MSRV proxy, the iOS-simulator test build, the examples build;
  - `./test_all.sh`, native and WASM in headless Chrome;
  - fmt, clippy and the WASM suite again under rustc 1.99.1, the stable version CI installs.
- **CI:** `ci.yml` runs on `pull_request` only for base `main`, so each stacked branch is run with `workflow_dispatch`.

## Pull requests

| PR | Branch | Base | Head | Local gate | CI |
|---|---|---|---|---|---|
| 1. The store surface | `bsneed/store-surface` | `main` | bef7cae | see PR | see PR |
| 2. FileStore | `bsneed/file-store-impl-v2` | `bsneed/store-surface` | 926bb2f | see PR | see PR |
| 3. Examples, README, chrono, CI | `bsneed/finish-0-4-0` | `bsneed/file-store-impl-v2` | this commit | see PR | see PR |

PR 1 targets `main`, so it also carries 12db575, the plan and spec. PR 2 is pushed as `bsneed/file-store-impl-v2` because `bsneed/file-store-impl` on the remote holds an earlier build of it (f73c4cb) on an older PR 1; that branch is superseded and was left as it was rather than force-pushed.

The final gate and CI results for each head are in that PR's description. A CI run cannot be recorded inside the commit it tests.

Earlier heads and their results:
- **PR 1:**
  - 39afc27: the local gate passed; CI failed on WASM (W1-CI1).
  - 0fb658e: CI passed, run 42935624697.
  - 5784d66: CI passed, run 42939280853.
- **PR 2:** f73c4cb: CI failed on Android ARM32, run 42937753273. That failure was W2-R7, since fixed.

## Commits

- **PR 1:** 39afc27, 322711b, 0fb658e, 5784d66, 40ccb14, bef7cae.
- **PR 2:** c8aa950, a588f3a, 7e0d222, 65de9cc, 2976a26, bda5ffe, 2f0f81c, 926bb2f.
- **PR 3:**
  - 23d777d: chrono.
  - a7600e0: examples.
  - 8766ec6: README.
  - 6ca1843: test_all and CI.
  - e6c1af8, 1978823, c236f7d, 5b7a9df: review fixes.
  - The commit that adds this record.

## Review findings

All are resolved. Each fix was verified by a review separate from the one that found it.

| ID | Severity | Summary | Fixed in |
|---|---|---|---|
| W1-CI1 | blocking | Duplicate wasm export under rustc 1.99.1 with wasm-bindgen 0.2.95 | 322711b |
| W1-R1 | minor | No regression test for settle's surrender-before-lock order | 322711b |
| W1-R2 | nit | MemoryStore docs promised `Io` | 322711b |
| W1-R3 | nit | Unreadable IndexedDB records were not counted or deleted | 322711b |
| W1-R4 | minor | `WebStore::new` hung while an older version was held open | 322711b |
| W1-R5 | minor | Giving up at once on `blocked` mistook a closing connection for a holder | 0fb658e |
| W1-R6 | minor | The give-up timer could fire during a running upgrade | 5784d66 |
| W1-R7, W1-R8 | nit | Upgrade docs | 5784d66, 40ccb14 |
| W2-R1 | major | One damaged entry saturated sequence numbers | 7e0d222 |
| W2-R2 | minor | A failed drop payload write with the oldest item held was untested | 7e0d222 |
| W2-R3 | minor | A dangling symlink at the path gave a permanent error | 7e0d222 |
| W2-R4 | nit | `has_data` with only damaged items: a spec deviation, recorded | 7e0d222 |
| W2-R5 | nit | A symlink to a missing directory in the path | 65de9cc |
| W2-R6 | nit | `append` exhaustion doc | 65de9cc |
| W2-R7 | minor | A cross-thread test depended on thread ordering | 2976a26 |
| W3-R1 | minor | The examples did not check what they showed | e6c1af8 |
| W3-R2, W3-R3 | minor | README gaps on WebStore persistence and MemoryStore `Io` | e6c1af8 |
| W3-R4 to W3-R7 | nit | README and example wording; the size bound | e6c1af8, bda5ffe |
| W3-R8 to W3-R12 | nit | Example plurals, a comment, README wording | 1978823, 40ccb14 |
| F-1 | nit | The symlinked-path case in the `FileConfig::path` docs and the README | 2f0f81c, c236f7d |
| F-2 | nit | Kept file length when the survivors sit in low slots | 2f0f81c |
| F-3 | nit | Precision of the WebStore exhaustion docs | bef7cae, c236f7d |
| G-1 | nit | Which calls fail for a path through a missing symlinked directory | 926bb2f, 5b7a9df |

## Open for the owner

These are the questions for review in the PR descriptions. The ones that most need a decision:
- the WebStore hydrate sweep, which reverses the 2026-11-09 decision;
- the places where FileStore differs from `FILE_STORE_SPEC.md`, which was left unedited because it is the contract;
- the W1-CI1 exposure for applications.
