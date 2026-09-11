# The Way to Jai compatibility inventory

Inventory date: 2026-09-09

## Scope and evidence

This inventory compares Jairs with the locally vendored
[`The_Way_to_Jai`](../../references/The_Way_to_Jai/README.md) tutorial and example
corpus. The submodule is pinned to
`19cb4b7acb0de2798c769f9ad73313a4d15f4056` (2026-06-11).

The source is useful as a broad compatibility target, but it is not the
canonical specification of unpublished Jai. Its README calls the book a work in
progress, says it grew from personal notes and community material, and only
specifically identifies the text and code through section 8 as tested against
Jai beta `0.2.024` (2025-12-31). The stronger README statement that all examples
work with the latest compiler cannot be verified from this repository because
the Jai compiler and its standard modules are not vendored.

The comparison therefore uses this evidence order:

1. Executable Jairs declarations and tests in `modules/`, `crates/`, and
   `tests/corpus/`.
2. The current handoff and implementation inventory in
   [`PLAN.md`](../../PLAN.md) and
   [`docs/capabilities.md`](../capabilities.md).
3. Tutorial prose and `.jai` examples in the pinned submodule, as secondary
   evidence of desired source shapes and feature families.

No row below means that a guide example compiles unchanged. The status applies
to the feature family:

- **Present** — the family is usable end to end in Jairs, although names or
  command-line spelling may differ.
- **Partial** — a useful subset exists, but an example needs source changes or
  depends on a missing subfeature.
- **Absent** — no current Jairs language or standard-library facility covers the
  central feature.
- **Intentionally divergent** — Jairs has an explicit design decision or
  architecture that does not attempt the guide's Jai shape.

### Evidence conflicts corrected during this audit

The live source outranked several stale statements, corrected alongside
ADR-0217:

- `docs/capabilities.md` listed `#must` as absent, while
  [`tests/corpus/valid/120-must.jr`](../../tests/corpus/valid/120-must.jr),
  [`modules/File/module.jr`](../../modules/File/module.jr), and parser/sema
  source implement it.
- The same table listed a `#c_call` procedure-pointer type as absent, while
  [`modules/Thread/module.jr`](../../modules/Thread/module.jr),
  [`crates/jr-syntax/src/ast.rs`](../../crates/jr-syntax/src/ast.rs), and
  [`crates/jr-sema/src/ctx.rs`](../../crates/jr-sema/src/ctx.rs) use it.
- One capabilities row said array literals were absent, while ADR-0194, parser
  source, and the `T.[...]` syntax recorded in `PLAN.md` implement typed array
  literals.
- [`docs/spec/00-overview.md`](../spec/00-overview.md) said that nothing works.
  It now labels that text as the historical tracer-bullet boundary rather than
  current status.
- Some module headers retain historical blockers after the declarations below
  them evolved. For example, `modules/Array` discusses an unusable cross-file
  polymorphic struct but currently declares `Array :: struct($T)`.

The corrections do not make any prose inventory generated ground truth. The
executable source and gates still outrank every hand-maintained table.

## Findings at a glance

- The prose matrix has **59 feature-family rows** covering all 60 Markdown book assets
  (`01A`/`01B` share one row): **2 present, 42 partial, 8 absent, and 7 intentionally
  divergent**. “Partial” dominates because Jairs usually has the core mechanism but not
  the guide's complete spelling, inference, library surface, or platform breadth.
- Jairs already covers much of the guide's procedural core: explicit memory,
  numeric types, pointers, structs/unions/enums, loops, `defer`, operator
  overloading, multiple returns plus `#must`, compile-time execution, basic
  reflection, macros, FFI, native compilation, threads, files, processes, and a
  graphics subset.
- The largest remaining source-compatibility multipliers are general procedure
  overloading, inferred array literals, imported/call-backed type-level constants,
  `#load` and item-level `#if`, richer polymorphic inference, and the missing
  metaprogramming model around first-class `Code`.
- `String_Builder` is now present in `Basic`: zero-value and explicit
  initialization, captured-allocator chained buffers, string/pointer-length/byte
  append, unbounded formatted append, length, copy conversion and idempotent
  cleanup. Exact guide-compatible overloads and destination-allocator parameters
  still depend on language work or primary Jai evidence.
- Standard-library breadth is less complete than language breadth. The guide
  assumes a large Jai distribution containing `Preload`, `Basic`, `String`,
  `POSIX`, `Compiler`, graphics/audio modules, pools, hash tables, testing
  modules, a bindings generator, and plugins. Jairs has 25 focused modules, but
  many signatures and ownership conventions differ.
- Some differences are explicit decisions, not backlog: one-file modules make
  `#scope_file` redundant; build-message polling, Jai workspaces, plugin hooks,
  and a packaged VS Code extension have been declined; Jairs' generated build
  source is a `Build` module rather than shared global scope.
- ADR-0219 turns the first parity baseline into a required test: at least one owned probe
  for each of the guide's 35 example-bearing top-level groups. ADR-0227 adds a second
  chapter 17 probe and ADR-0228 a second chapter 11 probe, for 37 total. Six selected
  examples check unchanged at the pinned revision; every other row records a
  runnable port, an exact blocker, or an intentional divergence. The submodule
  remains provenance only, and this representative set does not support a
  percentage claim.

## Coverage accounting: what was assessed and what is executable

