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
| [0026](0026-root-marker-order.md) | `.git` before `modules` as the workspace root marker (amends ADR-0025 §1) | Accepted |
| [0027](0027-doc-comments.md) | `///` and `//!` doc comments: trivia kinds plus a side-table query | Accepted |
| [0028](0028-hover-and-completion.md) | The hover card, and completion pulled forward from W9 | Accepted |
| [0029](0029-workspace-discovery.md) | The workspace is the search paths plus the root tree, walked and watched | Accepted |
| [0030](0030-references-and-rename.md) | References, rename that refuses rather than half-renames, and symbols | Accepted |
| [0031](0031-code-actions-and-hints.md) | Code actions from diagnostics, an unused-import warning, signature help, inlay hints | Accepted |
| [0032](0032-write-before-queue.md) | Every write before the snapshot; a cancelled publish must be re-queued (amends ADR-0024 §2) | Accepted |
| [0033](0033-latency-measurement.md) | `jr bench`: latency measured in three cache regimes, because a benchmark harness would measure the memo | Accepted |
| [0034](0034-no-reverse-index.md) | No reverse index: the reference scan is 99% parsing, 1% searching (closes ADR-0030's reservation) | Accepted |
| [0035](0035-import-navigation.md) | An `#import` line navigates to its module, from anywhere on the line | Accepted |
| [0036](0036-no-vscode-extension.md) | No VS Code extension; Neovim is the supported editor (amends §1.4's first criterion) | Accepted |
| [0037](0037-numeric-tower-and-cast.md) | The integer tower is a naming change; `cast(T, x)` is Jai's, checked at comptime | Accepted |
| [0038](0038-negative-literals.md) | A leading `-` on a literal is folded during lowering, so a signed minimum is writable | Accepted |
| [0039](0039-fixed-arrays-and-bounds-checks.md) | `[N]T` fixed arrays, and the explicit `bounds_check` op ADR-0003 asked for in the slice | Accepted |
| [0040](0040-floating-point.md) | `float32`/`float64` are plain IEEE-754 with no traps; `Convert` carries a `NumKind` | Accepted |
| [0041](0041-enums.md) | `enum` is a nominal type whose members are namespaced; bare `.RED` is deferred with a plan | Accepted |
| [0042](0042-bitwise-operators.md) | Bitwise binds tighter than comparison (unlike C); an out-of-range shift traps | Accepted |
| [0043](0043-enum-flags.md) | `enum_flags` numbers by powers of two; a combination is a value, not a member | Accepted |
| [0044](0044-array-views.md) | `[]T` is a `{data, count}` pair; an array converts to one only via explicit `buf[]` | Accepted |
| [0045](0045-unions.md) | `union` is untagged — a tag with no pattern matching to read it is cost without benefit | Accepted |
| [0046](0046-autocast-and-bare-enum-members.md) | `xx` and bare `.RED` are one idea: the context supplies what the source omits | Accepted |
| [0047](0047-imported-enum-members-and-refused-bodies.md) | An enum member is found through its *type*; a refused body warns, and running one fails | Accepted |
| [0048](0048-operator-overloading.md) | `operator +` is a constant whose name is an operator; one operand must be declared locally | Accepted |
| [0049](0049-for-labels-and-defer.md) | `for` iterates three known shapes; a label names a loop; `defer` runs at every scope exit | Accepted |
| [0050](0050-using.md) | `using` promotes a struct's fields into scope; a real local always wins, silently | Accepted |
| [0051](0051-aggregate-returns.md) | An aggregate is returned through a caller-allocated `sret` pointer, uniformly by size | Accepted |
| [0052](0052-multiple-return-values.md) | `-> (s64, bool)` is a structural results aggregate; `_` discards a result positionally | Accepted |
| [0053](0053-named-and-default-arguments.md) | A named argument matches a parameter name; a default must be a literal | Accepted |
| [0054](0054-scope-visibility.md) | `#scope_module` hides what follows it from importers; export is the default | Accepted |
| [0055](0055-imported-constants.md) | An imported constant`s value crosses the boundary the way a callee does | Accepted |
| [0056](0056-float-constants-are-not-integers.md) | A compile-time float result is interned as a float, not as an integer | Accepted |
| [0057](0057-implicit-context.md) | `context` is a real hidden parameter, leading rather than trailing (amends ADR-0001) | Accepted |
| [0058](0058-bounds-check-build-setting.md) | The bounds-check build setting, and `#no_abc` on a procedure (amends ADR-0003) | Accepted |
| [0059](0059-indirect-calls.md) | A procedure is a value you can call through a pointer | Accepted |
| [0060](0060-null-and-a-memory-source.md) | `null` is a context-typed pointer literal, and `malloc`/`free` reach libc | Accepted |
| [0061](0061-vm-malloc-from-its-own-region.md) | The VM satisfies `malloc`/`free` from its own region (corrects ADR-0060 §4) | Accepted |
| [0062](0062-the-allocator-protocol.md) | `context.allocator` is a struct of procedure pointers | Accepted |
| [0063](0063-push-context.md) | `push_context` gives a block its own copy of the context (amends ADR-0057 §2) | Accepted |
| [0064](0064-pointer-arithmetic.md) | Pointer offset (`p + n`, `p - n`) is element-scaled, unchecked, and lowers to an indexed address | Accepted |
| [0065](0065-temporary-storage.md) | Temporary storage is a lazily-allocated bump arena in two context fields | Accepted |
| [0066](0066-trap-backtraces.md) | A trap reports the call chain of the frames that still exist | Accepted |
| [0067](0067-switch-and-exhaustiveness.md) | `switch` with exhaustiveness from the pool, and W4.5 moves before W4 (amends §2.1's order) | Accepted |
| [0068](0068-tagged-variants.md) | `variant` is a tagged union with a checked read, destructured by `switch` (completes W4.5) | Accepted |
| [0069](0069-run-across-files-and-in-a-body.md) | A `#run` may call an imported procedure and appear in a body; W4 is split into sub-waves | Accepted |
| [0070](0070-array-length-from-a-constant.md) | An array length may name a literal-valued constant (amends ADR-0039 §3a) | Accepted |
| [0071](0071-type-values.md) | A type is a compile-time value; using one at run time is refused | Accepted |
| [0072](0072-insert.md) | `#insert` of a literal string, lowered where it is written | Accepted |
| [0073](0073-insert-computed-operand.md) | `#insert` of a computed string; the cycle broken by a narrow pre-pass, not by salsa's fixed-point recovery | Accepted |
| [0074](0074-aggregate-constants.md) | An aggregate compile-time value, interned field-wise rather than as a target-specific byte image | Accepted |
| [0075](0075-type-info.md) | `type_info` returns a `Type_Info` declared in `Basic`; a constant may hold a string (corrects ADR-0074's closing claim) | Accepted |
| [0076](0076-any.md) | `Any` is a `{type, pointer}` pair; a pointer converts to `*u8` only where a type is erased, and the read back is checked | Accepted |
| [0077](0077-type-info-id.md) | `Type_Info` gains a stable `id` (the pool id) so a type has a runtime identity for `any_as` to check (amends ADR-0075 §3) | Accepted |
| [0078](0078-type-info-per-kind.md) | `Type_Info` gains fixed-size per-kind facts (`count`, `element`); the variable-length field list stays deferred (amends ADR-0075 §3) | Accepted |
| [0079](0079-no-pointers-in-compile-time-aggregates.md) | A pointer or view in a compile-time aggregate is refused — it addressed the evaluator's memory and silently miscompiled (completes ADR-0074 §2) | Accepted |
| [0080](0080-code.md) | `#code { … }` is unquoted source that splices, and it is sugar over `#insert` — no `Code` value, declined until something can inspect a tree | Accepted |
| [0081](0081-polymorphic-parameter.md) | A single `$T` parameter, inferred from the call and instantiated structurally — sub-wave 1 delivers the surface, refusing a call (E0268) pending instantiation | Accepted |
| [0082](0082-instantiation.md) | A polymorphic call instantiates by expanding the HIR with a substituted procedure per structural key, checked and lowered per instantiation (lifts E0268) | Accepted |
| [0083](0083-multiple-type-variables.md) | A polymorphic procedure may introduce several type variables; the structural key is the tuple of all bindings (generalises ADR-0082) | Accepted |
| [0084](0084-nested-inference.md) | A type variable is inferred through a pointer or view parameter (`*$T`, `[]$T`) by a one-layer structural match | Accepted |
| [0085](0085-polymorphic-structs.md) | A polymorphic struct is a parameterised type keyed on `(decl, args)`; design of record for the sub-wave that builds it | Accepted |
| [0086](0086-polymorphic-structs-implementation.md) | Polymorphic structs as built: staged (zero-diff representation, then behaviour), a second instance-keyed field map, and every ADR-0085 §5 deferral held as a compile-time refusal | Accepted |
| [0087](0087-comptime-value-parameter.md) | A comptime-value parameter `$N: s64` — the surface (parses, lowers, body type-checks); a call is refused by design (E0271) pending the instantiation half | Accepted |
