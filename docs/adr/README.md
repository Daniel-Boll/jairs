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
| [0015](0015-type-identity.md) | Type identity: nominal structs, distinct `string`, interned `void` | Accepted |
| [0016](0016-jairs-0-typing-rules.md) | Jairs-0 typing rules: context-typed literals, deferred `#run` | Accepted |
| [0017](0017-mir-shape.md) | MIR shape: block parameters, SSA at construction, poison refused | Accepted |
| [0018](0018-vm-shape.md) | VM shape: register bytecode, layout in the pool, const-eval as a query | Accepted |
| [0019](0019-native-backend-shape.md) | Native back end: three-phase `Backend`, traps via a runtime helper, interned foreign library | Accepted |
| [0020](0020-trap-source-locations.md) | A trap names its source location; one formatter in `jr-base` decides how | Accepted |
| [0021](0021-inliner-and-optimized-mir.md) | The inliner, a staged `optimized_file_mir`, and the `#run` closure it must not touch | Accepted |
| [0022](0022-dce-constprop-shared-arithmetic.md) | DCE and const-prop, ADR-0002's arithmetic shared in `jr-pool`, and a bounded fixed point | Accepted |
| [0023](0023-store-to-load-forwarding.md) | Store-to-load forwarding: block-local, identical paths, and no layout | Accepted |
| [0024](0024-language-server.md) | The language server: a worker snapshot, a span scan, negotiated positions | Accepted |
| [0025](0025-editor-integration.md) | Editor integration as a runtimepath directory, verified rather than gated | Accepted |