The pinned reference contains **42 numbered groups**, **60 Markdown chapters**, one
ASCII-art PDF, and **315 `.jai` examples**. The chapter matrix above is the full-book
assessment layer. The executable layer is intentionally smaller:

| Executable probe result | Count | Meaning |
|---|---:|---|
| Source-compatible | 6 | A selected pinned example checks without a Jairs-side source port |
| Ported | 19 | The behavior is expressible with documented spelling/API changes |
| Blocked | 9 | The representative pins an exact current compiler diagnostic |
| Divergent | 2 | Jairs deliberately chooses a different architecture or product boundary |

Those 37 probes cover every **example-bearing top-level group** selected by ADR-0219.
They do not cover every numbered group:

- `00` and `01` are narrative/philosophy, so executable coverage would be artificial.
- `02` (development environment and compiler CLI) is materially under-tested: command
  contracts, project discovery, help text, and option differences need explicit CLI tests.
- `32` (processes) is materially under-tested: capture, environment, working directory,
  timeout, stdin/stdout and portable spawn behavior need contract tests.
- `36` (plugins) is an intentional divergence and needs a permanent executable/documented
  refusal boundary rather than a pretend implementation.
- `37` (testing) is materially under-tested: Jairs has Rust-side tests but no in-language
  `#assert`, test discovery, runner, or testing module matching the guide.
- `65` is ecosystem/community material rather than one compiler feature.
- `05C` is an ASCII table PDF and carries no independent language capability.

One probe is also too weak for broad chapters. The highest-priority families for
multi-probe coverage are modules/loading (8), structs (12), procedures (17), arrays
(18), polymorphism (22–23), metaprogramming (26), builds (30), and concurrency (31).
Until those are split into family-level contracts, “chapter covered” means only that
one representative outcome is pinned.

## Chapter-by-chapter matrix

