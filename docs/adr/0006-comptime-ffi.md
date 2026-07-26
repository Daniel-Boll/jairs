# ADR-0006: Compile-time code may call foreign functions

- **Status:** Accepted
- **Date:** 2026-07-25
- **Deciders:** dboll

## Context

Jairs runs code at compile time in a bytecode VM (`#run`). A build metaprogram
(wave W6) is a `#run` that configures the build — and a build metaprogram that
cannot read a file, query the environment, or shell out is useless: it cannot do
the very thing makefiles exist to do. So the question is whether compile-time
code is allowed to call *foreign* (C ABI) functions, not just other Jairs code.

- **No comptime FFI:** the VM stays a pure interpreter with no host reach. Simple
  and sandboxed, but `#run` cannot read a file, so build metaprograms are toys.
- **Comptime FFI:** the VM can call arbitrary C functions dynamically, which
  requires a libffi-style dynamic-call bridge inside the VM and opens a hole
  through which compile-time code touches the host.

## Decision

Compile-time code **may** call foreign functions, gated behind an explicit
`#foreign_at_comptime` allowance. Comptime FFI is off unless a declaration opts
in. The VM therefore includes a libffi-style dynamic-call bridge so it can invoke
C functions during compilation.

## Consequences

### Positive

- Build metaprograms (wave W6) can read files, inspect the environment, and call
  into host libraries — which is the entire point of "build scripts become the
  build system".
- The allowance is explicit, so a reader of a program can see which comptime code
  reaches out to the host.

### Negative

- The bytecode VM must carry a libffi bridge; the comptime engine is no longer a
  pure sandbox.
- Compile-time execution can now have side effects on the host machine, which is
  a real trust and reproducibility consideration for build scripts.

### Follow-on work this forces

- **Into the slice / VM design:** the VM's architecture must accommodate a
  libffi-style dynamic-call path (`libffi` is already pinned in the workspace for
  this reason). The slice's own `#run` is trivial arithmetic, but the VM design
  cannot foreclose comptime FFI.
- **Into wave W6:** `#foreign_at_comptime` and the build-metaprogram machinery
  land together and depend on this bridge.

## Alternatives considered

- **No comptime FFI.** Rejected: without it, `#run` cannot so much as read a file,
  which makes build metaprograms — the Jai "build scripts replace makefiles"
  superpower, wave W6 — impossible. The sandboxing benefit is not worth losing
  the feature the comptime engine exists to enable.
