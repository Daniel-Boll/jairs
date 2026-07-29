# ADR-0049: `for` iterates three known shapes, a label names a loop, and `defer` runs at every scope exit

- **Status:** Accepted
- **Date:** 2026-07-29
- **Deciders:** dboll
- **Scope:** W2's control-flow block only. `using`, multiple return values, named/default arguments
  and `#scope_*` are **deliberately not here** — see §7.

## Context

`PLAN.md` §2.1 gives W2 seven features and estimates 6–8 weeks. Taking them as one wave would mean
a single ADR deciding both a loop's iteration protocol and a name-resolution rule, which is the
plan-contradiction failure mode `AGENTS.md` names — and `using` is flagged in the same table as
"the first genuinely hard resolution problem". So this wave is the three features that share a
question: **what is a scope exit?** `defer` needs the answer, `break` and `continue` need it, and
a label changes which scope they exit.

Six facts were established by reading the code before this ADR was written, and three of them
shaped the decisions.

- **`while` already has everything a loop needs.** `while_stmt` pushes a header, a body, a
  *pre-exit* and an exit block, keeps every edge non-critical (ADR-0017 §1), and fills the header
  before sealing it because the back edge does not exist yet — the distinction `ssa.rs` keeps two
  bits for. `for` reuses that shape rather than inventing one.
- **`LoopFrame` has two fields, `header` and `exit`, and no name.** `jump` resolves `break` with
  `self.loops.last()`. **This is the fact that decides §2**: a label is a third field and a search,
  not a new mechanism.
- **`jump` already records a stray `break`** into `Facts::stray_jumps`, which E0229 reports. A
  `break` naming a label that does not exist is the same shape of error and reuses that channel.
- **`DOT_DOT` is already a token**, lexed and reserved for `[..]T` dynamic arrays. So `0..n` needs
  no lexer change — and it means the range form must not collide with the *type* form, which is
  what §1's "a range exists only inside a `for`" rule is for.
- **`Statement` has no "run these on the way out" concept**, and MIR terminators are set once per
  block. **This decides §3's mechanism**: a `defer` cannot be a terminator decoration, so its
  statements are *emitted* before each exiting terminator, at every exit.
- **`[N]T` and `[]T` differ in where the length comes from** — a constant from the type versus a
  load of `.count` (ADR-0044 §4) — and `Statement::BoundsCheck` takes its length as an operand for
  exactly that reason (ADR-0039 §1). A `for` over either reuses that, so iteration needs no new
  statement.

## Decision

### 1. `for` iterates exactly three shapes, known to the compiler

```jr
for x: buf        { … }   // [N]T or []T — x is the element
for i: 0..count   { … }   // a range — i counts up, count excluded
for x, i: buf     { … }   // the element and its index, both named
for < x: buf      { … }   // reverse
```

**Arrays, views and ranges.** No user-extensible protocol: the compiler knows these three and
nothing else. Jai's real design is `for_expansion`, a macro a type provides — genuinely better, and
it needs `#expand` and hygiene, which are W5. Pulling a whole wave forward to generalise a loop
would be the plan contradicting itself in the other direction.

**The loop variable is named, not implicit.** Jai defaults to `it` and `it_index`; Jairs requires
`for x: buf` and offers no implicit binding. The reason is that `it` is a *name introduced without
being written*, and ADR-0014 §3's whole position on names is that behaviour must not depend on
something invisible. `for x, i: buf` is three characters longer than `for buf` and says what both
names are.

**A range is not a type.** `0..n` is legal *only* between `for x:` and the loop body — there is no
`Range` in the pool, no `..` operator in the expression grammar, and `r := 0..n;` does not parse.
This is what keeps it from colliding with `[..]T`, and it means a range costs nothing anywhere
else in the language.

- **Half-open**, so `for i: 0..n` runs `n` times and `0..0` runs zero times. Matches every
  language that has ranges, and makes `for i: 0..buf.count` the natural array walk.
- **Both ends are ordinary expressions**, evaluated once before the loop. A range whose start
  exceeds its end runs zero times rather than trapping: an empty loop is a legitimate answer and
  trapping would make `0..n` unusable for a computed `n`.

**Rejected: arrays and views only, no ranges.** Smaller, and it misses the case people reach for
first — a counting loop. `while` covers arrays adequately already, so a `for` that could *only*
walk an array would add the least valuable half.

**`for < x: buf` reverses.** The `<` is a prefix marker rather than a `..` direction, because
reversing a *range* and reversing an *array* have to be spelled the same way and only a marker on
the `for` can do both. Jai spells it exactly this way.

### 2. A label names a loop; `break` and `continue` may name one

```jr
outer: for a: rows {
    for b: cols {
        if bad(b)  break outer;
        if skip(b) continue outer;
    }
}
```

A label is `name:` before `for` or `while`, and `break name` / `continue name` targets that loop.
Unlabelled `break` still means the innermost, unchanged.

`LoopFrame` gains a `label: Option<Symbol>` and `jump` searches **outward from the innermost**
rather than taking `last()`. That is the whole implementation: no new blocks, no new terminator,
because a labelled `break` jumps to a frame that already has an `exit`.

**A `break` naming an unknown label reuses E0229's channel** — the stray-jump fact MIR already
records — rather than inventing a code. The *message* distinguishes them, because "there is no loop
labelled `outer`" and "a `break` outside a loop" are different mistakes with the same shape.