| Guide chapter | Feature family and concrete source | Status | Jairs evidence and compatibility gap |
|---|---|---:|---|
| 00 — Preface | Tutorial goals in [`book/00A_Preface.md`](../../references/The_Way_to_Jai/book/00A_Preface.md) | **Intentionally divergent** | Jairs is a separate Jai-inspired language, not a distribution or reimplementation claim. Compatibility must be stated per feature, never inherited from the tutorial's Jai claims. |
| 01A/01B — What is Jai? | Language philosophy and feature survey; [`book/01B_What_is_Jai_-_more_in_depth.md`](../../references/The_Way_to_Jai/book/01B_What_is_Jai_-_more_in_depth.md) | **Intentionally divergent** | Jairs shares no-GC, explicit-cost, compile-time-execution goals, but has its own VM, Cranelift/LLVM pipeline, error model, module system, and accepted ADRs. |
| 02A — Development environment | Jai toolkit installation and editor setup | **Intentionally divergent** | Jairs installs with Cargo, bundles its modules, and ships Neovim/Zed plus an editor-neutral LSP. It explicitly declines a VS Code extension. |
| 02B — Compiler CLI | [`book/02B_Compiler_command_line_options.md`](../../references/The_Way_to_Jai/book/02B_Compiler_command_line_options.md) | **Partial** | `jr check/run/build/fmt/parse/lsp/bench/new/init` cover ordinary workflows. Jai flags and metaprogram options are not command-line compatible. |
| 03 — First program | `main`, printing, runtime and `#run`; [`examples/03/3.2_hello_sailor_comptime.jai`](../../references/The_Way_to_Jai/examples/03/3.2_hello_sailor_comptime.jai) | **Partial** | Jairs has `main`, `print`, `exit`, `jr run`, native builds, and substantial `#run`. Formatting syntax is similar, but runtime startup and CLI behavior differ. |
| 04A — Compiler architecture | Front end, bytecode, back ends, linking, debug/release | **Partial** | Jairs has an error-recovering front end, bytecode VM, Cranelift, LLVM 21, linker driver, `-O0/-O1`, and DWARF. It has no Jai X64 backend, no `-O2`, and no exact debug/release option set. |
| 04B — Code from command line | Jai `-run` and `-add`; [`book/04B_Options_for_giving_code_at_the_command-line.md`](../../references/The_Way_to_Jai/book/04B_Options_for_giving_code_at_the_command-line.md) | **Absent** | Jairs has source `#run`, `#insert`, build strings, and stdin formatting, but no CLI that evaluates or injects arbitrary source text into an ordinary compilation. |
| 04C — Preload | Implicit `Preload`, intrinsics, `memcpy/memcmp/memset`; [`examples/04/4.2_intrinsics.jai`](../../references/The_Way_to_Jai/examples/04/4.2_intrinsics.jai) | **Intentionally divergent** | Jairs compiler intrinsics and `modules/Basic` supply the substrate without a source-visible `Preload` module or `#intrinsic` declarations. Aggregate copies may lower to `memcpy`, but there is no matching user-facing intrinsic surface. |
| 04D — Memory management | Stack, heap, explicit ownership | **Present** | Jairs has explicit stack locals, `malloc/free`, context allocators, temporary storage, no GC, no RAII, and `defer`. Ownership remains a library/documentation contract. |
| 04E — Startup | Jai runtime support, argc/argv, first context | **Intentionally divergent** | Jairs back ends emit their own entry shim and initialize a default context. It has `#program_export` and `#c_call`, but no source-level `Runtime_Support` startup module or ordinary-program argument API. |
| 05A — Declarations and primitives | Constants, variables, swaps, printing; [`examples/05/5.3_variable_declarations.jai`](../../references/The_Way_to_Jai/examples/05/5.3_variable_declarations.jai) | **Partial** | Numeric/string/bool literals, `::`, `:=`, typed declarations, multiple assignment, `xx`, and formatted printing exist. Unicode is byte-oriented and the full Jai formatting vocabulary is not present. |
| 05B — Identifier backslashes | [`examples/05/5B.ident_back.jai`](../../references/The_Way_to_Jai/examples/05/5B.ident_back.jai) | **Absent** | No Jairs syntax or lexer support was found for escaped/backslashed identifiers. |
| 06A — Booleans and numbers | Arithmetic, casts, bitwise, assert, random, math | **Partial** | The numeric tower, casts, autocast, trapping/wrapping arithmetic, bitwise operations, floats, `Random`, and `Math` exist. `#assert`, float `%`, `is_nan`, broad format controls, and much of Jai's math surface do not. |
| 06B — Time and dates | [`examples/06/6B/6B.2_measuring_time.jai`](../../references/The_Way_to_Jai/examples/06/6B/6B.2_measuring_time.jai) | **Partial** | `modules/Time` provides monotonic/wall nanoseconds and conversions. Calendar/date formatting, time zones, and sleep are absent. |
| 07 — Scope | Globals, locals, blocks, shadowing; [`examples/07/7.2_shadowing.jai`](../../references/The_Way_to_Jai/examples/07/7.2_shadowing.jai) | **Present** | Jairs has block scope, shadowing, file globals, local constants, and nested procedures. Imported global access is more restricted. |
| 08A — Modules and loading | `#import`, `#load`, named imports, module parameters; [`examples/08/8.2_main.jai`](../../references/The_Way_to_Jai/examples/08/8.2_main.jai) | **Partial** | Bare and aliased `#import`, manifests, exact dependencies, module catalogs, and search paths exist. `#load`, import-from-string/dir semantics, `#module_parameters`, nested modules, and Jai's workspace model are absent. |
| 08B — Scope directives | `#scope_file`, `#scope_module`, `#scope_export`; [`examples/08/8B/8.2_file_and_global_scopes.jai`](../../references/The_Way_to_Jai/examples/08/8B/8.2_file_and_global_scopes.jai) | **Partial** | `#scope_module` and `#scope_export` work. `#scope_file` is intentionally omitted because one Jairs module is one file, so file scope and module scope are identical. |
| 09 — First-class types and `Any` | [`examples/09/9.1_types.jai`](../../references/The_Way_to_Jai/examples/09/9.1_types.jai) | **Partial** | Type constants, `size_of`, `type_of`, `type_info`, `Any`, `any_of`, and `any_as` exist. `Type` as an annotation/parameter, type-value chains, implicit bare-value-to-`Any`, and full type comparison/reflection do not. |
| 10 — Pointers | Address, dereference, pointer chains, null, casts | **Partial** | Typed pointers, `null`, address/deref, unchecked element-scaled indexing and compound offsets, same-type signed pointer difference, `typed/untyped`, and FFI pointers exist. Ordering and general pointer casts are absent or deliberately constrained. |
| 11 — Allocation and `defer` | [`examples/11/11.4_memory.jai`](../../references/The_Way_to_Jai/examples/11/11.4_memory.jai) | **Partial** | Default context allocation, `malloc/free`, `New(T)`, typed allocation helpers, and `defer` exist. `New` uses the active context allocator, zeroes success, preserves null failure, and has no Jai-style named allocator argument; allocator modes and the exact Basic allocation API remain absent. |
| 12 — Structs | Literals, recursive/anonymous structs, `#as`, alignment, member procs, parameters | **Partial** | Named/nested structs, typed and context-inferred named/positional/empty literals, heap allocation, representation-indirect recursive structs, `using`, polymorphic structs, `#align`, and `#place` exist. Inline representation recursion, anonymous structs, `#as`, member procedures as a language feature, struct-level packing, and several parameter forms are absent. |
| 13 — Unions and enums | [`examples/13/13.3_enum_flags.jai`](../../references/The_Way_to_Jai/examples/13/13.3_enum_flags.jai) | **Partial** | Untagged unions, tagged variants, enums, enum flags, explicit values, bare contextual members, and exhaustive switching exist. Anonymous enums, `#specified`, enum methods through general overloading, richer casts, and some cross-file member use remain absent. |
| 14 — Branching | `if`, `then`, `ifx`, if-case, `#complete`, `#through`; [`examples/14/14.3_if_case.jai`](../../references/The_Way_to_Jai/examples/14/14.3_if_case.jai) | **Partial** | `if/else`, optional `then` on one braceless statement, `switch`, and exact `if #complete value == { ... }` work. `ifx`, truthiness for arbitrary values, `#through`, guards, ranges, and switch expressions do not. |
| 15 — Loops and reflection | while/for, reverse, pointer iteration, enum/field iteration, notes | **Partial** | `while`, ranges, arrays/views/strings, `for <`, `it/it_index`, labels, break/continue, and declaration notes exist. `for *item`, user-defined for expansion, enum-value iteration, and general runtime field reflection are absent. |
| 16 — Types in depth | Full `Type_Info`, inheritance queries, runtime reflection | **Partial** | Jairs exposes kind/name/size/alignment/id/count/element and emitted field tables used by `Basic.print`. It does not expose Jai's complete type table, arbitrary runtime type traversal, `#specified` queries, or `#as` inheritance. |
| 17 — Procedures | Defaults/named args, multiple returns, `#must`, overloads, inline, `#this`, reflection, anonymous procs | **Partial** | Local/nested procedures, procedure values, explicit literal defaults (`name: T = literal`), inferred literal defaults (`name := literal`) on ordinary local/imported procedures and their `#run` calls, and declaration-ordered named/default calls on pure `$T`, local `$N`, and local mixed `$T`+`$N` procedures exist alongside several returns, declaration-only labels such as `-> (value: s64, found: bool)`, `#must`, recursion, and operator overloading. A literal may default `$N` itself; defaults must have a fixed type and caller arguments must still infer every `$T` (ADR-0236, ADR-0241). Non-literal defaults, pure-template calls inside `#run`, imported `$N`/mixed calls, procedure-pointer parameter/default metadata, general procedure overloading, unparenthesized/defaulted named results, implicit result bindings, anonymous/lambda procedures, `inline`, `#this`, `#procedure_name`, `#deprecated`, and full procedure reflection do not. |
| 18A — Arrays | Fixed/dynamic arrays, views, literals, variadics, reverse and pointer iteration | **Partial** | `[N]T`, `[]T`, `[..]T`, typed `T.[...]` literals, bounded direct indexing of fixed/view/dynamic arrays, ordinary and reverse loops over their elements, variadic `..T` packing, and local VM-evaluated integer length expressions exist (ADR-0242, ADR-0245). Inferred `.[...]`, multidimensional conveniences, by-reference iteration, broad generic array helpers, dynamic-array slicing, and imported/call-backed evaluated lengths do not. |
| 18B — Ordered removal | [`examples/18/18B_ordered_remove.jai`](../../references/The_Way_to_Jai/examples/18/18B_ordered_remove.jai) | **Absent** | `modules/List` supports growth, push/pop/get/set/clear/free, but no ordered-remove operation or Jai `remove` loop statement was found. |
| 18C — Raw struct copy | [`examples/18/18C/18C_memcpy_struct.jai`](../../references/The_Way_to_Jai/examples/18/18C/18C_memcpy_struct.jai) | **Partial** | Compiler-generated aggregate copies, struct literals, and the example's local evaluated array-length shape exist, but Jairs has no public `memcpy` intrinsic matching the example. |
| 19A — Strings | Strings, builders, operations and C strings; [`examples/19/19.3_string_builder.jai`](../../references/The_Way_to_Jai/examples/19/19.3_string_builder.jai) | **Partial** | Byte strings, direct byte iteration, `#char`, comparisons/search/split/join/replace/trim/case conversion, numeric parsing, C-string conversion, owned copies, freeing, and a chained `Basic.String_Builder` with unbounded formatted append exist. `sprint/tprint`, multiline `#string`, implicit string truthiness, general append overloads, the optional destination allocator, and exact Jai ownership rules remain absent or unconfirmed. |
| 19B — Command-line arguments | [`examples/19/19B/19B.1_command_line_args.jai`](../../references/The_Way_to_Jai/examples/19/19B/19B.1_command_line_args.jai) | **Absent** | Build scripts can read arguments through `modules/Compiler`, but an ordinary Jairs program has no `get_command_line_arguments()` surface and `main` receives no declared argc/argv. |
| 19C — Console input | POSIX/Windows examples in [`book/19C_Get_console_input.md`](../../references/The_Way_to_Jai/book/19C_Get_console_input.md) | **Partial** | The FFI can bind `read`, and `modules/File` exposes descriptor reads, but no portable console-input or POSIX/Windows module matching the guide is shipped. Windows is not a verified target. |
| 19D — Comparing field names | [`examples/19/19C/19C.1_comparing_fields.jai`](../../references/The_Way_to_Jai/examples/19/19C/19C.1_comparing_fields.jai) | **Partial** | Compiler-emitted field metadata exists, but general runtime iteration over `Type_Info_Struct.members` and generic dynamic `[]string` construction do not match the example. |
| 20 — Debugging | Assert, comptime debugger, `#dump`, native debuggers | **Partial** | Jairs has diagnostics, source traps, call chains, MIR snapshots, `jr parse`, real DWARF line/type/stack-local information, and external debugger compatibility. It lacks `#assert`, `#dump`, Jai's interactive comptime debugger, natvis, and complete register-local location lists. |
| 21 — Allocators and temporary storage | [`examples/21/21.1_temp_storage.jai`](../../references/The_Way_to_Jai/examples/21/21.1_temp_storage.jai) | **Partial** | Context allocator/free/data, a working default allocator, `push_context`, `talloc`, and context-installable `Pool`/`Flat_Pool` arenas with reset and bulk cleanup exist. Jai allocator mode structs, `push_allocator`, stack temporary arenas, leak detection, ownership queries, and configurable/aligned temporary storage do not. |
| 22 — Polymorphic procedures | `$T`, lambdas, proc arguments, `#bake_arguments`; [`examples/22/22.6_baked_args.jai`](../../references/The_Way_to_Jai/examples/22/22.6_baked_args.jai) | **Partial** | `$T`, multiple type variables, `$N`, procedure arguments, template calls, instantiation, `$$T`, literal `#bake_arguments`, and named/fixed-default arguments on pure `$T`, local `$N`, and local mixed calls exist; literal defaults may fill `$N` itself (ADR-0241). Lambda `=>`, explicit type arguments, broad two-way inference, cross-file `$N`/mixed instantiation, pure-template calls inside `#run`, non-literal defaults, and several higher-order forms do not. |
| 23A — Polymorphic arrays and structs | Constraints, `#bake_constants`, interfaces; [`examples/23/23.3_poly_structs2.jai`](../../references/The_Way_to_Jai/examples/23/23.3_poly_structs2.jai) | **Partial** | Polymorphic structs, pointer/view inference, `#modify`, and reflected bound types exist. `#bake_constants`, `$T/Base`, interface constraints, recursive generic structures, and `using` on parameterized structs do not. |
| 23B — Struct inheritance showcase | [`examples/23/23B/23B.1_documents.jai`](../../references/The_Way_to_Jai/examples/23/23B/23B.1_documents.jai) | **Absent** | The example's central `#as` inheritance/coercion model is absent, even though field promotion and polymorphism separately exist. |
| 24 — Operator overloading | [`examples/24/24.1_overloading_vec.jai`](../../references/The_Way_to_Jai/examples/24/24.1_overloading_vec.jai) | **Partial** | Binary arithmetic/comparison overloads, cross-module resolution, mixed operand order, and aggregate results exist. Unary, indexing, call, compound assignment, `#symmetric`, and `#poke_name` support do not. |
| 25 — Context | Context extension, allocator scopes, logging, stack trace, print style | **Partial** | Hidden context ABI, `#c_call`, a spellable `#c_call` procedure type, default allocator, temporary storage, backtraces, and `push_context` exist. `#add_context`, `push_allocator`, Jai's logger/print-style fields, and stack-address queries do not. |
| 26A — Metaprogramming | Type table, `#run`, `#if`, `#insert`, `#code`; [`examples/26/26.6_insert.jai`](../../references/The_Way_to_Jai/examples/26/26.6_insert.jai) | **Partial** | `#run`, computed/literal `#insert`, file-scope declaration insertion, `#code` statement splicing, type values, RTTI, and step budgets exist. Item/block `#if`, `#compile_time`, `#no_reset`, first-class `Code`, and compile-time struct construction breadth are absent. |
| 26B — Macros | `#expand`, for expansion, `#modify`, caller scope; [`examples/26/26.22_insert_scope.jai`](../../references/The_Way_to_Jai/examples/26/26.22_insert_scope.jai) | **Partial** | Jairs has `#expand` splicing and `#modify` predicates. It does not have first-class `Code`, `#insert,scope()`, user-defined `for_expansion`, caller-code/location facilities, or the guide's macro reflection model. |
| 26C — Metaprogram applications | SOA, compiler nodes, AST mutation, type variants, code generation | **Partial** | Built-in `#soa`, note-driven generation, compiler-emitted metadata, and source-string insertion cover selected outcomes. Compiler node APIs, AST mutation, `#type` variants, variable-name capture, first-class code-to-string, and arbitrary generated-type loops are absent. |
| 27 — Files and paths | Files, CSV, copy/delete directories, path utilities; [`examples/27/27.1_working_with_files.jai`](../../references/The_Way_to_Jai/examples/27/27.1_working_with_files.jai) | **Partial** | `modules/File` has descriptors, read/write/seek/size/delete and owned whole-file operations; `File_Utilities` has textual path operations. Directory listing/tree mutation, metadata/stat, canonicalization, CSV helpers, and broad filesystem portability are absent. |
| 28 — Inline assembly | `#asm`, registers, SIMD/AVX, asm macros | **Absent** | Jairs has `#simd` and atomics as language operations, but no inline assembly syntax, machine module, register pinning, or assembly feature flags. |
| 29 — C interaction | `#foreign`, libraries, callbacks, per-OS selection, bindings generator; [`examples/29/29.3_c_call.jai`](../../references/The_Way_to_Jai/examples/29/29.3_c_call.jai) | **Partial** | Libraries/frameworks, `#foreign`, aggregates under supported ABI classifications, `#c_call` callbacks/types, dynamic libraries, exported symbols, and per-OS value selection exist. C variadic calls are parsed then refused, Windows is unverified, and `Bindings_Generator` is absent. |
| 30A — Integrated build system | Workspaces, build files/strings, options, placeholders | **Partial** | `modules/Compiler` and the driver build executables, objects, static/dynamic libraries, generated source, commands, paths, two back ends, bounds checks, and optimization levels. Jai workspaces, shared global build strings, `#placeholder`, most Build_Options fields, and exact CLI behavior do not. |
| 30B — Manipulating builds | Message loop, coding rules, bitcode, notes, exports, binary data, bindings | **Partial** | Build scripts, notes, generated modules, custom commands/linking, `#program_export`, and several output kinds exist. The compiler message loop and workspace polling are intentionally refused; LLVM bitcode export, binary embedding, placeholders, and bindings generation are absent. |
| 31 — Threads | Threads, groups, mutexes, channels | **Partial** | `modules/Thread` supports native spawn/join, yield, spin locks, and language atomics with a defined memory model. Thread groups, OS mutex/condition variables, channels, thread results, comptime spawning, and reliable per-thread trap stacks are absent. |
| 32 — Processes | Start, read/write, wait | **Partial** | `modules/Process` supports native `fork/execvp`, wait, status decoding, signals, and run-to-completion. Pipes/output capture, environment control, nonblocking waits, portable spawn, Windows, and VM execution of pointer-rich argv are absent. |
| 33 — Graphics modules | GLFW/SDL/GL/Simp/Input/fonts/textures/audio; [`examples/33/33.2A_simp_window.jai`](../../references/The_Way_to_Jai/examples/33/33.2A_simp_window.jai) | **Partial** | Jairs ships SDL2-backed `Window`, `Input`, `Image`, `GL`, a Simp-shaped renderer, `UI`, and a small `Game` facade. Exact Jai APIs, GLFW, fonts/text, PNG/JPEG, audio, 3D overloads, render targets, and full input state are absent. |
| 34 — Useful modules | Sort, hash table, pools, mail | **Partial** | `Sort`, generic `Hash_Table.Table(K, V)`, `Pool`, `Flat_Pool`, `Map`, `List`, `Bucket_Array`, and `Socket` cover these families through Jairs-adapted APIs. Exact Jai overloads, allocator modes/signatures, ownership details, and the `Mail` module remain absent or unverified. |
| 35 — External modules | Raylib sample/bindings; [`examples/35/35.1_raylib_sample.jai`](../../references/The_Way_to_Jai/examples/35/35.1_raylib_sample.jai) | **Absent** | Jairs can express many fixed-arity C bindings, but ships no Raylib module or package mechanism for this example. C variadics and some ABI cases remain blockers for generated bindings generally. |
| 36 — Plugins | Compiler plugins and plugin distribution | **Intentionally divergent** | Plugin hooks were considered and declined in ADR-0154. Jairs has no plugin ABI or plugin package model. |
| 37 — Testing | Jai testing approach and Stubborn | **Absent** | The Jairs implementation has extensive Rust/corpus/differential tests, but programs written in Jairs have no shipped unit-test framework, test discovery directive, or Stubborn-compatible module. |
| 50 — Guessing game | Console input, random, platform branches | **Partial** | Random numbers, loops, strings, and FFI are available; the example needs porting because console-input helpers, `#if`, and Windows modules are absent. |
| 51 — Game of Life | Arrays, console/graphics, metaprogram helpers | **Partial** | Fixed/dynamic arrays, loops, SDL/OpenGL drawing, and generated code exist in subsets. Evaluated array dimensions, inferred literals, text, and exact graphical APIs block a close port. |
| 52 — Pong | Simp and Raylib versions; [`examples/52/52.1_simp_pong.jai`](../../references/The_Way_to_Jai/examples/52/52.1_simp_pong.jai) | **Partial** | Jairs has its own built-and-run Pong under [`examples/games/pong`](../../examples/games/pong), but its SDL/Simp/Game APIs and input/resource model differ, and no Raylib module exists. |
| 65 — Applications written in Jai | Ecosystem/project catalogue | **Intentionally divergent** | This is ecosystem evidence rather than a language or library contract. Jairs cannot claim application compatibility from project descriptions without source, build inputs, and primary Jai behavior. |

