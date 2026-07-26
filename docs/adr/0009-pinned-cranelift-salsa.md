# ADR-0009: `cranelift-*` and `salsa` are pinned with `=`

- **Status:** Accepted
- **Date:** 2026-07-25
- **Deciders:** dboll

## Context

Two of the project's core dependencies — the Cranelift codegen crates and
`salsa` — have APIs that are explicitly **not semver-stable**. A minor version
bump can break compilation. Left on caret ranges (`^`), a routine `cargo update`
would intermittently break the build in ways unrelated to any change we made.

Cranelift additionally shapes the compiler's *architecture*, not just its
dependency graph: it has **no function inlining** and only limited loop
optimisation (LICM/GVN/const-fold via its egraph mid-end). Anything that needs
inlining — `#expand` macros, comptime-heavy code — must be inlined before the
code ever reaches Cranelift.

## Decision

`cranelift-*` and `salsa` are pinned with `=` in `Cargo.toml`
(`cranelift-* = "=0.134.2"`, `salsa = "=0.28.1"`); every other dependency uses
caret ranges. All Cranelift API contact is confined to the `jr-codegen-clif`
crate behind a `Backend` trait, so that an API break — or a later LLVM backend —
touches exactly one crate.

We also record the architectural consequence: because Cranelift does no
inlining, the **inliner lives in our own MIR mid-end**, upstream of every
backend. We need that inliner anyway for `#expand` macros and comptime, so it is
not extra work — but it must exist before any backend does.

## Consequences

### Positive

- The build does not break on an unrelated `cargo update`; dependency upgrades of
  these two crates are deliberate, reviewed events.
- Cranelift's non-semver API is quarantined to one crate behind `Backend`, so the
  blast radius of an upgrade — or of adding LLVM — is contained.
- Placing the inliner in our MIR mid-end serves Cranelift, the VM, and a future
  LLVM backend uniformly, since all consume the same optimised MIR.

### Negative

- Upgrading Cranelift or salsa is manual and must be done in lockstep with any
  required code changes; `cargo update` will not do it.
- The MIR mid-end owns optimisations (notably inlining) that an LLVM-only project
  could have delegated to the backend.

### Follow-on work this forces

- **Into the slice:** the `Backend` trait (`jr-codegen`) and the confinement of
  all Cranelift calls to `jr-codegen-clif` must exist from the first native
  binary, and a real inliner must live in `jr-mir` *before* any backend consumes
  MIR.
- **Into wave W8:** the LLVM backend slots in behind the same `Backend` trait,
  reusing the already-inlined MIR.

## Alternatives considered

- **Caret ranges for all dependencies.** Rejected for `cranelift-*` and `salsa`
  specifically: their APIs are not semver-stable, so a caret range is a promise
  the upstream crates do not keep, and it would surface as random build breakage.
- **Relying on Cranelift's optimiser for inlining.** Impossible: Cranelift does
  not inline. The inliner has to be ours regardless, which is why it is placed in
  the shared MIR mid-end.
