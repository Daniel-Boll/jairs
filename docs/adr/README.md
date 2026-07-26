# Architecture Decision Records

An **Architecture Decision Record** (ADR) captures a single significant,
hard-to-reverse decision: the forces that pressed on it, the choice made, and
the consequences that choice forces onto the rest of the system. It is a record,
not a proposal — an ADR is written once the decision is made, and it is
*immutable*. If a later decision overturns an earlier one, we write a new ADR
that supersedes it rather than editing history.

Add an ADR when a decision (a) shapes the architecture, an interface, or a data
representation; (b) is expensive to undo later; and (c) a future contributor
would otherwise reasonably ask "why on earth is it done this way?". Small,
local, easily-reversed choices do not need an ADR. Everything in the table below
came out of [`PLAN.md`](../../PLAN.md) §0.1 — the resolved design questions — and
each is load-bearing for a specific crate, wave, or IR representation.

The ADR numbers are stable and are referenced from code comments and
`Cargo.toml`. **Do not renumber.**

## Index

| ADR | Title | Status |
|---|---|---|
| [0001](0001-implicit-context-hidden-parameter.md) | Implicit context is a hidden trailing parameter | Accepted |
| [0002](0002-integer-overflow-traps.md) | Integer overflow always traps | Accepted |
| [0003](0003-bounds-checks-build-setting.md) | Bounds checks are a build setting | Accepted |
| [0004](0004-string-representation.md) | Strings are `{data, count}` and not NUL-terminated | Accepted |
| [0005](0005-structural-polymorph-identity.md) | Polymorph instantiation identity is structural | Accepted |
| [0006](0006-comptime-ffi.md) | Compile-time code may call foreign functions | Accepted |
| [0007](0007-salsa-single-frontend.md) | salsa from the first slice; the LSP is a query consumer | Accepted |
| [0008](0008-error-handling-effect-row-slot.md) | Error handling is Jai's, with a reserved effect-row slot | Accepted |
| [0009](0009-pinned-cranelift-salsa.md) | `cranelift-*` and `salsa` are pinned with `=` | Accepted |
| [0010](0010-handwritten-parser-separate-treesitter.md) | Hand-written compiler parser; tree-sitter is editor-only | Accepted |
| [0011](0011-postfix-deref-prefix-address-of.md) | Dereference is postfix `.*`; address-of is prefix `*` | Accepted |
| [0012](0012-procs-and-structs-are-constants.md) | Procedures and structs are constants | Accepted |
| [0013](0013-hir-spans-defer-astidmap.md) | HIR nodes carry spans; `AstIdMap` is deferred | Accepted |
| [0014](0014-module-resolution.md) | Module resolution: search paths, flat imports, cycles are legal | Accepted |