## `String_Builder` compatibility slice

The guide presents this surface in
[`book/19A_Working_with_Strings.md`](../../references/The_Way_to_Jai/book/19A_Working_with_Strings.md),
[`examples/19/19.3_string_builder.jai`](../../references/The_Way_to_Jai/examples/19/19.3_string_builder.jai),
and
[`examples/19/19B/19B.2_clargs_string_builder.jai`](../../references/The_Way_to_Jai/examples/19/19B/19B.2_clargs_string_builder.jai):

- `String_Builder`
- `init_string_builder(*builder)`
- overloaded `append` for `string`, `*u8 + length`, and `u8`
- `print_to_builder(*builder, format, ..Any)`
- `builder_string_length(*builder)`
- `builder_to_string(*builder, allocator, allocator_data, extra_bytes_to_prepend)`
- `free_buffers(*builder)`

ADR-0217 implements the common surface with explicit names where general
overloading is unavailable. The resulting inventory is:

| Dependency | Current state |
|---|---|
| Default and replaceable allocator | **Present** — fresh contexts have allocator/free/data; custom contexts and `push_context` work. |
| Owned string and release path | **Present** — `String.adopt`, `copy_of`-style operations, and `String.free_string`. |
| Byte append substrate | **Present** — the builder appends `string`, `*u8 + count`, and `u8`; the latter two use `append_bytes` and `append_byte` until overloading exists. |
| Cleanup | **Present** — `defer`. |
| Variadic formatting values | **Present** — `..Any` and `Basic.print/format`. |
| Unbounded formatted append | **Present** — `print_to_builder` adds a sink to the existing renderer and bypasses the 4096-byte staging array. |
| Exact overloaded `append` spelling | **Absent** — Jairs supports operator overloading, not general procedure overloading. Explicit names would be a compatibility compromise. |
| Jairs allocation/ownership behavior | **Present and explicit** — builder buffers use the captured allocator triple; conversion copies through the current context allocator; cleanup retains the captured allocator and may be repeated. |
| Exact Jai allocation/ownership behavior | **Unconfirmed** — the secondary guide does not establish whether conversion transfers a bucket/buffer, copies, resets the builder, or which allocations `free_buffers` retains. |

