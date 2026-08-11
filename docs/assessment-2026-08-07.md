# Assessment — 2026-08-07, at `354d900`

A full-tree review of Jairs conducted at `354d900` on `main` (tree clean, W7 sub-wave 17
— cross-file generics), asking four questions: what is the current status, where are the
coverage gaps, which deferred promises should have landed by now, and what is straight up
wrong.

This file is the **durable record of the audit**, not a living document. It is written
once and left alone. Where a finding is acted on, the fix is recorded in an ADR and in
`PLAN.md` §7 as usual; the row in [§7 below](#7-remediation-ledger) is updated with the
commit that closed it so that a reader can tell what was addressed from what was
accepted. If this file and the code disagree, the code is right and this file is a
historical document.

---

## 1. Method

Five assessors were run in parallel over disjoint scopes, each measured against *this
project's own* conventions (`AGENTS.md`'s six gates and house style, the ADR record) rather
than against generic best practice. Their reports were then cross-examined: findings raised
by two lenses independently are marked **corroborated**, disagreements are adjudicated in
[§5](#5-contested-and-how-it-was-adjudicated), and everything unexamined is stated in
[§6](#6-coverage).

| Assessor | Scope | Verdict |
|---|---|---|
| `argus` | `jr-hir`, `jr-sema`, `jr-mir`, `jr-db` — correctness | **SERIOUS DEFECTS FOUND** |
| `daedalus` | Crate graph, module seams, duplicated knowledge | **EROSION PRESENT** |
| `aletheia` | Gates, counts, claims-vs-reality, test teeth | **DRIFT PRESENT** |
| `chronos` | `jr-vm`, `jr-db` queries, mid-end, `jr-lsp`, `modules/` — cost | **COSTS FOUND** |
| `nemesis` | Security | **FAILED — returned empty twice** |

`nemesis` stalled on two successive dispatches, which is the subagent unreliability
`AGENTS.md` already documents ("Three of four stalled on the MIR wave"). Two of its
highest-value items were done by hand instead — the `BUILD_OUTPUT` trace and the `unsafe`
inventory — and **the rest of the security scope is unexamined**. That is the single
largest gap in this assessment and is recorded as such.

Confidence labels used throughout:

- **confirmed** — the full path from trigger to wrong outcome was traced, and the
  triggering input can be stated.
- **likely** — the path is there but one step could not be closed.
- **suspected** — pattern-matched, unverified.

---

## 2. Ground truth

Established by running gates 1–5 (gate 6 needs network `npx` and was skipped). Recorded
because several of these numbers appear wrong in the tree's own documents.

| Check | Result |
|---|---|
| Gate 1 `cargo fmt --all --check` | **PASS** |
| Gate 2 `cargo clippy --workspace --all-targets -- -D warnings` | **PASS** |
| Gate 3 `cargo test --workspace` | **PASS — 986 passed, 0 failed, 1 ignored** |
| Gate 4 `cargo doc` with `RUSTDOCFLAGS=-D warnings` | **PASS** |
| Gate 5 `jr fmt --check` over all seven trees | **PASS** |
| Gate 6 (tree-sitter regenerate / parse / query) | **SKIPPED** — needs network `npx` |
| Workspace tests | **986** (jr-syntax 161, jr-mir 124, jr-lsp 118, jr-cli 101, jr-db 96, jr-pool 82, jr-hir 73, jr-fmt 71 +1 ignored, jr-sema 64, jr-vm 48, jr-base 22, jr-diag 18, jr-codegen-clif 3, doctests 5) |
| Corpus `.jr` files | **220** under `tests/corpus/` + 9 in `modules/` + 1 fixture |
| Neovim checks | **166 ok, 0 fail** (`nvim --headless -u NONE -l editors/nvim/verify.lua`) |
| Diagnostic codes defined | **113** distinct (jr-syntax 37, jr-sema 52, jr-hir 16, jr-db 6, jr-mir 3); E0211 deliberately in two crates, documented |
| ADRs | **119** files (0001–0119); index complete in both directions |
| CI runs, ever | **zero** — only `origin/feat/cross-file-generics` was ever pushed, never `main` |

The one deliberate `#[ignore]` is `jr-fmt`'s `show_corpus_diffs`, a diagnostic aid.

---

## 3. Findings

Ordered by impact × confidence × blast radius.

### F1 — [CRITICAL, confirmed] Expansion and instantiation are single-round, and their id-keyed side tables are only partially rebuilt

*Raised by `argus`. Corroborated by `aletheia` (no corpus file exercises these shapes) and
independently by `chronos`, which flagged nested instantiation as unverified — `argus`
answered it.*

`file_consts` keys at least six side tables on unexpanded `(scope, ExprId)` pairs:
`folded_calls`, `type_info_calls`, `pointer_views`, `any_calls`, body-scoped `#run` values,
and `comptime_arg_mask`. A `#insert` splice **renumbers every id after it**, and an
instantiation clone gets a **fresh `BodyId`**. Only two of those tables are rebuilt against
the expanded tree, and expansion runs exactly once.

Sites:

- `crates/jr-db/src/mir.rs:364-368` — when the `#insert` branch fires, `instantiated()` is
  skipped **entirely**, taking `comptime_arg_mask` and `#modify` evaluation with it.
- `crates/jr-db/src/mir.rs:410-421` — the stale-fold clear covers `folded_calls` only.
- `crates/jr-db/src/mir.rs:443-464` — clones get `folded_calls` and `type_info` re-recorded;
  `pointer_views` and `any_calls` are not copied.
- `crates/jr-db/src/sema.rs:675-694` — redirects are built from the **base** check; the
  expanded re-check's `instantiations` are discarded.
- `crates/jr-mir/src/build.rs:3439-3444` — with no redirect, the call lowers as
  `Callee::Direct(template)`; a template emits no MIR (`build.rs:138`).

Three symptoms, all on programs the checker accepts:

1. **One computed `#insert` anywhere in a file silently disables polymorphic instantiation
   for the whole file.** A second body's ordinary `$T` call then lowers to a direct call on
   a template that has no MIR → `internal compiler error: no routine for file N proc M`.
2. **A template calling a template** with concrete arguments — `g :: (x: $T) -> T {…}`,
   `f :: (x: $T) -> s64 { return g(1); }` — same ICE, because the clone's body is keyed
   under a `BodyId` no redirect mentions.
3. **`#run`, `typed`, `untyped`, `any_of` or `any_as` inside a template body** — the
   callee resolves to `Res::Error`, `scan` refuses the body (`build.rs:531,690`), which is
   only **E0245, a warning**, and the call then ICEs when reached.

`jr check` reports clean throughout, because only `main`'s refusal is gated
(`crates/jr-db/src/run.rs:77-93`).

This is the project's own **#1 named failure mode** — a legal construct with no
representation on the lowering path — in its newest and most advertised features. ADR-0101
fixed one instance of it (the two-computed-inserts case) and left the general case open.

### F2 — [HIGH, confirmed] CI has never run, voiding three claimed verification tiers

*Raised by `aletheia` (zero Actions runs, ever). Compounded by `daedalus`, which found the
only wrong-tree grammar guard lives in CI rather than in the six gates.*

Neither assessor could see this alone, and together it is worse than either half:

- `README.md:920` states "Linux x86-64 is **kept green in CI** as a sanity oracle." It has
  never been green in CI, because CI has never executed. This contradicts `README.md:451`
  and `PLAN.md:1374` ("Configured, never run" — the honest version) **in the same tree**.
- `README.md:443` says "six **CI** gates green"; they are green locally.
- `.github/workflows/ci.yml:111-190` defines a `corpus-drift` job that encodes gate 6, and
  it has never run.
- `tree-sitter test` — comparing expected S-expressions in
  `tree-sitter-jairs/test/corpus/jairs.txt`, the **only** check that can detect a wrong
  parse tree rather than an error count — is CI-only and absent from the six gates.

Net effect: **there is no wrong-tree guard in practice at all**, no Linux verification, and
no drift check. The grammar is the project's known-fragile artefact, having once been
reverted nine waves by a careless `git checkout`.

### F3 — [HIGH, confirmed] Compile-time execution has no fuel, step budget, or timeout

*Raised by `chronos`.*

`crates/jr-vm/src/interp.rs:393` — `run_instrs`' dispatch loop has no step counter. The
only bound is `MAX_DEPTH = 256` recursion at `interp.rs:56`. The VM is invoked inside a
salsa query at `crates/jr-db/src/consts.rs:974`, and because the loop makes no database
reads, salsa's cancellation can never reach it.

So a `#run while true {}`, or a non-terminating `#modify` predicate, hangs the compiler.
Under `jr lsp` it hangs the single worker thread
(`crates/jr-lsp/src/server.rs:716-729`) and the unbounded job channel then grows with
every keystroke. The blast radius is **merely opening a file in an editor**.

### F4 — [HIGH, confirmed] `BUILD_OUTPUT` gives attacker source an unconstrained filesystem write and argument injection into `cc`

*Traced by hand, standing in for `nemesis`.*

ADR-0102 lets the compiled program name its own artefact —
`BUILD_OUTPUT :: #run choose_name();` — with the value computed by arbitrary compile-time
code. Nothing sanitises it anywhere along the path:

- `crates/jr-db/src/build.rs:228` — the string is returned verbatim.
- `crates/jr-cli/src/commands/build.rs:115-121` — `PathBuf::from`, no validation: no check
  for an absolute path, for `..`, or for a leading `-`.
- `crates/jr-link/src/lib.rs:114-115` — `fs::write` to `output.with_extension("o")`.
- `crates/jr-link/src/lib.rs:123` — `-o <output>`, writing the linked executable.
- `crates/jr-link/src/lib.rs:122` — the object path is `cc`'s **first positional argument**,
  so a value beginning with `-` is injected as a flag.
- `crates/jr-link/src/lib.rs:189` — `codesign … .arg(path)` with the path last, same issue.

`BUILD_OUTPUT :: ".git/hooks/pre-commit";` makes `jr build file.jr` write an executable to
a path git runs on the next commit. An absolute path writes anywhere the user can.

ADR-0102 documents *naming the artefact*. It does not document writing anywhere on the
filesystem, nor influencing the linker's command line, so this is **worse than
documented**. `jr-link` correctly builds its command with separate `.arg()` calls and no
shell, so this is argument injection rather than shell injection. `jr check` and `jr run`
do not link and are unaffected by this path.

### F5 — [MEDIUM-HIGH, confirmed] `print_int` is executed by nothing, and 13 corpus files are never run

*Raised by `aletheia`, independently verified by hand: a grep over every `*.jr` in the tree
finds `print_int` and `print_error` only in their own definitions and in comments.*

`modules/Basic/module.jr:262` defines `print_int`. `README.md:457` advertises it as the
"Print a number" capability. **No `.jr` file calls it.** Both engines could break it with
all six gates green.

This is the project's own named failure shape recurring verbatim: "`modules/Basic` hid a
bug for a whole wave because it is not in `tests/corpus/valid/`."

Also unexecuted:

- All 13 files in `tests/corpus/imports/valid/` — checked, resolved and MIR-snapshotted,
  but no binary is built and no VM/native comparison runs. The harness reads only
  `tests/corpus/valid/` (`crates/jr-cli/tests/differential.rs:124-130`).
- `Sort.is_sorted` and `less_int`. Since cross-file template instantiation is refused
  (E0268), an importer *cannot* call the generic `is_sorted` — it is currently dead stdlib
  surface.

And the differential's blanket test asserts **agreement only** (stdout, stderr, status) for
~98 valid programs; only 14 have pinned exit values. So "the corpus asserts exit codes
rather than agreement" is true of a minority of files.

### F6 — [MEDIUM] Deferral reasons that have expired, and a frozen "Open" section

*Raised by `aletheia`, with one addition from `daedalus`.*

The highest-value class in the audit: refusals whose stated justification the project has
itself dissolved and never revisited. ADR-0109 caught one of these once, noting "both
halves are now false."

| Item | Stated reason | Why it expired |
|---|---|---|
| `PLAN.md:1352`, `README.md:525` | `talloc` hands out bytes only, "cannot store a wider type without a pointer cast the language does not have" | `typed(T, p)` **is** that cast, since ADR-0106 |
| `PLAN.md:1311`, `README.md:516` | `T == U` absent because its meaning is a design question "no ADR has argued" | ADR-0077 gave every type a stable `id`; `type_info(T).id == type_info(s64).id` is the blessed idiom in `valid/077` |
| `PLAN.md:1397` | `print_digits` still recurses | The project already admits no missing language feature remains |
| `PLAN.md:1363` | Cross-file procedure values "stay refused" | The cross-file half shipped in ADR-0104 |
| `PLAN.md:1396` | `%` on floats, `is_nan`, math intrinsics — all W7's `Math` | `sqrt/sin/cos/exp/ln/powf` shipped; only `is_nan` and float `%` remain |

Shipped but still listed open in "Open, and honest about it", which is frozen roughly 15
waves back: `PLAN.md:1301` (`type_info`/`Any` "are the next thing" — ADR-0075–0078),
`:1316` (computed `#insert` — ADR-0073), `:1326` (`#code` — shipped; the `Code` *value* was
declined, so the item wants striking and replacing rather than leaving open), `:1332` ("W4
… four sub-waves in" — W4 completed all ten), `:1368` (W4.5 — completed), `:1404`
(cross-file `#run` — the callable half shipped). `daedalus` adds `PLAN.md:1414`, whose trap
that `jr-pool`'s field walk "makes every field after the first unreachable" is **fixed**
(`crates/jr-pool/src/layout.rs:664-676`) but still listed as live.

Worth recording precisely: `aletheia` found **no** deferred item that was secretly broken,
and the Traps list is in good order. So *the deferred list is honest; the critical defects
are not on any list.*

### F7 — [MEDIUM, corroborated] The diagnostic-code scheme has failed and nothing checks its union

*Raised independently by `daedalus` and `argus`; quantified by `aletheia`.*

`AGENTS.md` states that "each crate has a `code.rs`". Two do not: **`jr-hir` and `jr-db`**,
which between them hold all the range exceptions, keep their codes as inline constants
(`crates/jr-hir/src/lower.rs:43-75`, `resolve.rs:87-104`, `crates/jr-db/src/consts.rs:90,97`,
`mir.rs:797`, `imports.rs:62`, `module_loader.rs:66,78`).

The only uniqueness and range tests are `crates/jr-syntax/src/code.rs:262,273`, and they are
**per-crate, blind to cross-crate duplicates** — as that file's own header admits: "they
cannot check a claim about somebody else's range." The E0200/E0201/E0202 collision with
`jr-hir` stood for waves precisely because nothing checked, and the mitigation added
afterwards cannot catch the next one.

The range table is hand-copied in three drifting places: `AGENTS.md`,
`crates/jr-syntax/src/code.rs:22-38` (already stale — it claims E0258 and E0131 are free,
but E0131 is used at `code.rs:209`), and `crates/jr-db/src/imports.rs:59-61`.

### F8 — [MEDIUM, structural] Const-eval rebuilds the whole VM program per constant, per round

*Raised by `chronos`; structural, unmeasured.*

`crates/jr-db/src/consts.rs:894-953` — per unresolved constant, per round (`MAX_ROUNDS = 16`
at `consts.rs:578`), `evaluate` builds a fresh `Program`, **re-lowers every imported
module's entire MIR** (`:929`) and re-assembles it, then `Vm::new` allocates and zeroes a
1 MiB region (`crates/jr-vm/src/memory.rs:93`) and scans the **entire pool** to intern
strings (`interp.rs:202`). This compounds with the pool never evicting
(`crates/jr-pool/src/pool.rs:73-80`), so const-eval gets slower as an LSP session ages.

Related, same lens:

- Every query is keyed per file with `no_eq` (`crates/jr-db/src/queries.rs:20-23`), so one
  character re-runs lower → resolve → check → const-eval → MIR for every body in the file.
  This is the "finer optimized-MIR key" `PLAN.md` already owes.
- `crates/jr-lsp/src/server.rs:522-525` runs `load_workspace_files()` — up to
  `MAX_FILES = 10_000` reads — **synchronously on the write thread**, for `references`,
  `rename` *and* `codeAction`, which editors send eagerly.

### F9 — [LOW-MEDIUM] Structural erosion with a known per-wave cost

*Raised by `daedalus`, all confirmed.*

- **The attribute token-set trap, seven documented bugs and counting.** Two unlinked
  literal lists: `crates/jr-syntax/src/parser.rs:377` (in `looks_like_proc_signature`) and
  `:1007-1008` (the attribute-consuming arm), plus a third at `lexer.rs:1052`. One shared
  `const PROC_ATTRS` would make the eighth bug a compile error instead of a silent
  fourteen-error cascade.
- **Field-type walk: three crates × five kinds.** `crates/jr-pool/src/layout.rs:624`,
  `crates/jr-vm/src/lower.rs:637`, `crates/jr-codegen-clif/src/body.rs:1761`, each needing
  to know Struct, Union, Results, Context and Variant. `PLAN.md` says "four kinds now",
  understating its own cost. The good news is ADR-0052's silent-failure arm is fixed.
- **Array element stride re-derived in both engines** —
  `crates/jr-codegen-clif/src/body.rs:1312`, `crates/jr-vm/src/lower.rs:565`,
  `interp.rs:623` — three comments promising they match `layout_of`'s internal rule where
  one exported `jr_pool::stride_of` belongs.
- **The trap frame line exists twice.** The message *head* is single-sourced through
  `jr_base::trap_message`, but the backtrace punctuation `"  in "` is emitted as generated
  data with a hardcoded length 5 at `crates/jr-codegen-clif/src/lib.rs:200`, duplicating
  `crates/jr-base/src/trap.rs:74`. So `jr-base` is "the one place that decides what a trap
  says" for the head only. Documented as accepted coupling; guarded only by differential
  backtrace tests.
- **33 `pub mod` declarations** across five crates (`crates/jr-db/src/lib.rs:53-62`,
  `jr-lsp`, `crates/jr-hir/src/lib.rs:56-60`, `jr-syntax`, `jr-cli`) against the house rule
  "private `mod` plus a curated `pub use`". `jr-hir` exposing `lower` and `resolve`
  wholesale makes internal reshuffles visible to all eight dependents. Either enforce the
  rule or amend it by ADR; a rule half-followed teaches people to ignore the others.

---

## 4. Needs verification before action

- **`argus` F4 (likely) — ADR-0117 cross-file generics may cache a wrong field type in the
  pool.** `crates/jr-sema/src/ctx.rs:835` consults `type_bindings` *before* the module-scope
  answer at `:872-877`, and `resolve_instance_fields_in` (`:773-805`) swaps HIR, file and
  signatures but **not bindings**. `set_instance_fields` (`:686`) then caches the result for
  every later user. Silent wrong type or layout is the worst class in this codebase, and
  this is the freshest code in the tree. *Cheapest check:* a two-module program where the
  importer is inside an instantiation with `T` bound and the declaring module's
  parameterised struct has a field typed `T`.
- **`argus` F5 (likely)** — `check_polymorphic_call`
  (`crates/jr-sema/src/check.rs:4413-4429`) *removes* inferred bindings rather than
  restoring what they shadowed, unlike the correct save/restore idiom at
  `crates/jr-sema/src/ctx.rs:678-692`. Masked today by F1, and a landmine for its fix.
- **`argus` F6 (suspected)** — store-to-load forwarding skips the ADR-0106 type guard for
  constant operands (`crates/jr-mir/src/forward.rs:165`, `:245-250`) on a comment claiming
  that shape "cannot arise". That is the checkable-stale-comment pattern which has twice
  hidden real bugs here. *Cheapest check:* `typed(T, …)` on a constant `null`.
- **F8's magnitude** — structural only. *Settling measurement:* `jr bench` after-edit on a
  file with ~20 constants importing `Basic`, before and after hoisting the `Program`
  rebuild.

---

## 5. Contested, and how it was adjudicated

- **`aletheia`: "(d) Broken, not deferred: none found" vs `argus`: two confirmed
  criticals.** Not a genuine conflict. `aletheia` scoped its class (d) to items *on the
  deferred list*, and explicitly flagged that it ran no adversarial programs — "'not
  found', not 'proven absent'". `argus`'s defects are not on any list. Both hold, and the
  union is the sharper answer: the deferred list is honest, and the critical defects are
  undocumented.
- **`jr bench` having no threshold or CI gate.** The dispatch framed this as possibly an
  unfinished intention. `chronos` read ADR-0033 §4 and reports it as a *considered* trade,
  genuinely used for decisions (ADR-0034 cites a 55 ms measurement on a 302-file synthetic
  workspace). `chronos`'s reading is accepted. The residual risk stands and is worth one
  sentence somewhere: a performance regression is invisible to CI by construction.

---

## 6. Coverage

**Examined.** `jr-hir`, `jr-sema`, `jr-mir`, `jr-db` for correctness. The whole crate graph
and its duplicated-knowledge seams. Gates 1–5, all counts, doc claims, and corpus/test
teeth. `jr-vm`, the query layer, the mid-end, `jr-lsp` and `modules/` for cost.
`BUILD_OUTPUT` end to end, and the complete `unsafe` inventory.

**Not examined — the security lens, almost entirely,** because `nemesis` failed twice.
Specifically unexamined:

- The VM linear region's heap/frame collision and offset wraparound.
- Whether `any_as`'s checked read can be defeated by a hand-built `Type_Info` — a program
  *can* construct one, since it is an ordinary Jairs struct in `modules/Basic`, and
  validation covers the declaration rather than a value's provenance.
- Whether a procedure pointer can be forged through the deliberately-untagged `union` into
  `resolve_callee`.
- Comptime-FFI-gate bypasses via an indirect call, a `#modify` predicate, or generated code.
- `jr-lsp` path handling and whether workspace discovery can escape its root.
- Supply chain, including gate 6 running `npx --yes tree-sitter-cli` from the network on
  every verification, with no integrity pin and no `--ignore-scripts`.

Also unexamined: gate 6 itself (needs network `npx`); anything on Linux x86-64;
`crates/jr-hir/src/lower.rs`'s 3,450 lines beyond its code constants — including every
`Stmt::Error` construction site, the `#insert` splice, macro splicing and
`#bake_arguments`; `jr-mir`'s `inline.rs`, `constprop.rs`, `dce.rs` and `verify.rs`
internals; `jr-fmt` and `jr-syntax` internals; grammar divergence between `parser.rs` and
`grammar.js` construct by construct; module `//!`-doc quality crate-wide; and
exhaustive-match and `missing_docs` compliance sweeps.

**Recommended second round.** Security, split into three narrow dispatches (VM memory;
`Any` and proc-pointer forgery; LSP and supply chain), since two broad ones died. Then
`jr-hir/src/lower.rs`'s placeholder sites, where F1's sibling defects most likely live.

---

## 7. Remediation ledger

Ordered so that each step is safe to take given the ones before it. The **Closed by**
column is filled in as work lands; an empty cell means outstanding.

| # | Item | Finding | Closed by |
|---|---|---|---|
| 1 | Verify F4 and F6 of §4 before touching their areas | §4 | `505950e` — **both NOT reachable.** F6: `typed(s64, null)` is E0257, and there is no other pointer *constant*, so `forward.rs`'s comment is currently true. F4: a bound type variable cannot be a parameterised struct's type argument (E0212, ADR-0085 §5), so every instance resolves before any binding is live — the invariant held by an *unrelated* refusal. Hardened anyway in `3fa61cb` |
| 2 | Correct the false CI claim; add `tree-sitter test` to gate 6 | F2 | `7b318e9` (claim corrected in README, `PLAN.md` §1.4 and §7). **Gate 6 addition still owed** — recorded in the reconciled "Open" list |
| 3 | Write the corpus files that expose F1 — they must fail first | F1, F5 | `505950e` (`valid/099`, `valid/100`, both reproducing `no routine for file 0 proc 0` before the fix), `7b318e9` (`valid/101`, `print_int`) |
| 4 | Make expansion iterate to a fixed point; rebuild every id-keyed table | F1 | `505950e` — ADR-0120. All four symptoms fixed or refused; MIR snapshot grew by exactly the two new files |
| 5 | Add a VM step budget | F3 | `3fb7cd7` — ADR-0121 |
| 6 | Confine `BUILD_OUTPUT` | F4 | `444850b` — ADR-0122 |
| 7 | One workspace-level code-uniqueness test; `code.rs` for `jr-hir` and `jr-db` | F7 | `b7ef89e` — ADR-0123. The **test** is done and teeth-checked; the two `code.rs` files are **not**, deliberately downgraded to tidiness since the test closes the collision risk that motivated the rule |
| 8 | Reconcile the docs: true counts, strike shipped items, revisit expired refusals | F5, F6 | `7b318e9` — ADR-0125 |
| 9 | Shared `PROC_ATTRS` constant | F9 | `3fa61cb` — ADR-0124, as an exhaustive `ProcAttr` enum rather than a shared `&str` list, so an eighth attribute is a compile error |
| 10 | Hoist the const-eval `Program` rebuild, once measured | F8 | **Open.** Unmeasured; the settling measurement is named in §4 |

Step 3 precedes step 4 deliberately, and the ordering is not a preference. F1 exists
*because* those tests do not, and fixing a single-round pipeline without them risks a
half-fix that hides a miscompile — the exact hazard ADR-0085 was landed in two commits to
avoid.

### What remains open after the remediation waves

Recorded here so the ledger above is not read as "the audit is discharged".

- **The whole security scope** (§6). `nemesis` failed twice; only `BUILD_OUTPUT` and the
  `unsafe` inventory were covered, by hand. A second pass is owed and should be **three
  narrow dispatches** — VM memory region; `Any`/proc-pointer forgery; LSP and supply chain —
  since two broad ones died.
- **Every performance finding** (F8). Structural only, none measured.
- **`tests/corpus/imports/valid/`'s thirteen files** are still never executed in either
  engine, and `Sort`'s generic surface is unreachable across a module boundary while E0268
  stands.
- **`E0245` is only a warning**, so a body `scan` refused still links. That is what let F1's
  four defects reach an engine. Gating it on reachability would have *masked* them, so it was
  deliberately not done in the same wave.
- **`check_polymorphic_call`'s binding leak** (§4, `argus` F5) — the sibling of the one
  ADR-0124 fixed, masked by the same E0212 deferral.
- **`jr-hir` and `jr-db` still have no `code.rs`**, which `AGENTS.md` now states rather than
  implies.

---

## 8. What is sound

Recorded so that remediation does not damage it.

- **All five runnable gates pass**, 986 tests, one deliberate documented `#[ignore]`. No
  `.snap.new` files, no `assertion_line:` headers, no `FileId` in any snapshot — the
  snapshot conventions are followed exactly.
- **`jr-pool` as the single layout and arithmetic oracle is real**, disciplined, and
  commented at every consumption site (`crates/jr-codegen-clif/src/repr.rs:5`: "Nothing
  here computes a size"). Arithmetic is genuinely two implementations and not three —
  `jr-sema` folds nothing. Extend this rather than bypass it.
- **The crate graph has no cycles**, and `jr-vm` depends on neither `jr-diag` nor
  `jr-codegen`, so the two-engine symmetry is enforced by the graph itself.
- **All 9 `unsafe` blocks carry a `// SAFETY:` comment** —
  `crates/jr-base/src/id.rs:43`, `crates/jr-syntax/src/kind.rs:847`,
  `crates/jr-vm/src/ffi.rs:186,245,258,268,330`, `crates/jr-vm/src/memory.rs:403`. The
  house rule holds with no exceptions.
- **The mid-end is properly bounded** — `MAX_OPT_ROUNDS = 8` with a real convergence test,
  and a leaf-only inliner with a 24-statement cap whose termination is structural
  (`crates/jr-mir/src/inline.rs:52-60`).
- **`Int_Map` is textbook-correct**: tombstones counted in probe load, growth drops them
  (`modules/Map/module.jr:48-50,70-72,225`). `jr-link` is a genuinely deep module — one
  function, zero internal deps, no shell.
- **Frame memory is a bump allocator with mark/release**, zeroing only the frame's own
  bytes (`crates/jr-vm/src/memory.rs:112-126`).
- **ADR discipline is excellent** — 119 ADRs, index complete both ways, amendments arriving
  as new ADRs. `PLAN.md` §7's counts are the only correct ones in the tree.

---

## 9. Verdict

**REMEDIATION REQUIRED.**

F1 is a confirmed Critical with a complete causal chain from a legal source program to an
internal compiler error on code `jr check` reports clean, which would ordinarily read as
*do not ship*. The project self-describes as pre-alpha and makes no release claim, so that
label would measure it against an intent it does not have.

The honest statement: the architecture is sound and worth building on, the gates are green,
and there is a confirmed miscompile-class defect in the newest features that the test suite
structurally cannot see. The gap between what this project verifies and what it believes it
verifies is wider than the code quality suggests — chiefly because CI has never run.
