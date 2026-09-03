# ADR-0184: `#insert` at file scope — comptime code that generates declarations

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **Wave A of the compatibility plan**, and the change that turns per-OS support from a compiler feature into
  a library one. The user's framing was the correct one: *"perhaps most of this in compiletime"*.

## Context

### What already worked, and what one missing arm cost

ADR-0181 gave a per-OS **value**: `CLOCK_MONOTONIC :: #run monotonic_clock_id();`, where the callee reads
`os()`. `modules/Time` uses it and works on both targets.

What could not be selected was a **declaration** — and a `#foreign` library is one. So the plan proposed a
per-OS `#system_library` operand and hit a query-order cycle: the library name is needed while building the
signatures const-eval depends on.

Probing found the mechanism was already there. `#insert` has spliced **statements** since ADR-0072, and a
computed operand — `#insert #run pick();` — already worked *inside a body*, choosing per OS. The gap was one
arm in the file-scope directive dispatcher:

```rust
match text {
    "import" => self.parse_import_decl(),
    "run"    => self.parse_run_decl(),
    "scope_module" | "scope_export" => self.parse_scope_decl(),
    _ => false,          // ← `#insert "X :: 7;";` at file scope: E0101, unexpected token
}
```

**Four arms, and the missing fifth is the whole wave.** A generated `#framework "OpenGL"` is a *declaration*,
so the cycle the plan feared is dissolved rather than broken: what runs is comptime code emitting the
declaration, not const-eval reaching for a library name.

## Decision

### §1 — A new item kind, because a generated declaration is not a `#run`

`ItemKind::Insert { expr, span }`, parallel to `Run`. The parser reuses `RUN_DECL`'s node shape, so
tree-sitter needed **no** grammar change and gate 6 was clean on the first try — `run_decl` is
`directive expr ";"`, which a `#insert` at file scope already matches.

**Not a variant of `Run`.** A `#run` is evaluated for its *effect* and its value is discarded; an `#insert`'s
value is source text that becomes items. Sharing a variant would have made every consumer test a flag, and
the `wanted()` enumeration — which decides what const-eval evaluates — needs them apart: a file-scope
insert's operand is a first-class const-eval target (`Wanted::FileInsertOperand`), which is how it inherits
evaluation, cycle detection and E0230 reporting for free rather than growing a second evaluator.

### §2 — Generated items are allocated straight into the file's arena

Not held aside and merged. `lower_insert_decl` parses the text as a file and lowers its items in place, so a
generated declaration is **indistinguishable** from a written one from that point on: it resolves in any
order, exports under `#scope_export`, appears in the LSP, and is formatted.

This is what makes the feature small. Nothing downstream learned about generated items — the alternative, a
side table of "items that came from an insert", would have needed every consumer to consult it and would have
been a second definition of what a file's items are.

**`ItemId` is not stable across the re-lowering that consumes an evaluated operand**, and this is stated
because it is a live hazard: expanding an insert shifts every item after it. ADR-0072 §2 keys inserts by
**span** for exactly this reason, and `valid/136` pins it with two inserts in one file plus a declaration
generated *after* its use.

### §3 — Signatures are recomputed over the expanded tree

`checked_expanded` previously reused the **unexpanded** signatures, under a comment reading "because
`#insert` adds no items". That was true when an insert could only splice statements. It is now false, and the
comment is the kind of stale reasoning that survives review: a generated procedure would have had no
signature, and the failure surfaced as *"internal compiler error: called a procedure taking 2 arguments with
1"*.

The polymorphic branch beside it already recomputed signatures over its own expanded tree, for the same
reason one phase earlier. The two now share the shape.

**A comment explaining why a phase can be skipped is a comment that expires**, and this one expired quietly.
It is the third instance in this project of a hand-maintained claim with nothing enforcing it — after the
E0290 collision and `file_consts`' feature list — and the same lesson: the enforcement here is that
`file_signatures` is called on whichever tree is current, with no branch that can choose the other.

### §4 — The computed form is refused for everything but a library, with a named diagnostic