The common guide examples now need no spelling change for string append,
formatted append, length, conversion or cleanup. Exact source compatibility for
byte overloads and the extended conversion parameters still depends on language
work and primary evidence respectively.

## Prioritized dependency order toward broad compatibility

“Full compatibility” needs two separate targets:

1. **Behavioral compatibility** — Jairs can express the same programs with
   documented porting changes.
2. **Source compatibility** — representative pinned `.jai` examples can be
   translated mechanically or accepted with only extension/name changes.

The guide alone is insufficient to certify either against real Jai, but it is
large enough to order the work.

### P0 — Establish executable compatibility probes — done in ADR-0219

[`tests/compatibility/probes.toml`](../../tests/compatibility/probes.toml)
contains at least one representative for each of the 35 example-bearing
top-level chapters: 03–31 except 32, plus 33–35 and 50–52. ADR-0227 adds a
second chapter 17 probe and ADR-0228 a second chapter 11 probe, making 37 total. A strict typed runner
copies each Jairs-owned probe and its fixtures into an isolated temporary
directory, then exercises the real `jr check`, `run`, `build`, or native
build-and-run boundary.

The manifest records the pinned upstream path, compatibility status, earliest
useful classification, reason, and exact observable result. Blockers pin
diagnostic codes rather than prose. Six entries begin `source-compatible`; the
remainder are ports, blockers, or intentional divergences. Ordinary tests never
read the submodule.

