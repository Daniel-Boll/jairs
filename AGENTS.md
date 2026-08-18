# Working conventions for this repository

Read [`PLAN.md`](PLAN.md) §7 first — it is always the current handoff, and it is
rewritten at the end of every wave. §1.5 is the per-crate status table.
[`CONTRIBUTING.md`](CONTRIBUTING.md) has the human-facing rules; this file records how
work actually proceeds, including the things that have cost real time.

---

## The rhythm

Work happens in **waves**. One wave is one component of the slice, and it follows the
same five steps every time:

1. **Put the design forks to the decider before writing code.** Not after. Every wave's
   forks turned out to be expensive to undo, and two of them were only *visible* as
   forks because someone asked. Use the options-with-tradeoffs form: name what each
   choice costs, and name a recommendation.
2. **Record the decisions in an ADR**, with the rejected alternatives argued at their
   point of decision and the project that chose them named where one did. An ADR is
   written once the decision is made and is immutable; a later decision that overturns
   an earlier one gets a *new* ADR that says so (ADR-0018 §5 amends ADR-0017 this way).
   Add the index row in `docs/adr/README.md`.
3. **Implement on a branch named `feat/<component>`.**
4. **All six gates green** (below), then update `PLAN.md` §1.5 and rewrite §7 as the
   *next* wave's handoff, and refresh the README's **"Status, honestly"** section — the
   wave name and test count in its first line, plus any row of its four tables the wave
   changed. That section is the project's only outward-facing honest inventory, and it
   has rotted before: it went a whole wave claiming "a trap still reports no source
   location" after both engines had learned to report one. A capability table is easier
   to keep true than a paragraph, which is why it replaced one.
5. **Commit each wave as it goes green** — a `git commit` on the wave's `feat/<component>`
   branch the moment all six gates pass, *before* starting the next wave. This is not the
   same as merging: merging to `main` still needs the decider's explicit say-so (step 6).
   Committing is the wave's own safety net and does not.

   **Why this is a rule and not a preference.** Fourteen waves once sat uncommitted in one
   working tree at once, and a careless `git checkout tree-sitter-jairs/grammar.js` — run as a
   casual undo during a teeth-check — reverted the grammar *nine waves*, because `HEAD` was
   nine waves behind the working state. It cost an hour of rule-by-rule reconstruction. A
   per-wave commit bounds the blast radius of any such slip to a single wave: `git checkout`
   or `git restore` then takes a file to the end of its own wave, not to a HEAD from before the
   feature existed. `grammar.js` is the sharpest case — gate 6 checks it against *drift* by
   regenerating, never against *reversion* — but the rule is general.

6. **Merge to `main` with `--no-ff`**, one logical change per commit — but only when the
   decider explicitly says so.

## The six gates

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo run -q -p jr-cli -- fmt --check tests/corpus/valid tests/corpus/imports/valid \
    tests/corpus/type-errors tests/corpus/cfg-errors tests/corpus/modules modules tests/fixtures