**A literal operand expands during `file_hir`** — before signatures, before const-eval — so it can generate
*anything*. `valid/136` generates a constant, a struct, a procedure, a nested insert and an empty insert, and
exits 63.

**A computed operand expands after const-eval**, so:

| generated | literal | computed |
|---|---|---|
| library declaration | works | **works** — needs no signature and no value |
| constant | works | no value: `#run` already ran |
| procedure / struct | works | no signature: the phase is past |

Three withholding sites had to learn "a file insert is pending" — name resolution, unknown types, and the
`#foreign` library lookup — mirroring what body-level inserts already do (`body_has_pending_insert`). Without
them, a program with a computed insert reported E0201/E0212 on the *generated* names before the text existed.

The unsupported half is **E0294**, not a leaked internal. It names the phase, and its help points at the two
things that do work: a literal insert, or `X :: #run pick();` for a value. Both refused shapes were internal
errors before this — "called a procedure taking 2 arguments with 1" and "a file-level item has no value until
jr-vm" — which is the well-typed-placeholder family AGENTS.md names, reported against the consumer rather
than the cause.

**Refusing rather than implementing** is the right trade here for the reason ADR-0150 gave: the fix is a
second const-eval pass over generated items, which is a wave, and a leaked ICE in the meantime teaches a user
that their correct-looking program broke the compiler.

### §5 — `modules/GL` is the proof, and it is a module rather than a compiler feature

```jairs
gl_library_declaration :: () -> string {
    if os() == Operating_System.MACOS   { return "gl :: #framework \"OpenGL\";"; }
    if os() == Operating_System.WINDOWS { return "gl :: #system_library \"opengl32\";"; }
    return "gl :: #system_library \"GL\";";
}

#insert #run gl_library_declaration();
```

Three names and **two different linker argument forms**, selected at compile time, in ordinary Jairs. That is
the shape the user asked for: *"per-OS belongs at compile time"*.

`modules/GL` binds constants and a small set of entry points and **calls none of them in any test**, because
every GL call needs a current context and `glGetString` with none *segfaults* on macOS rather than returning
null — measured, not assumed. The claim under test is that the symbols resolved, which is what linking means.

### §6 — `modules/File`'s hedged flags are unhedged

`CREATE`, `TRUNCATE` and `APPEND` were macOS numbers with a comment saying they were wrong on Linux — the
hedge ADR-0155 §1 named and PLAN §7 owed. They are now `#run` selections over `os()`, so the module is
correct on both targets. The corpus program that uses them exits 124 before and after, which is the
measurement that matters: the mechanism changed and the behaviour did not.

## Consequences

- **Per-OS support is a library concern.** A module can select a library, a link form, a flag or a value
  without a compiler change. That is the plan's Wave A, and it lands the plan's C1 and A3 items with it.
- **E0295 is the first free code.**
- Owed, and named rather than left: a computed insert generating a **constant or procedure** (§4's table),
  which needs a second const-eval pass; and per-OS **struct layouts**, which need more than a declaration
  because a layout is computed before comptime code runs.
- The formatter and tree-sitter both handled the new construct with no change — the first wave in thirteen
  where the formatter did not silently drop something. `run_decl`'s node shape is why.

## Verification

- **`tests/corpus/valid/136-file-scope-insert.jr`**, exit **63**, seven independent bits so a failure names
  itself: a generated constant, struct, procedure, two inserts in one file, a declaration generated *after*
  its use, a nested insert, and an empty one. Both engines run it, and there is nothing here for one to do
  differently — a generated declaration is an ordinary one.
- **`imports/invalid/020` and `021`** pin E0294 for the procedure and the constant. They live there, not in
  `type-errors/`, because E0294 comes out of the **expanded lowering** and that harness runs sema on the
  unexpanded tree: the seventh time a file has moved for a stage reason rather than the harness being
  weakened.
- **`a_per_os_library_is_chosen_by_comptime_code_and_linked`** builds, links and runs a program whose GL
  library was chosen by comptime code, and reads `otool -L` to prove the framework is really recorded.
- All seven gates green; the workspace suite passes.