This is the prerequisite for later percentage claims, not such a claim itself:
36 representatives establish executable chapter coverage, not support for all
315 guide examples or unpublished Jai.

### P0 follow-up — close the audit's executable blind spots

Before expanding the compatibility percentage or adding more showcase features:

1. Add CLI/project contract tests for chapter 02: help surface, project discovery,
   source injection differences, build/run/check/fmt behavior, and deliberate flag
   incompatibilities.
2. Add process contract tests for chapter 32: arguments, environment, working
   directory, capture, stdin, timeout/termination, and the VM-versus-native boundary.
3. Decide the chapter-37 testing surface: at minimum `#assert`, in-language test
   declarations/discovery, a runner contract, and failure locations.
4. Pin chapter 36 as an explicit plugin divergence unless a concrete consumer justifies
   reversing ADR-0154.
5. Split chapters 8, 12, 17, 18, 22, 23, 26, 30, and 31 into several family probes each.

This follow-up improves the evidence. It is intentionally ahead of graphics breadth:
missing CLI, process, and testing contracts affect ordinary programs and every later
compatibility claim.

### P1 — Finish the high-leverage everyday language surface

1. General procedure overloading. It unlocks guide-shaped `append`, string
   helpers, math APIs, constructors, and many imported modules.
2. Inferred array literals (`.[...]`); struct `T.{...}` / `.{...}` and its field
   initialization rules are now present.
