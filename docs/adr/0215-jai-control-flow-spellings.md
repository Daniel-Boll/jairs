# ADR-0215: Jai control-flow spellings reuse Jairs' existing branch semantics

- **Status:** Accepted
- **Date:** 2026-09-08
- **Deciders:** dboll
- **Amends:** ADR-0067 §§3–4 (enum exhaustiveness and duplicate cases)

## Context

The Tsoding-shaped source probe reaches two Jai spellings that Jairs cannot currently read:

```jr
if byte == #char "<" then inside = true;

if #complete token == {
    case .OPEN;  depth = depth + 1;
    case .CLOSE; depth = depth - 1;
}
```

The missing runtime mechanisms are much smaller than the missing surface suggests. Jairs already
accepts a braceless single-statement `if`, and ADR-0067 already provides an enum-exhaustive
`switch` that lowers to an `if`/`else if` branch chain. The compatibility work should therefore
name those existing meanings rather than create a second control-flow system.

Probing the existing `switch` exposed three defects that the new spelling would otherwise inherit:

- two integer cases with the same value are accepted;
- two enum aliases with the same runtime value count as different coverage even though both lower to
  the same equality test;
- an `else` may appear before a later `case`, after which lowering silently moves it to the end.

The decider chose optional `then`, exact `if #complete … == { … }` sugar, and repair of all three
shared switch invariants in the same wave.

## Decision

### §1. `then` is an optional keyword on a braceless `if` body

Both forms are equivalent:

```jr
if ready run();
if ready then run();
```

`then` appears only between an `if` condition and one braceless statement. A braced body remains:

```jr
if ready {
    run();
}
```

and `if ready then { … }` is a parse error. `while` and other control forms do not gain the
keyword. This keeps `then` as punctuation for the one source shape that asks for it rather than as
a second spelling for every block.

`then` is reserved, represented in the CST, and preserved by the formatter when written. The
formatter neither inserts it into the older spelling nor removes it from the Jai spelling.

### §2. `if #complete value == { … }` is exact syntax sugar for `switch value { … }`

The accepted shape is:

```jr
if #complete value == {
    case a; statements
    case b; statements
    else;   statements
}
```

It is a statement. `#complete` cannot decorate an ordinary boolean `if`, and `== {` does not become
a general pattern or expression form. Lowering constructs the existing HIR switch statement and
switch arms, so checking, variant destructuring, MIR branch construction, no-fallthrough behaviour,
and all back ends remain shared with `switch`.

The spelling consequently has the same type domain and exhaustiveness rules as `switch`: an enum is
checked against its finite values; an integer has no finite declared set; `else` is the catch-all.
`#complete` introduces no second or stronger checker.

### §3. A statically readable case is identified by the runtime value compared by the branch

ADR-0067 §2 defines a case as a value compared with `==`. Duplicate detection and exhaustiveness
therefore use that same value whenever sema can read it without invoking the downstream const
evaluator:

- equal integer literals, whatever their radix spelling, are E0259;
- enum members or aliases with one underlying integer value form one coverage class;
- naming two aliases from one class is E0259;
- naming any alias covers that value class for exhaustiveness.

For an uncovered enum value class, E0258 names its first-declared member as the stable representative.
It does not list every alias as though several branches were required.

This amends ADR-0067 §3's phrase "must name every member": it must cover every distinct runtime value
declared by the enum.

An arbitrary call, variable, or expression remains a legal value case but is not proof of a
compile-time duplicate or of enum coverage. This preserves ADR-0067's existing boundary: checking
does not run the const evaluator merely to classify a case.

### §4. `else` is final in source order

For both `switch` and complete-if, an `else` arm must be last. A later `case` is E0134 at parse time.
The parser owns the refusal because source ordering is syntactic, and rejecting it there prevents
lowering from receiving a tree whose order it cannot preserve.

A second `else` remains E0259 when it reaches semantic checking, and an `else` after complete enum
coverage remains E0260. These diagnostics answer different questions: duplicated catch-all,
unreachable catch-all, and source after catch-all.

### §5. No new HIR, MIR, pool, or backend representation

`then` is source punctuation on the existing `if`. Complete-if lowers to the existing switch HIR.
Value-based duplicate and coverage checks use the enum values already interned in the pool. No
runtime operation changes, so gate 7 is not required by this wave unless implementation unexpectedly
touches MIR or a back end.

## Rejected alternatives

- **Make `then` mandatory for every braceless body.** It would break valid Jairs source for no
  semantic gain.
- **Allow `then` before a block.** Braces already delimit the body, so the keyword would carry no
  information and create two canonical block spellings.
- **Parse complete-if as a special boolean expression.** Its arm list, exhaustiveness and
  no-fallthrough semantics already have one representation in `switch`; duplicating them invites
  drift.
- **Add general patterns, ranges or guards.** The requested spelling does not require them, and
  ADR-0067 deliberately keeps cases as values.
- **Count enum aliases by declaration name.** Two names with one runtime value produce one equality
  branch. Treating them as separate coverage is a proof about syntax that execution cannot honour.
- **Permit a case after `else` and reorder it.** The formatter and lowerer must not silently change
  control-flow order.

## Consequences

Jai source may opt into `then` without making it a Jairs requirement, and the complete-if spelling
inherits the tested switch path all the way through execution.

E0134 is consumed by a case after `else`; E0135 becomes the first free parser code. The semantic
repairs reuse E0258–E0260 and consume no global diagnostic code, so E0297 remains the first free
workspace code.

`then` becomes a reserved identifier. `#complete` is completed by the language server and highlighted
as a directive, while `then`, `case`, and `else` are keywords.
