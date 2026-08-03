# ADR-0073: `#insert` of a computed string, and the cycle broken by a pre-pass

- **Status:** Accepted
- **Date:** 2026-08-03
- **Deciders:** dboll
- **W4 sub-wave 5.** ADR-0072 §4 deferred a computed operand and named the reason: it would make
  `file_hir` depend on `file_consts`, which is a salsa cycle. This is the sub-wave that owes the answer,
  and PLAN §5 calls it the project's top risk. It also owes the **depth bound** a literal `#insert` did
  not need (ADR-0072 §5).

## Context

### 0. What running found, and why it splits the wave differently than PLAN did

PLAN §7 described W4's remainder as "one problem with two faces": `type_info()`/`Any` needing a type as
runtime data, and a computed `#insert` needing the cycle broken. **Probing shows those are two different
problems, and only one of them is this cycle.** Four facts, each checked rather than assumed — the habit
that has now caught a false schedule three waves running (ADR-0067 §0, ADR-0070 §0, ADR-0072 §5):

- **A `#run`-computed *string* constant already works.** `S :: #run mk();` where `mk` returns a `string`
  checks cleanly today, with no diagnostic. So the value a computed `#insert` needs is one const-eval
  already produces; nothing about the *string* case is missing.
- **A `#run`-computed *struct* is refused**, by name: `E0230`, "a compile-time struct value arrives with a
  later wave". `jr-db`'s `reduce` cannot intern one because `jr-pool`'s `Item` **has no aggregate value
  variant at all** — the variants are `IntValue`, `FloatValue`, `StrValue`, `TypeValue`, `ProcValue`,
  `ForeignLibraryValue` and the void/bool pair, and that is the whole list. `type_info()` returns a struct,
  so it is blocked on a *representation* prerequisite that has nothing to do with a dependency direction.
- **`#insert S;` does not parse in the *compiler* today**, quite apart from E0262: its parser expects a
  string after `#insert`, so a bare name produces E0100 *first* and then E0262. The compiler's parser
  therefore needs a change — but the **tree-sitter grammar does not**, and this was checked rather than
  assumed. The grammar parses every directive generically as `run_expr` (directive + expr), so
  `#insert S;` and `#insert mk();` already parse there with **no ERROR node**, which is all gate 6's drift
  check asks. An earlier draft of this ADR claimed "gate 6 acquires new work"; running `tree-sitter parse`
  on a computed insert showed that was false, so gate 6 has nothing new after all — the same
  no-grammar-change position ADR-0072 §0 reached, reached again for a different reason.
- **`file_signatures` depends only on `file_hir` and `resolved`** — never on `checked`, `file_consts` or
  `file_mir`. That is the fact §1 turns on, and it was read out of the query rather than assumed.

So the two remainders are unblocked in different ways and this one is genuinely the smaller: it needs no
new pool variant.

### The cycle, stated precisely

ADR-0072 §4 drew it as `file_hir → file_consts → checked → resolved → file_hir`. Reading the queries makes
it sharper, and the sharper version matters because it names where the break can go:

```text
file_consts  → frontend_diagnostics → checked → resolved → file_hir
             → file_hir
```

`file_consts` is gated on `frontend_diagnostics` (ADR-0017 §4: no MIR from a file with errors, and a thunk
is MIR), and `frontend_diagnostics` reaches `checked`, `resolved` and — **directly, not through
`file_hir`** — `jr_hir::lower_file`. So a `file_hir` that asked for a constant's value would close the loop
through the error gate, not merely through the type checker.

## Decision

### 1. A narrow pre-pass query, `insert_operands`, evaluates only what an `#insert` needs

```text
insert_operands(file, search_paths)
    → parse_file, file_signatures        (and so file_hir, resolved — never `checked`)
    → evaluates *string-valued* constants only
file_hir_with_inserts(file, search_paths)
    → parse_file, insert_operands
```

The graph stays **acyclic**, which is the whole point: nothing depends on a query downstream of it. This is
ADR-0069 §1's precedent applied a second time — that wave hit this same shape, took the imported file's MIR
from `file_mir`, got salsa's cycle panic in three corpus tests at once, and restructured to lower the MIR
locally instead. The lesson recorded there was that restructuring is the honest position rather than a
workaround.

