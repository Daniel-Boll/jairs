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
| [0088](0088-comptime-value-instantiation.md) | A comptime-value call is instantiated by evaluating the argument via the acyclic const-eval pre-pass and baking it into a clone; design of record, implementation deferred (a re-resolve gap to settle first) | Accepted |
| [0089](0089-array-length-from-comptime-param.md) | An array length may name a `$N` comptime parameter, read from the instantiation's baked value; a template's own `[N]T` gets a placeholder whose length-dependent checks are withheld | Accepted |
| [0090](0090-expand-macros.md) | `#expand` marks a macro whose body a call splices into the caller's scope; the surface ships with a by-design refusal (E0272) because an accepted-and-ignored directive is worse than a rejected one | Accepted |
| [0091](0091-expand-splice.md) | A `#expand` macro call splices the macro's body into the caller's scope — a generated prelude binds each argument once, a tail `return` assigns a result local; an early `return` (E0273) and a cross-file call (E0272) are refused | Accepted |
| [0092](0092-reflect-a-bound-type.md) | `type_info(T)` describes a bound type variable — bindings consulted first and seeded per body, withheld in a template, and an instantiation's `Type_Info` folded against its own check; unblocks `#modify` | Accepted |
| [0093](0093-modify-predicate.md) | `#modify { … }` is a compile-time predicate over an instantiation; the surface ships with a by-design refusal (E0274) because a parsed-and-ignored predicate would accept calls the author rejected | Accepted |
| [0094](0094-modify-predicate-lowering.md) | A `#modify` predicate is lowered as an ordinary synthetic procedure and cloned per instantiation with its bindings; **amends ADR-0093 §2**, whose stated blocker did not exist | Accepted |
| [0095](0095-modify-evaluation.md) | A `#modify` predicate runs at compile time in `file_mir` (the only host with the expanded tree, its MIR and the VM); a `false` refuses the instantiation with E0275 and E0274 is retired | Accepted |
| [0096](0096-bake-arguments.md) | `#bake_arguments` is a partial application producing a specialised procedure — a clone with the baked parameters dropped, reusing ADR-0088's mechanism; the surface ships with a by-design refusal (E0276) that replaces a leaked "please report it" gap message | Accepted |
| [0097](0097-bake-arguments-specialisation.md) | A `#bake_arguments` declaration produces a specialised procedure — a clone with the baked parameters dropped, substituted and remapped (ADR-0088 §3's steps, reused); a baked value must be a literal because const-eval runs after lowering. **W5 closes** | Accepted |
| [0098](0098-notes.md) | `@note` attaches metadata to a declaration for a metaprogram to read — its own node kind, since a note is data while the directives are instructions. **W6 — Metaprogram opens** | Accepted |
| [0099](0099-note-reader.md) | `has_note` / `note_value` read a declaration's notes at compile time — folded in sema with no VM, taking the declaration itself so a misspelling is an error rather than a silent `false`. §4 refuses `==` on an aggregate (E0278), a leaked ICE found by probing | Accepted |
| [0100](0100-note-query.md) | `noted_count` / `noted_name` query a file's noted declarations — folded, so both arguments must be literals; §2 states the honest limit (no `for` loop without a compiler-emitted table) and names the wave that lifts it | Accepted |
| [0101](0101-note-driven-codegen.md) | `noted_insert` generates code for every noted declaration — the metaprogram loop lives *inside the fold*, so generation needs no static-data table; §2 corrects ADR-0100 §2, which deferred generation and inspection as one thing; §3 fixes a stale `ExprId`-keyed fold, a latent miscompile a verifier panic exposed | Accepted |
| [0102](0102-build-script-output.md) | A build script names its own artefact — `BUILD_OUTPUT`, a declared constant (not an order-dependent intrinsic call) that `jr build` reads through `file_consts`; `-o` still wins | Accepted |
| [0103](0103-string-module.md) | `String` is a module of **non-allocating** byte operations — W7 opens. It exists because ADR-0099 §4 refused `==` on two strings and named a byte loop as the fix; its own module so two-module imports are finally exercised | Accepted |
| [0104](0104-sort-module.md) | `Sort` orders a view given a comparison — insertion sort for stability and no allocation. §1 and §2 fix **two leaked internal errors** in cross-file polymorphism that writing a library found: an imported procedure used as a value, and an imported template call (now E0268) | Accepted |
| [0105](0105-array-module.md) | `Array` is a **fixed-capacity** array, and three probed refusals decided that rather than effort: typed allocation is unreachable (E0232), inference through a parameterised struct is deferred, and such a struct cannot cross a module boundary (E0269) — so a polymorphic one would be unusable by every importer | Accepted |
| [0106](0106-typed-allocation.md) | `size_of` / `typed` / `untyped` make heap storage reachable **without widening `cast`** — the target type is a type argument at a searchable boundary, not an assertion buried in an expression. §2 fixes a pre-existing store-to-load forwarding miscompile only this could reach | Accepted |
| [0107](0107-growable-list.md) | `List` is a genuinely growable array. §2 fixes a **VM miscompile** — `malloc` shared the frame bump cursor, so heap memory allocated in a callee was reclaimed on return — which made the two engines disagree: the corpus differential's first real catch | Accepted |
| [0108](0108-module-diagnostics.md) | A program's diagnostics are **every reachable file's** — a root whose imported module was broken used to check clean and fail inside an engine. Each diagnostic keeps the module's own span, because attributing it to the `#import` discards the line to fix | Accepted |
| [0109](0109-view-from-pointer.md) | `view(p, n)` builds a `[]T` from a pointer and a count, so `sort_ints(elements(*l))` sorts a growable list in place — the library composes. It revisits ADR-0044 §4, whose stated reason had expired; §2 fixes a sixth leaked gap report (a view returned by value had no place) | Accepted |
| [0110](0110-null-call-traps.md) | Calling a **null procedure pointer** traps in both engines — it leaked an internal error naming an arity nobody wrote (the VM decoded null to file 0 proc 0, an arbitrary real procedure). The VM handle is biased by one so zero means null; `valid/048` proved the bias necessary | Accepted |
| [0111](0111-string-allocating-half.md) | `String`'s allocating half — `concat` / `substring` / `to_upper` / `to_lower` / `free_string` — allocates through `context.allocator` and the caller frees. Not `talloc` (a result expiring on an unrelated reset is a trap) nor an explicit parameter (the context carries it). Settles ADR-0103 §3's deferred fork | Accepted |
| [0112](0112-math-module.md) | `Math` ships the **exact closed-form** functions only — `abs`, `min`, `max`, `sign`, `clamp`, `pow`, `gcd`, `floor`/`ceil`/`round` — because a float cannot cross the FFI boundary yet, so libm is unreachable and an approximation could make the two engines disagree on the last ulp | Accepted |
| [0113](0113-random-module.md) | `Random` is a caller-owned xorshift64 generator — state a caller threads, so a sequence is reproducible in both engines. §3 records a language gap it surfaced: a `u64`-range named constant has no `name : T : value` form, so it needs `#run` of a typed procedure | Accepted |
| [0114](0114-ffi-floats.md) | A float may cross the **FFI boundary** — passed in a float register (xmm0/d0), not as a word, in both engines. libffi is told the arg/return is a float; native uses an F32/F64 AbiParam. Unblocks Math's transcendentals as a libm wrap | Accepted |
| [0115](0115-math-transcendentals.md) | `Math`'s transcendentals (`sqrt`, `sin`, `cos`, `exp`, `ln`, `powf`) are **libm wraps**, now that ADR-0114 let a float cross the FFI boundary — correctly rounded and identical in both engines because both call the same libm. Closes ADR-0112's deferred item | Accepted |
| [0116](0116-hash-map.md) | `Int_Map` is an open-addressed hash table (linear probing, tombstones, 3/4-load growth) — a heap array of structs. §2 fixes a comptime miscompile it surfaced: the wrapping operators computed in i128 and overflowed, the second engine divergence the differential caught | Accepted |
| [0117](0117-cross-file-parameterised-structs.md) | A **parameterised struct may cross a module boundary** — the *importer* resolves its fields, from the declaring file HIR, because a TypeRef indexes that file arena. Identity stays the declaring file. Unblocks generic Map/List/Array, named by three library sub-waves | Accepted |
| [0118](0118-generic-containers.md) | The containers become **generic structs with concrete procedures** — half a conversion, because inference through a parameterised struct is still deferred. `Map` stays concrete: an intrinsic type argument is not parsed in type position, a fourth small named unblocker. §4 closes two more unused-import traps | Accepted |
