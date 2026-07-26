# ADR-0004: Strings are `{data: *u8, count: s64}` and not NUL-terminated

- **Status:** Accepted
- **Date:** 2026-07-25
- **Deciders:** dboll

## Context

A systems language has to pick a string representation, and the choice is
load-bearing because it appears at the FFI boundary from the very first slice:
the Jairs-0 stdlib's `print` calls libc `write`, which takes a pointer and a
length. The candidates:

1. **NUL-terminated `*u8`** (C strings): interops with C for free, but the length
   is not carried, so every length query is an O(n) scan, embedded NULs are
   impossible, and slicing requires copying.
2. **A fat pointer `{data, count}`** (the Jai/Go/Rust-slice model): the length
   travels with the pointer, slicing is O(1) and allocation-free, embedded NULs
   are fine — but it is not directly a C string, so C interop needs a bridge.

## Decision

A Jairs `string` is `{data: *u8, count: s64}` and is **not NUL-terminated**. The
two fields are directly accessible as `.data` and `.count`. Bridging to a
`#foreign` function that expects a C string is done explicitly with
`to_c_string()`, which produces a NUL-terminated copy in temporary storage.

## Consequences

### Positive

- `.count` is O(1); slicing is O(1) and copy-free; embedded NULs are legal.
- The representation maps straight onto the `(pointer, length)` shape that
  `write(2)` and most modern syscalls already want, so the slice's `print` is a
  direct pass-through of `s.data` and `s.count`.

### Negative

- Passing a Jairs string to a C function that expects `char*` requires an
  explicit `to_c_string()` and a temporary allocation; it is not free.

### Follow-on work this forces

- **Into the slice:** the two-field string layout must exist before the stdlib's
  `print` can hand `s.data` and `s.count` to `write`; the FFI boundary and the
  string ABI therefore both land in Jairs-0. See
  `tests/corpus/valid/021-string-literals.jr` (uses `plain.count` and
  `plain.data`) and `PLAN.md` §1.2.
- **Into wave W3:** `to_c_string()` depends on temporary storage, which arrives
  with the runtime-core wave; until then, string→C bridging is not available.

## Alternatives considered

- **NUL-terminated C strings.** Rejected: O(n) length, no embedded NULs, and
  copy-on-slice — all of which the fat pointer avoids — in exchange for C interop
  that `to_c_string()` recovers cheaply when it is actually needed.