**Rejected: salsa's fixed-point cycle recovery.** salsa 0.28.1 *does* support it — `cycle_fn` plus
`cycle_initial`, iterating from an initial value until stable — and this ADR is the first place in the
project to consider it, because the plan never mentioned it existed. It is the more general answer: it
would also serve a `#run` that reads another file's constant, which is one of the three refusals waiting on
this wave. Rejected on two grounds. First, **convergence would have to be proved rather than observed**: an
insert whose text declares a constant that another insert's text reads is a fixpoint whose termination is a
property of the *program*, not of the compiler, and a wrong fixpoint is a silently wrong program — the
failure mode PLAN §5 names first. Second, and decisively, **opting a query into recovery removes salsa's
cycle panic as a guard**. That panic is what caught ADR-0069's mistake immediately and cheaply; turning it
off for `file_hir` would mean the next accidental cycle anywhere in the front end converges quietly
instead of failing loudly. A general mechanism that disables the project's best cycle detector is a poor
trade for a feature that a narrow query delivers acyclically.

**Rejected: evaluating the operand in the parser or a pre-lowering textual pass.** It would dodge the
cycle entirely by running before any query — and it cannot work, because the operand's *value* comes from
a `#run`, which needs MIR, which needs the front end. This is recorded because it is the first thing that
suggests itself.

### 2. Only a string-valued constant, and the pre-pass says so

`insert_operands` evaluates a constant whose type is `string`. Anything else is refused with a diagnostic
naming the type, rather than evaluated and rejected later.

**This is a narrower rule than "any constant", deliberately.** A general pre-pass would be a second,
partial const-eval — and two evaluators that must agree is precisely the shape ADR-0019 refuses for the
two *execution* engines, on the grounds that a plausible argument for agreement is not a check. Restricting
to the one type `#insert` can consume keeps the pre-pass a *lookup* of something `file_consts` also
computes, so the two cannot disagree about anything a program can observe.

### 3. A depth bound, because a generated string can reproduce itself

ADR-0072 §5 established that a *literal* `#insert` cannot recurse without bound: escaping doubles the text
at every level, so 18 levels is 512 KB and the file itself is the bound. **A computed operand removes that
bound** — a generated string can reproduce itself without growing, which is a quine, and the ADR named this
sub-wave as the one that owes the guard.

The bound is on **insert-expansion depth**, not on total inserts: a file may contain any number of
inserts, and each may expand to a fixed depth. Exceeding it is a diagnostic that names the depth and the
directive, not a panic — a compiler that hangs or aborts on a program is the one failure mode a compiler
must never have, which is the argument `LayoutError::Recursive` already makes for a recursive struct and
`E0199` for parser nesting.

### 4. What is deliberately absent

- **A non-string operand** (§2), refused with the type named.
- **`#insert` at file scope**, still (ADR-0072 §5): it changes the item tree, so the signature phase would
  see declarations no `#import` and no file walk produced. Unchanged by this wave, and worth restating
  because a computed operand makes it *more* tempting rather than less.
- **`#code` and the `Code` type** (ADR-0072 §4), still following rather than preceding.
- **`type_info()` and `Any`**, which §0 shows are blocked on a pool aggregate-value representation rather
  than on this cycle. They are their own sub-wave, and the E0230 refusal is the thing to lift first.
- **A `#run` reading another file's constant** stays refused. It is one of the three refusals PLAN says
  come free with this wave — and §1's narrow query is precisely what does *not* deliver it, since the
  general mechanism that would was rejected. Saying so plainly, because the plan claims otherwise.

## Consequences

- **The front end gains a second lowering entry point.** `file_hir` stays as it is — depending only on
  `parse_file` — and a second query lowers *with* operands. Two lowerings of the same file is a cost worth
  naming; it buys the acyclic graph, and salsa memoises both.
- **Gate 6 has nothing new**, despite the compiler's parser needing a change. The tree-sitter grammar
  parses every directive generically as `run_expr` (directive + expr), so a computed insert already parses
  there with no ERROR node — verified with `tree-sitter parse`. `grammar.js` is untouched, as it was for
  ADR-0072, though this time only the *compiler's* parser moved.
- **The depth bound is a new diagnostic** (§3), and the first refusal in this project whose trigger is a
  program that is *legal at every individual step*.
- **PLAN §7's framing of W4's remainder was wrong and is corrected**: the two remaining features are not
  one problem, and the one that looked harder (this) is the one that needs no new representation.