**Rejected: labels on arbitrary blocks.** `outer: { … break outer; }` is a labelled *block*, which
is a different feature: it makes a block an early-exit construct rather than naming a loop. Useful,
and out of scope — `PLAN.md` §2.1 says "labeled `break`/`continue`", which is about loops.

**Rejected: numeric levels (`break 2`).** Shorter to implement and a maintenance trap: inserting a
loop silently changes what `break 2` means. A name is checked.

### 3. `defer` runs at scope exit, in reverse order, on every path out

```jr
{
    a := open();
    defer close(a);       // runs at the closing brace
    if bad  return;       // and here
    for x: xs {
        defer step();     // runs once per iteration
        if x == 0  break; // and here
    }
}
```

**The scope is the enclosing block**, so a `defer` in a loop body runs *per iteration*. That is
Jai's and Go's behaviour, and the alternative — accumulating until the procedure returns — is
surprising in the case where cleanup matters most.

**Reverse order within a scope**, so `defer a(); defer b();` runs `b` then `a`. Anything else makes
paired acquisition and release inexpressible.

**Every path out runs the defers of every scope it leaves.** `break` out of two scopes runs both
sets, innermost first. The mechanism is forced by MIR's shape: a terminator is set once and carries
no statement list, so `build.rs` *emits the deferred statements before each exiting terminator*.
A `defer` therefore appears in the MIR once per exit path, which is duplication of statements and
not of evaluation — the deferred expression is evaluated where it runs, exactly once per exit taken.

**Not on a trap.** A trap ends the process (ADR-0002), so nothing runs afterwards. Stated because
the natural expectation from a language with unwinding is the opposite, and promising cleanup that
cannot happen is worse than not promising it.

**The deferred statement is arbitrary**, not restricted to a call: `defer count = count + 1;` is
legal, because restricting it would need a rule about what "a cleanup" is.

**Rejected: procedure-scope only.** One list per body, emitted before each `return`. Simpler, and
it makes a `defer` in a loop accumulate — differing from both Jai and Go for no gain.

**Rejected: refuse `defer` inside a loop.** Dodges the per-iteration question by refusing the
construct, which refuses the case cleanup matters most for. A refusal that exists only to avoid a
decision tends to become permanent.

### 4. What `for` lowers to, and why it needs no new MIR

An array or view loop is the `while` shape with an induction variable:

```text
  i = 0                    // synthesised local, not user-visible
header:  i < len ? body : exit
body:    bounds_check i < len
         x = load base[i]  // the user's loop variable
         …
         i = i + 1
         goto header
```

`len` is the array's constant or a load of the view's `.count` — the *same* operand
`Statement::BoundsCheck` already takes (ADR-0039 §1), so a `for` over a view needs nothing new.
A range loop is the same without the bounds check and with the range's ends as bounds.

**The bounds check stays.** A `for` provably stays in range, so const-prop could delete it — and
emitting it anyway is right: ADR-0003 made the check an explicit statement precisely so a pass
*proves* it redundant rather than lowering skipping it, and a `for` whose bound was miscompiled
would otherwise read out of bounds silently.

**The induction variable is a synthesised local**, subject to the same promotion rules as any
other (ADR-0017 §2). `for x, i: buf` binds `i` to that variable's value; `for x: buf` synthesises
one the user cannot name.

**The loop variable is a *copy*.** `for x: buf { x = 0; }` modifies the copy, not the array. That
is Jai's behaviour and it follows from `x` being a local: iterating by reference needs a pointer
form (`for *x: buf`), which is out of scope and recorded as owed.

## Consequences

- **Three new keywords become real** — `FOR_KW`, `DEFER_KW` and the label form — and each must
  leave the tree-sitter *reserved* match. `cast`, `enum`, `union` and `xx` each made that trip;
  §7's trap list has it, so it is checked rather than discovered.
- **`Stmt` gains `For`, `Defer`, and `Break`/`Continue` gain an optional label.** Every exhaustive
  match over `Stmt` therefore changes: `jr-hir`'s dump and resolve, `jr-sema`'s checker, `jr-mir`'s
  `scan`, `stmt` and the escape walk. The compiler lists them.
- **A label is a *new kind of name*** and deliberately **not** in the `ResolveMap`: it names a loop,
  not a value, and putting it in the expression-name map would make `break outer` look like a name
  reference to anything reading that map. `build.rs` resolves it against its own `loops` stack,
  which is the only place the answer exists.
- **`defer` is the first construct whose statements appear more than once in the MIR.** Nothing
  breaks — SSA is per-block and a duplicated `Store` is two statements, not one shared — but a
  snapshot will show the duplication, and a reader who expects one-statement-per-source-statement
  should know it is intended.
- **`for` is the first loop with a *synthesised* local.** `jr-hir` must allocate one that has no
  name a user could write, which is the same trick ADR-0048 used for `operator+` and for the same
  reason: nothing can collide with a name that is unspellable.
- **Two new diagnostic codes**: E0247 (a `for` over something not iterable) and E0248 (a `break`
  or `continue` naming an unknown label), making **E0249 the first free code**. E0127 is the first
  free *parser* code, and the parser needs one for a malformed `for`.
- **A corpus program must observe a `defer` running on the `break` path**, not merely at the
  closing brace, because §3's "every path out" is the claim most easily got wrong — and a `defer`
  that only ran on the fall-through would look correct in a program that never breaks.
