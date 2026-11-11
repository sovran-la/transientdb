# transientdb 0.4.0 delivery: journal

All dates are 2026-11-10.

1. **Baseline gate on 12db575:** pass (native lib 324, integration 36, doctests 11, WASM 155).
2. **PR 1, the store surface:** committed as 39afc27, and the local gate passed.
   - CI failed only on WASM (W1-CI1). Under rustc 1.99.1, wasm-bindgen 0.2.95 gave closure shims from `gloo-timers` and transientdb the same export name in the integration-test binaries.
   - Review W1 found W1-R1 to W1-R4, and an owner question on the hydrate sweep.
   - All five were fixed in 322711b. The integration tests now wait on a `setTimeout` promise, and `WebStore` no longer hangs on an upgrade. CI passed.
3. **PR 2, FileStore:** built in two phases, the storage core and then the store.
   - Review W2 found W2-R1 (major: sequence saturation) and W2-R2 to W2-R4.
   - Fixed in 7e0d222.
4. **PR 3:** chrono, examples, README, test_all and CI, in four commits.
5. **Verifying PR 1's fixes:**
   - That review found W1-R5, a race in the new `blocked` handling, and the cause of a gate failure seen once. Fixed in 0fb658e; CI passed.
   - Verifying that fix found W1-R6, a timer race during an upgrade. Fixed in 5784d66; CI passed.
   - Verifying that fix found W1-R8, a doc nit. Fixed in 40ccb14.
6. **Verifying PR 2's fixes:**
   - That review found W2-R5 and W2-R6. Fixed in 65de9cc.
   - Verifying those found W2-R7, an ordering-dependent test. CI hit the same failure independently on Android ARM32. Fixed in 2976a26.
7. **Reviewing PR 3:**
   - The review found W3-R1 to W3-R7. Fixed in e6c1af8.
   - Verifying those found W3-R8 to W3-R12, plus the size-bound sentence in `FileConfig`. Fixed in 1978823, bda5ffe and 40ccb14.
8. **Final review of the last unreviewed commits across the stack:** it found F-1 to F-3, all nits. They were fixed in bef7cae, 2f0f81c and c236f7d; the F-2 mutant is now caught. Verifying those fixes found G-1 to G-4, all nits: G-1 was fixed in 926bb2f and 5b7a9df, and G-2 to G-4 were fixed in the PR descriptions.
9. **The disk filled once during the work.** Build directories were cleaned, and the interrupted fix was finished.
10. **The stack was restacked after each round of fixes,** and each head was gated before it was pushed.