3. Finish evaluated type-level constants for array/SIMD/SOA lengths. Local arithmetic and aliases
   are present (ADR-0245); imported constants and call-backed values remain.
4. `ifx`, fuller `then`/if-case compatibility, and a decision on `#through`.
5. `for *item` and reusable/user-defined iteration.

These features recur in ordinary examples before the advanced metaprogramming
chapters and reduce porting noise across the whole corpus.

### P2 — Land the string and container compatibility layer

1. ~~`String_Builder` append, length, conversion, reset/free, and explicit
   ownership tests.~~ **Done in ADR-0217.**
2. ~~Refactor formatting around a sink abstraction and implement
   `print_to_builder` without a fixed 4096-byte promise.~~ **Done in ADR-0217.**
   `sprint` and `tprint` remain.
3. Generic growable arrays and common operations: overloaded add, remove,
   ordered remove, reset/free, reserve, and typed views.
4. Fill remaining `String` name/shape differences where semantics are known:
   left/right searches, splitting variants, mutable slices, and formatting
   conversions.
5. Add ordinary-program command-line arguments and portable stdin helpers.

This layer makes chapters 18, 19, build scripts, generated code, and many later
examples substantially easier.

### P3 — Close module and conditional-compilation gaps

1. Decide and implement `#load` semantics without creating a second,
   disagreeing module graph.
2. Add item/block conditional compilation or document an equivalent generated
   declaration mechanism that can cover every guide use.
3. Decide `#module_parameters` and import remapping against the existing
   manifest/catalog model.
4. Keep `#scope_file` as an explicit divergence unless multi-file modules are
   introduced.
5. Add ordinary-program argc/argv at the entry shim before claiming startup or
   command-line compatibility.

### P4 — Deepen polymorphism, procedures, and type construction

1. Cross-file template instantiation and stronger bidirectional inference.
2. Mixed type/value comptime parameters and explicit type arguments.
3. Defaulted/implicit named-result semantics, anonymous/lambda procedures, inline controls, and `#this`.
4. Recursive and anonymous structs, parameterized recursive containers, and
   `using` on parameterized structs.
5. Decide `#as` coercion/inheritance and interface constraints.

This is the dependency layer for chapters 22–24 and many library APIs that are
currently concrete or specially named.

### P5 — Decide the metaprogramming compatibility boundary

