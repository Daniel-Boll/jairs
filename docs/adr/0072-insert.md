# ADR-0072: `#insert` of a literal string, lowered where it is written

- **Status:** Accepted
- **Date:** 2026-07-31
- **Deciders:** dboll
- **W4 sub-wave 4, scoped to a literal operand.** §2.1 lists the sub-wave as "`#insert` and the `Code`
  type"; §4 explains why a *computed* operand and a `Code` value are each their own decision, and the
  reason is a dependency direction rather than a scheduling preference.

## Context

### 0. What running found

`#insert` and `#code` are both genuinely absent, which is worth stating because the last two sub-waves
each found their scheduled work already done (ADR-0067 §0, ADR-0070 §0):

```
#insert "x := 1;";     // error[E0209]: `#insert` is not valid here
c := #code x := 1;     // error[E0100]: expected `;`, found an identifier
                       // error[E0209]: `#code` is not valid here
```

So this sub-wave has real content. Three further facts, all checked rather than assumed:

- **`jr-hir` already depends on `jr-syntax`**, and `jr_syntax::parse` is public. So lowering *can* parse
  a string of source without any new dependency — which is the whole reason a literal `#insert` needs
  none of W4's mutual recursion.
- **The parser already produces the node.** `#insert "text"` lexes as one directive token plus a string
  and parses as the generic `DIRECTIVE_EXPR` with a `string_arg`, because the lexer is deliberately
  permissive about `#anything` (`DIRECTIVES_VALID_AS_EXPRESSIONS`' docs say so). **No grammar change,
  no lexer change, no new `SyntaxKind`** — which also means gate 6 has nothing new to check.
- **A `Span` is `(FileId, TextRange)` into a real file**, and offsets past EOF are **clamped**, not
  rejected: `jr-diag`'s renderer takes `.min(primary_len)` on every offset so it "never panics on
  out-of-range spans". That is the fact that decides §2. A span into synthesized text is not caught by
  anything — it silently points at whatever byte range happens to be there.

### The span problem is the whole design problem

Inserted code has no position in any file. Every alternative therefore has to answer: what does a
diagnostic *inside* generated code point at? Getting this wrong is not a cosmetic failure, because the
renderer clamps: a wrong span underlines real source that the user did not write, and says the error is
there.

## Decision

### 1. `#insert "…"` parses its operand as Jairs source and lowers it in place

```jr
main :: () {
    #insert "n := 2 + 3;";
    exit(n);                 // sees `n` — the insert declared it in *this* scope
}
```

The operand is a **string literal**. Lowering calls `jr_syntax::parse` on it, then lowers the resulting
statements into the enclosing body as though they had been written there — same scope, same arenas, same
`ResolveMap` and `TypeMap` entries. That is what makes `exit(n)` above resolve: an insert is not a nested
scope, it is *textual* substitution that happens after parsing rather than before.

**Lowered in `jr-hir`, not in the parser.** A pre-parse textual splice would make `#insert` a macro over
bytes, and the CST would then have no node for the directive at all — so the formatter would delete it
(`is_stmt_kind`'s failure mode, which destroyed source four times in one wave) and the LSP would have
nothing to hover. Doing it in lowering keeps the CST lossless, which is ADR-0026's whole premise.

### 2. Every synthesized node's span is the `#insert` directive

Not a synthesized offset, not a clamped one: the directive's own `TextRange`, in the real file.

**This is honest rather than a compromise.** The `#insert` *is* where that code entered the program —
there is no other place it exists. And it is always in range, so §0's clamping can never fire on it.

**Rejected: a synthesized `FileId` for the inserted text.** Registering the string as its own source file
would give genuinely-real spans into it, and a diagnostic could underline the generated line directly. It
is the better *message* and it was rejected on cost: a `FileId` is an index assigned in database load
order — AGENTS.md forbids printing one into a snapshot for exactly this reason — and every consumer that
maps a `FileId` to a path would need to learn about a file with no path: the `SourceMap`, the module
loader, salsa's inputs, and the LSP's document store. That is four subsystems changed to improve one
message, and it can be added later without changing what programs mean.

**Rejected: pointing at the directive and saying nothing more.** Simplest, and it produces a diagnostic
the reader cannot act on as soon as an insert has more than one statement — "the error is somewhere in
this string". ADR-0043's lesson was that a diagnostic which is true and useless is a diagnostic people
learn to ignore.

### 3. A diagnostic in inserted code says so, and names the offset

```text
error[E0201]: unresolved name `undefined_thing`
  --> file.jr:4:5
   |
 4 |     #insert "y := undefined_thing;";
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = in inserted code, at offset 5
   = the inserted text was: y := undefined_thing;
```

The primary span is §2's; the notes carry what the span cannot. The offset is a byte position into the
*inserted string*, so a reader can find the mistake in text they generated.

**A parse error in the operand is reported the same way**, with its own code (§5) rather than the
parser's: the parser's codes describe positions in a file, and this text is not in one.

### 4. A computed operand and a `Code` value are each a later decision

`#insert build_it()` — where the text comes from a `#run` — is the feature W4 §2.1 really means, and it is
where sema and the VM become mutually recursive. The reason is a dependency *direction*, not difficulty:
lowering produces the HIR that `resolved` consumes, which `checked` consumes, which `file_consts` consumes.
An `#insert` asking for a computed value would need `file_hir` to depend on `file_consts`, closing

```text
file_hir → file_consts → checked → resolved → file_hir
```

which is a salsa cycle — the same shape ADR-0069 §1 hit and had to restructure around, and the same
`file_consts`-is-downstream fact ADR-0018 §3 established and ADR-0070 §1 and ADR-0071 §2 both relied on.
Breaking it is the sub-wave's own problem, and PLAN §5 calls it the project's top risk.

A **`Code` value** (`c := #code x := 1;`) needs a representation for a quoted syntax tree, and the first
question is the one ADR-0071 §4 asked about a type: does it exist at run time? If it does not — which is
the likely answer — it is comptime-only like `Item::TypeType`, and then it needs the same treatment
`Type` just got. Either way it is only useful once something can splice it, so it follows `#insert`
rather than preceding it.

### 5. What is deliberately absent

- **A computed or named operand** (§4). `#insert CODE;` where `CODE` is a constant is refused too, even
  though its value is a string the signature phase may already know — because the general case needs §4's
  cycle broken, and accepting the easy case would make the refusal depend on how the string was written.
- **`#code` and the `Code` type** (§4).
- **`#insert` at file scope.** An insert that declares a *procedure* is a different thing: it changes the
  item tree, so `ItemScope` and the whole signature phase would see declarations that no `#import` and no
  file walk produced. Body scope only, and the refusal says so.
- **Nothing about nesting.** An insert whose text contains another **works**, and it needed no code: the
  recursion falls out of `lower_stmt` calling itself. This section's draft said nesting was deferred
  because it "needs a depth bound, and an unbounded one is a compiler hang" — that is wrong, and running
  it is what showed why. **Escaping doubles the text at every level**, so the operand of an *n*-deep
  nest is exponential in *n*: 12 levels is 8 KB of source, 18 levels is 512 KB, and 40 levels would be
  ~10¹² bytes, which cannot be written. A literal `#insert` therefore cannot recurse without bound —
  the bound is the file. Verified: 12 and 18 levels both compile, run and exit correctly.

  A depth bound will be needed the day §4's *computed* operand arrives, because a generated string can
  reproduce itself without growing. That is the sub-wave that owes it.
- **An insert that declares nothing** is legal: `#insert "";` inserts no statements. Refusing it would be
  a rule about a program that means exactly what it says.

## Consequences

- **`#insert` becomes the first construct whose HIR has no one-to-one CST node.** Every synthesized
  statement's span points at the directive, so several statements share one span — which the `TypeMap` and
  `ResolveMap` handle fine, because both key on `ExprId` rather than on a span.
- **Two new diagnostic codes**: one for an `#insert` whose operand is not a literal string (§5), one for a
  parse error inside the inserted text (§3). **E0264 becomes the first free code.**
- **The formatter and the LSP need no change**, which is the payoff for §1's choice: the CST still has an
  ordinary `DIRECTIVE_EXPR` where the `#insert` was written, so `jr fmt` round-trips it and hover finds a
  node. This was checked against the four times a missing `is_stmt_kind` entry silently deleted source.
- **No grammar change, no lexer change, no new `SyntaxKind`** (§0), so gate 6 has nothing new to validate
  and `grammar.js` is untouched — which matters given that gate 6 checks drift by regeneration and cannot
  see a reversion.
- **`jr-hir` gains a second call into `jr-syntax`**, alongside the one that produced the tree it is
  lowering. Worth naming: lowering is no longer a pure function of *one* parse tree, though it remains a
  pure function of its inputs.

### What implementing it added to the above

- **E0262's corpus file belongs in `imports/invalid/`, not `type-errors/`, and the rule is the *stage*.**
  `type-errors/`' harness asserts its files "parse, lower and resolve cleanly" and *then* report exactly
  their declared code as reported by `jr-sema`. E0262 comes out of **lowering**, so a file expecting it
  fails the first half before the second is reached — which is what happened: two `jr-sema` corpus tests
  failed, one saying the file did not lower cleanly and one saying it reported nothing. ADR-0050's three
  `using` refusals (E0250) are in `imports/invalid/` for exactly this reason, so the precedent already
  existed and importing nothing is something that directory permits. Filed as
  `imports/invalid/011-insert-needs-a-literal.jr` and added to `imports_invalid_corpus_fails`' list, which
  a drift test requires.
- **E0263 is a re-wording, and the parser's own code for the same fault is *reused*.** `parse_stmt_list`
  raises **E0114** ("a token that cannot start a statement") because the fault is identical to the one
  inside a block and only the text it indexes differs. `jr-hir` re-points and re-words it as E0263 before
  a reader sees it, per §3. So the wave defines two codes and reuses one.
- **The one number that separates the two designs is an exit status, and nothing asserted it.** The corpus
  differential checks only that the two engines *agree*: giving `Stmt::Insert` a defer scope of its own
  makes both exit **63** in perfect agreement, and the entire suite stays green but for a MIR snapshot diff
  a reviewer can accept without noticing. Verified by making exactly that change. `059-insert.jr`'s
  `defer exit(n)` is written in inserted text with an `n = n + 1` after it, so 64 is the assertion that the
  inserted `defer` belongs to the **enclosing** body — and it now has its own test
  (`an_inserted_defer_runs_when_the_enclosing_body_is_left`) rather than resting on a snapshot. This is §5's
  "when a claim is about a representation, dump the representation" one step on: when a claim is about
  *behaviour*, assert the behaviour.
- **The formatter payoff was checked rather than assumed**: `059-insert.jr` round-trips byte-identically
  through `jr fmt`, and the 166 Neovim checks still pass.
- **Eight tests (936 → 944)**, seven in `jr-hir` and one differential. Each teeth-checked by disabling the
  mechanism it pins: neutering the span override fails exactly the two span tests, pushing a scope for the
  insert fails exactly the enclosing-scope test, and giving it a defer scope fails exactly the exit-status
  test. Different flips, different failures.
