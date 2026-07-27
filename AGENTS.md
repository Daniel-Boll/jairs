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
5. **Commit and merge to `main` with `--no-ff`**, one logical change per commit — but
   only when the decider explicitly says so.

## The six gates

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo run -q -p jr-cli -- fmt --check tests/corpus/valid tests/corpus/imports/valid \
    tests/corpus/type-errors tests/corpus/cfg-errors tests/corpus/modules modules
# corpus drift + query validation (tree-sitter is not installed locally):
cd tree-sitter-jairs && npx --yes tree-sitter-cli@0.26.11 generate \
  && npx --yes tree-sitter-cli@0.26.11 parse --quiet ../tests/corpus/valid/*.jr \
     ../tests/corpus/imports/valid/*.jr ../tests/corpus/type-errors/*.jr \
     ../tests/corpus/cfg-errors/*.jr ../tests/corpus/modules/*.jr \
     ../tests/corpus/modules/*/*.jr ../modules/*/*.jr \
  && for q in highlights folds indents locals; do \
       npx --yes tree-sitter-cli@0.26.11 query "queries/$q.scm" \
         ../tests/corpus/valid/024-hello.jr > /dev/null || exit 1; \
     done
```

Track the workspace test count in the §7 handoff, so a silent loss of coverage is
visible. It has gone 376 → 429 → 511 → 596.

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
  `nvim --headless -u NONE -l editors/nvim/verify.lua` (23 checks, non-zero on failure).
  Neovim is not a build dependency, so it is not one of the six — but run it after
  touching `jr-lsp`, `grammar.js` or the queries.
- `insta` snapshots: review the `.snap.new` diff, then move it over the `.snap` and
  delete the `assertion_line:` header line, which is noise that changes whenever a test
  moves.
- Never print a `FileId` into a snapshot. It is an index assigned in database load
  order, so one new corpus file renumbers every occurrence — churn that defeats the only
  thing a snapshot is for. `jr-mir`'s dump prints `extern proc3` for this reason.

## Diagnostic codes

There is no central registry; each crate has a `code.rs` with one constant per code and
a `///` saying exactly what raises it. Ranges: E0001–E0006 lexer, E0100–E0199 parser,
E0200–E0211 `jr-hir` (E0210 actually raised by `jr-db`'s module loader, E0204 relocated
to `jr-sema`), E0212–E0226 `jr-sema`, E0227–E0229 `jr-mir`, E0230 `jr-db` const-eval.

**E0231 is the first free code**; E0123 is the first free *parser* code.

`jr-syntax` used to be the exception that proved the rule — it had no `code.rs`, its codes
were inline `&str` literals, and so its parser emitted **E0200/E0201/E0202** for three
"arrives in wave Wn" refusals, colliding with `jr-hir`'s duplicate-declaration,
unresolved-name and use-before-declaration. A `&str` cannot collide at compile time, so
this stood for waves behind a warning here telling people not to filter tests by those
codes. The codes are now E0120–E0122 and the crate has a `code.rs` whose tests assert that
no code is used twice and that every one falls inside a range the crate owns.