1. First-class `Code` values and a stable inspectable representation.
2. Caller scope/code/location facilities and safe `#insert,scope()` behavior.
3. User-defined `for_expansion` and macro hygiene rules.
4. `#compile_time`, item-level `#if`, type variants, and compiler-node
   inspection/mutation only after a primary-source contract exists.
5. Expand runtime type information only where a concrete consumer requires it.

This is an architectural decision, not a list of parser directives. Jairs
currently uses source splicing and compiler-emitted tables; adopting Jai's code
tree model would affect syntax, HIR, const evaluation, diagnostics, and tools.

### P6 — Broaden systems and toolchain compatibility

1. Public memory intrinsics and missing allocator utilities, if primary
   evidence confirms their contracts.
2. C variadic calls when backend support permits them; then bindings generation
   and more platform APIs.
3. Directory traversal, metadata, pipes/capture, environment control, portable
   process spawning, mutexes/conditions, channels, and per-thread trap stacks.
4. Expand build options and binary/resource embedding.
5. Keep the message loop/workspace/plugin decisions explicit: reversing an
   intentional divergence requires a new ADR and a demonstrated consumer, not
   compatibility-by-name.

### P7 — Ecosystem breadth

1. Complete the in-language testing module and discovery workflow specified in
   the P0 follow-up.
2. Add a package/module route for external bindings such as Raylib.
3. Complete held input, textures, PNG/JPEG, text/fonts, audio, and graphics
   resource destruction.
4. Consider pools, mail, advanced hash tables, and other guide modules based on
   example demand.
5. Treat inline assembly and plugins as separate strategic projects. Neither is
   a sensible incidental addition to make a tutorial row green.

### Recommended execution order

1. **Evidence first:** P0 follow-up contracts for CLI, processes, testing, plugins,
   and broad-chapter family probes.
2. **Largest source multiplier:** general procedure overloading.
3. **Everyday syntax:** inferred array literals, evaluated lengths, pointer
   iteration, then the remaining control-flow spellings.
4. **Module composition:** `#load`, conditional items, module parameters, argv/stdin.
5. **Generic library unlock:** cross-file template instantiation and stronger inference.
6. **Architectural project:** first-class `Code` and inspectable metaprogramming, only
   after its representation and tool impact are decided.
7. **Systems breadth:** process I/O/capture, directory/metadata, synchronization,
   C variadics and bindings generation.
8. **Ecosystem breadth:** packages, graphics/media expansion, then separate strategic
   decisions for assembly, plugins, Windows hosting, and exact Jai distribution parity.

## Claims requiring real Jai or vendored-primary confirmation

The following must not be promoted from “guide suggests” to “Jai guarantees”
without a real Jai compiler run, distributed module source, official
documentation, or another commit-pinned primary source:

1. The exact `String_Builder` layout, growth strategy, allocator capture,
   `builder_to_string` ownership/reset behavior, `extra_bytes_to_prepend`, and
   what `free_buffers` releases or retains.
2. Whether constant Jai string bytes are always followed by NUL, and whether
   that byte is part of any allocation/ownership contract. The guide states
   this; Jairs deliberately does not provide it.
3. Exact `then`, if-case, `#complete`, and `#through` grammar and behavior,
   especially duplicate enum values, fallthrough, ordering after `else`, and
   non-enum scrutinees.
4. Exact overflow, shift, cast, float, and default-number semantics for the Jai
   compiler version represented by the guide.
5. The layouts and mutation rules of fixed arrays, views, resizable arrays,
   strings, `Any`, `Type_Info`, `Context`, allocators, temporary storage, and
   thread/runtime support.
6. `#load`, named import, directory/string import, module parameter, scope, and
   workspace resolution rules, including collision and initialization order.
7. General procedure/operator overload ranking, implicit conversions,
   `#symmetric`, `#as`, interface constraints, and polymorphic inference.
8. All first-class `Code`, macro hygiene, caller-scope, AST node, `#modify`,
   type-variant, and for-expansion semantics.
9. FFI ABI details: aggregate classification, C/C++ calling conventions,
   variadics, callbacks, exported symbols, dynamic libraries, and per-platform
   library naming.
10. The current `Compiler` message protocol, workspace lifecycle, complete
    `Build_Options`, placeholders, bitcode output, resource embedding, and
    plugin ABI.
11. Exact standard-module names and signatures, including `Basic`, `String`,
    `POSIX`, `Thread`, `Process`, `Simp`, `Input`, `Window_Creation`,
    `Sound_Player`, `Hash_Table`, `Pool`, `Mail`, and testing modules.
12. Simp coordinate systems, global/context state, batching, texture/font/audio
    ownership, and which declarations are current. Existing public copies are
    already known to disagree.
13. Inline assembly syntax, register constraints, feature flags, compile-time
    execution, and optimizer interaction.
14. The concurrency memory model, thread-group scheduling, mutex/channel
    contracts, and whether contexts/temporary storage are initialized per
    thread.
15. Plugin availability, compatibility promises, and package/distribution
    semantics in current Jai.
16. The guide's assertion that every example works with the latest Jai
    compiler. The local corpus can be inventoried, but not validated as Jai
    without the compiler and matching standard modules.

Until those are confirmed, this document supports prioritization and
Jairs-side gap analysis, not a claim of exact Jai compatibility.