# corpus drift + query validation (tree-sitter is not installed locally):
cd tree-sitter-jairs && npx --yes tree-sitter-cli@0.26.11 generate \
  && npx --yes tree-sitter-cli@0.26.11 parse --quiet ../tests/corpus/valid/*.jr \
     ../tests/corpus/imports/valid/*.jr ../tests/corpus/type-errors/*.jr \
     ../tests/corpus/cfg-errors/*.jr ../tests/corpus/modules/*.jr \
     ../tests/corpus/modules/*/*.jr ../modules/*/*.jr ../tests/fixtures/*/*/*.jr \
  && for q in highlights folds indents locals; do \
       npx --yes tree-sitter-cli@0.26.11 query "queries/$q.scm" \
         ../tests/corpus/valid/024-hello.jr > /dev/null || exit 1; \
     done
```

Track the workspace test count in the §7 handoff, so a silent loss of coverage is
visible. It has gone 376 → 429 → 511 → 596 → 909 → 916 → 918 → 919 → 924 → 928 → 930 → 935 → 936
→ 969 (W5 sub-waves 1–4) → 974 (W5 sub-wave 5, polymorphic structs) → 976 (W5 sub-wave 6a, `$N` surface)
→ 977 (W5 sub-wave 6b, `$N` instantiation) → 978 (W5 sub-wave 6c, `[N]T` over `$N`; 7a `#expand` surface) → 979 (W5 sub-wave 7b, the `#expand` splice) → 980 (W5 sub-wave 7c, reflecting a bound type)
→ 981 (W5 sub-wave 7h, `#bake_arguments` specialisation — **W5 complete**). W6 sub-waves 1–4 hold at
981 — each adds corpus files that the existing differential and snapshot tests iterate rather than adding a test
case, which is why the *corpus* count is tracked too — and sub-wave 5 reaches **984** with three `jr-cli`
integration tests, because the driver's behaviour is not something a corpus file can observe (210 corpus files).

W7 sub-waves 1–17 reach **986** (211 corpus files). The audit sub-waves then go **988** (ADR-0120, the
expansion fixed point, +2 corpus files) → **990** (ADR-0121, the comptime step budget) → **1001**
(ADR-0122, `BUILD_OUTPUT` confinement — nine of the eleven are unit tests on the predicate, which is
why a wave can move this number a long way without touching the corpus) → **1005** (ADR-0123, the
cross-crate code check) → **1007** (ADR-0124, two latent traps) → **1008** (ADR-0125, `print_int`
executed at last, +1 corpus file = **213**) → **1009** (ADR-0126, the foreign-call pointer span — **no**
corpus file, because the VM traps where native writes short, so a program exercising it has no home in
`valid/`, whose whole premise is that the two engines agree; the test lives in `jr-vm` instead).
ADR-0127 holds at **1009** and adds one corpus file = **214**: a wording sweep changes no behaviour,
and `type-errors/073` is iterated by the existing harness rather than adding a test case. ADR-0128 reaches
**1010** (the instantiation-backtrace test) with no new corpus file. ADR-0129 **holds at 1010** and adds
two corpus files = **216** — both are iterated by harnesses that already exist, which is the clearest case
yet for tracking the corpus count separately from the test count. ADR-0130 **also holds at 1010** and adds
one = **217**: an all-library wave, where the only new coverage a corpus file can carry is a corpus file.
**ADR-0131 also holds at 1010** and adds one = **218** — the same pattern for the same reason, since
Matrix4 like Vector4 is exercised by the differential and snapshot harnesses rather than by a Rust unit
test. **ADR-0132 also holds at 1010** and adds one = **219**, closing sub-wave 3 with a Quaternion —
the third all-library wave in a row to move only the corpus count. **ADR-0133 also holds at 1010**
and adds one = **220** — a language wave (parser + HIR) but no new Rust test, since the coverage is
the corpus program that reads `it` and `it_index` in every combination. **ADR-0134 also holds at
1010** and adds one = **221** — another HIR-shaped wave (nested procedures + local constants) with
its coverage in `valid/107` and a repurposed regression test in `jr-hir` guarding the flip.
**ADR-0135 also holds at 1010** and adds one = **222** — a follow-up MIR change closing ADR-0133 §2
(range iteration with an index), with its coverage in `valid/108`.
**ADR-0136 also holds at 1010** and adds one = **223** — Wave 6 (`[..]T` dynamic-array syntax),
another all-corpus-file wave since the coverage is what a corpus program can observe.
**ADR-0137 also holds at 1010** and adds one = **224** — Wave 7 (`$$T`, poly + baking), whose
coverage in `valid/110` is what a corpus program can observe.
**ADR-0138 also holds at 1010** and adds one = **225** — Wave 8 (variadic `..T` declaration
surface), with `valid/111` exercising the callee-view shape and explicit-view passing. The
call-site packing sugar is deferred to a follow-up wave.
**ADR-0139 also holds at 1010** and adds one = **226** — the follow-up completing Wave 8: MIR
packs trailing arguments into a stack `[N]T` view. `valid/112` exercises the sugar (zero,
one, several trailing args; fixed+variadic mix; pass-through view).
**ADR-0140 also holds at 1010** and adds one = **227** — the first of the programme's owed
follow-ups: `modules/List` converted to operate on the native `[..]s64` (the hand-rolled
`List :: struct($T)` deleted), `Type_Info_Kind.DYNAMIC_ARRAY` added to `Basic`, and a dump defect
fixed (a `[..]T`'s `.data`/`.count`/`.capacity` all printed `.view_count`, so the mir snapshot could
not tell them apart). `valid/113` exercises the converted operations and the reflection; `valid/088`
and `089` now declare `[..]s64` with their exit codes unchanged — the one wave here that touched a
crate (`jr-mir`'s dump) and still moved only the corpus count, because the fix is display-only.
**ADR-0141 also holds at 1010** and adds one = **228** — the second owed follow-up, a `..Any` variadic,
probed and found already composing (ADR-0138's callee view ∘ ADR-0139's packing ∘ ADR-0076 §1's
`*U`→`Any` coercion). One gap fixed in `jr-sema` (the exactly-one-trailing disambiguation bypassed the
coercion, so `f(*a)` errored while `f(*a, *b)` worked); the decision is now shared by one helper. No new
code, no MIR change. `valid/114` pins mixed-type `..Any` (the `print(fmt, ..)` shape); bare values stay
E0214 (ADR-0076 §4's deferred bare-value→`Any`).

**A number in this file is now partly enforced.** `crates/jr-cli/tests/codes.rs` fails when the
"first free code" claim below rots. The test count and the corpus count are still prose, and both were
wrong in three places each when the audit looked — which is the argument for reading §7 rather than
trusting a count you find anywhere else. **That advice has itself been wrong once**: at ADR-0126 §7 said
"214 corpus files" while this file said 213, and 213 was right — so §7 now carries the *definition*
(the `.jr` files under `tests/corpus/` outside `tests/corpus/modules/`; 223 counting those) rather than
only the figure.

## House style

Enforced by the first four gates, so it is not a matter of taste:

- `[lints] workspace = true` in every crate, and **no crate-level `#![warn]`**.
- `missing_docs` is a workspace warning, so **every** public item — including enum
  variants and struct fields — needs a `///`.
- Private `mod` plus a curated `pub use` in `lib.rs`. Do not make a module public to
  satisfy an intra-doc link; link the item instead.
- Module `//!` docs argue **why**, and name the rejected alternative. A module whose
  docs only restate its type names is not finished.
- **Exhaustive matches** rather than `matches!` or a `_` arm, so that adding a variant
  is a compile error at every site that must change. This has caught real bugs.
- Stable Rust only; the toolchain is pinned. No nightly rustfmt options.
- `unsafe` needs a `// SAFETY:` comment stating the invariant.

## Verifying a split commit

When a wave contains a separable bugfix, give it its own commit *and prove it stands
alone*:

```sh
git add <the fix's files>
git stash push -u --keep-index -m "wave remainder"
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo test --workspace
git commit
git stash pop
```

This is not always possible. In the `jr-vm` wave the aggregate-parameter fix and the
`ConstValues` API change were interleaved hunks in `crates/jr-mir/src/build.rs`, so the
fix went into the wave commit with its own paragraph at the top of the body. Prefer that
over hunk-level surgery.

## Two failure modes this project actually has

### Silent miscompiles from well-typed placeholders

Twice now — braceless control bodies lowering to `Stmt::Error`, and a field of an
aggregate parameter lowering to `Rvalue::Undef` — the shape was identical:

> a construct the grammar allows, no representation on the lowering path, filled in
> with a placeholder that is a **legitimate value**.

Neither the verifier nor ADR-0017 §4's poison gate can catch one, because `Stmt::Error`
and `Rvalue::Undef` are both things a correct program produces. So:

- **A `None` from a place, callee or resolution helper must refuse the body**, never
  fall back to a placeholder. `jr-mir`'s `Lower::give_up` is the channel for a failure
  discovered mid-build; `scan` is the channel for one visible before it starts.
- **If a construct is legal in the corpus, something must execute or snapshot it.**
  `modules/Basic` hid a bug for a whole wave because it is not in
  `tests/corpus/valid/` and `file_mir` is per file, so its bodies never appeared in a
  snapshot.

### Plans that contradict themselves

`PLAN.md` §7 once put `jr run` and the slice exit criterion in scope while assigning a
refusal that criterion depends on to a later wave. Check the handoff's scope against
what the named test actually needs, early, and raise the contradiction rather than
picking a side quietly.

## Tooling notes

- **Subagents have been unreliable on this codebase.** Three of four stalled on the MIR
  wave. Write the modules that define an API yourself; delegate only single-file work,
  with the consumed signatures stated verbatim and a short reading list.
- The agent shell in use **rejects any command containing `grep`, `find` or `rg`** — and
  it rejects the *whole* command, so a `python3` heredoc chained after a `grep` silently
  never runs. If an edit appears not to have applied, check whether its command was
  refused. Use the dedicated search tools instead.
- A query naming a node the grammar has not got used to be **undetectable**, and the
  failure is silent: highlighting simply stops. `tree-sitter query` exits 1 with
  `Invalid node type`, which is why gate 6 now runs it over all four query files
  (ADR-0025 §4).
- Editor integration is **verified, not gated**:
  `nvim --headless -u NONE -l editors/nvim/verify.lua` (166 checks, non-zero on failure).
  Neovim is not a build dependency, so it is not one of the six — but run it after
  touching `jr-lsp`, `grammar.js` or the queries.
- `insta` snapshots: review the `.snap.new` diff, then move it over the `.snap` and
  delete the `assertion_line:` header line, which is noise that changes whenever a test
  moves.
- Never print a `FileId` into a snapshot. It is an index assigned in database load
  order, so one new corpus file renumbers every occurrence — churn that defeats the only
  thing a snapshot is for. `jr-mir`'s dump prints `extern proc3` for this reason.

## Diagnostic codes

There is no central registry of *constants*; the ownership table below is the central record, and
`crates/jr-cli/tests/codes.rs` enforces the part of it that is mechanically checkable. Each crate
keeps its codes near where they are raised — most in a `code.rs`, with one constant per code and
a `///` saying exactly what raises it. Ranges: E0001–E0006 lexer, E0100–E0199 parser,
E0200–E0211 `jr-hir` (E0210 actually raised by `jr-db`'s module loader, E0204 relocated
to `jr-sema`), E0212–E0226 `jr-sema`, E0227–E0229 `jr-mir`, E0230 `jr-db` const-eval,
E0231 `jr-db` unused imports, E0232–E0247 and E0250–E0270 `jr-sema` and `jr-hir` past
their original blocks (E0250/E0253 and E0262–E0264 in `jr-hir`, the rest in `jr-sema`).
E0262–E0264 are `#insert`'s: a non-literal operand and a parse error in the text (ADR-0072),
and expansion nested too deep (ADR-0073). E0265–E0268 are comptime/reflection refusals
(ADR-0075/0076); E0269–E0270 are parameterised-struct refusals — a `Name(args)` that is not a
parameterised struct, and a wrong type-argument count (ADR-0085); E0277 is `has_note`/`note_value`'s single refusal — an unreadable note name *or* a first argument that is not
a procedure (ADR-0099), one code because they are one intrinsic's two ways of being unaskable — and E0278 is
`==` on an aggregate, a `string` included (ADR-0099 §4), which was a leaked ICE until this wave probed it.
E0279 is `typed`/`untyped`'s single refusal — a `typed` operand that is not a `*u8`, or an `untyped` operand that is not a pointer (ADR-0106) — one code for one boundary's two directions. E0277 also covers `noted_count`/`noted_name`'s two refusals (ADR-0100) — an unreadable note name or index —
because all four note intrinsics are one mechanism and share its one way of being unaskable.
E0271 is a `$N` comptime-value
argument that is not a compile-time constant (ADR-0088) — **owned by `jr-db`** beside E0230,
because constancy is a const-eval judgement, defined in `crates/jr-db/src/consts.rs`.

E0272 is a **cross-file** `#expand` macro call (ADR-0091 §3 — repurposed from ADR-0090's
pending-splice refusal, which the splice lifted); E0273 is an early `return` in a macro body or a void macro
in expression position — **owned by `jr-hir`**, continuing its block (E0262–E0264 are `#insert`'s), because
it is raised in lowering where the splice is built.

E0274 was a call to a `#modify` procedure while its predicate was unevaluated;
ADR-0095 **retired** it when the predicate began running, the way E0120/E0122 were retired. E0275 is an
instantiation **rejected by its `#modify` predicate** — **owned by `jr-db`** beside E0230/E0271, because the
predicate is evaluated in `file_mir`.
E0276 is `#bake_arguments` refusing a **non-literal** baked value or an
operand that is not a locally-declared procedure (ADR-0096/0097) — **owned by `jr-hir`**, since a directive's
validity in expression position is judged in lowering.

**E0282 is the first free code**; E0132 is the first free *parser* code. E0280 refuses an
instantiation family that never settles and E0281 a `$N` call in a file whose `#insert`
operand is computed (both ADR-0120, **owned by `jr-db`**). E0231 is `jr-db`'s
unused-import warning — the first code in this project that is a *warning* rather than an
error, so a consumer filtering by severity has something to filter.

**This table is the authoritative one, and it is partly enforced.**
`crates/jr-cli/tests/codes.rs` reads every code declaration in the workspace and checks the
invariant no per-crate test can state — that no two crates declare the same code — plus that a
constant named after a code binds that code, and that the "first free code" sentence above is
true. So the number in bold fails a test when it rots; the prose around it still does not, and
`AGENTS.md` is the only place the ownership story is written down. Two other copies of this table
existed, in `jr-syntax/src/code.rs` and `jr-db/src/imports.rs`, and by the time the audit at
`354d900` looked they had drifted three ways — `jr-syntax`'s claimed E0131 was free while E0131
was in use. Both copies are now pointers here.

`jr-syntax` used to be the exception that proved the rule — it had no `code.rs`, its codes
were inline `&str` literals, and so its parser emitted **E0200/E0201/E0202** for three
"arrives in wave Wn" refusals, colliding with `jr-hir`'s duplicate-declaration,
unresolved-name and use-before-declaration. A `&str` cannot collide at compile time, so
this stood for waves behind a warning here telling people not to filter tests by those
codes. The codes are now E0120–E0122 and the crate has a `code.rs` whose tests assert that
no code is used twice and that every one falls inside a range the crate owns.

**`jr-hir` and `jr-db` still have no `code.rs`** — their codes are inline constants at or near
their emission sites (`jr-hir/src/lower.rs`, `jr-hir/src/resolve.rs`, `jr-db/src/consts.rs`,
`imports.rs`, `module_loader.rs`, `mir.rs`, `sema.rs`). That contradicts the first sentence of
this section and is recorded rather than quietly tolerated: the cross-crate test above closes the
*collision* risk those files carried, which was the reason the rule existed, so consolidating them
is now tidiness rather than a defect. `jr-mir` shows the other legitimate convention — it names its
codes semantically (`USE_OF_UNINITIALISED`) and binds the code as the value, which the test
accommodates deliberately.
