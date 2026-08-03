# ADR-0067: `switch` is a statement with exhaustiveness checked from the pool, and W4.5 moves before W4

- **Status:** Accepted
- **Date:** 2026-07-31
- **Deciders:** dboll
- **Opens W4.5 — Pattern matching**, and **amends PLAN.md §2.1's wave order**: W4.5 was placed after W4
  "because exhaustiveness diagnostics want comptime type info". §0 below shows that is a *want* and not
  a need, so this wave runs first. W4 is untouched and still next after W4.5's remaining work.
- **Settles two deferrals**: ADR-0041 §2 step 5 (a bare `.RED` as a case) and the first half of
  ADR-0045 §1's tagged-variant question, whose *matching* half this provides.

## Context

### 0. The wave order rested on a soft claim, and checking it moved the wave

PLAN.md §2.1 placed W4.5 after W4 with one reason: "exhaustiveness diagnostics **want** comptime type
info". That is a preference, not a dependency, and the difference is checkable. Exhaustiveness over an
enum needs one thing — the set of members the enum declares — and that set is already in the pool:

- `Pool::enum_members(decl)` exists and returns `Option<&[EnumMember]>` (ADR-0041 §4);
- it is **populated during checking**, by `jr-sema`'s `ctx.rs` (`set_enum_members`), so it is available
  in the phase that would report a non-exhaustive `switch`;
- `jr-sema` already reads it in three places, so this is not a new access pattern.

Two more prerequisites were verified by *running* rather than by reading:

```jr
Colour :: enum { RED; GREEN; BLUE; }
c := Colour.GREEN;
if c == .GREEN { … }          // bare member against an enum-typed value: works today
if c == Colour.GREEN { … }    // qualified: works today
```

Both branches are taken, in both engines. So `switch`'s two hard parts — resolving a bare `.RED` case
against the scrutinee's type, and comparing an enum value — are mechanisms the compiler already has.
`check_bare_member` resolves a bare member from an *expected type* (ADR-0046), which is exactly what a
case arm supplies.

**What comptime would add is real but not needed here**: RTTI would let exhaustiveness work over a type
computed at compile time, and `#insert` would let a `switch` be generated. Neither is what §2.1's row
asks for. Its three deliverables are a `switch`, exhaustiveness, and a tagged variant — and the first
two are reachable now.

**This is recorded as an amendment rather than done quietly**, because PLAN.md's own §5 names
"plans that contradict themselves" as a project failure mode, and a wave order justified by a
dependency that does not exist is one. The order changes; the reason for W4.5 preceding W5 (a polymorph
over a variant type needs the variant) is untouched.

### What Jairs has and has not

There is no `switch` and no `case` keyword — neither is lexed, reserved, or mentioned in any grammar
rule, so this adds surface rather than changing it. An `if`/`else if` chain over an enum is what a
program writes today, and it is unchecked: forgetting `BLUE` is silent.

## Decision

### 1. `switch` is a **statement**, not an expression

```jr
switch c {
    case .RED;   n = 1;
    case .GREEN; n = 2;
    case .BLUE;  n = 3;
}
```

A statement, for the reason `push_context` is one (ADR-0063 §5): making it an expression raises "what is
its type" and "what does a non-exhaustive expression evaluate to", and Jairs-0 has no place that needs a
`switch`'s value. An expression form is a compatible extension.

**`case <value>;` then statements, rather than `=>` or a braced block per arm.** The `case … ;` form
matches Jai, and it reuses the statement-list parsing every block already has — an arm is "statements
until the next `case` or the closing brace", so no new body shape enters the grammar. Braces per arm
were rejected as noise on the common one-statement arm; `=>` was rejected because it is not a token
Jairs has, and adding one for this would be the only place it appears.

### 2. Cases are **values, not patterns**

An arm's `case` takes an expression compared with `==`. It is not destructuring, not a range, not a
guard. That keeps this wave to what §2.1 asks and what the existing comparison machinery does: a
`switch` lowers to the chain of `==` tests a program would have written by hand.

**A bare `.RED` is legal as a case** (ADR-0041 §2 step 5, the deferral this settles), because the
scrutinee's type is the expected type an arm's value is checked against — the same context
`check_bare_member` already uses for `c == .GREEN`. Qualified `Colour.RED` is equally legal; they are
two spellings of one member.

### 3. Exhaustiveness is checked **for an enum scrutinee**, and is an error

A `switch` on an enum-typed value must name every member. A missing one is **E0258**, and the message
names the members that are missing rather than only counting them — the missing name is the fix, and a
count makes the reader re-derive it.

**An error rather than a warning.** ADR-0045 §1 rejected a tagged union partly because Jairs "has no
pattern matching" to make a tag worth reading; the point of adding matching is that the compiler can
then prove a case is handled. A warning would leave the proof optional, which is the same
"behaviour depends on something invisible" that ADR-0014 §3 refuses.

**Only for an enum.** A `switch` on an `s64` cannot be exhaustive in any useful sense, so it is
permitted without the check and needs an `else` to be total. Restricting exhaustiveness to the type
where the member set is *finite and known* is what makes the diagnostic true rather than approximate.

### 4. `else` is the catch-all, and it makes a `switch` exhaustive

```jr
switch n {
    case 0;  print("zero");
    else;    print("something else");
}
```

`else` rather than `default`, because `else` is already the keyword for "the other branch" in this
language and a second word for one idea is a second thing to remember. An `else` arm satisfies §3's
check — a `switch` with one is exhaustive by construction — and a **duplicate** `case` value or a
second `else` is **E0259**.

**An enum `switch` that names every member must not also have an `else`**: it is unreachable, and an
unreachable arm is a statement the reader believes runs. That is **E0260**, and it is the diagnostic
that makes §3's exhaustiveness worth having — without it, every `switch` could end in `else` and the
member check would never fire.

### 5. No fallthrough

An arm runs and the `switch` ends. C's implicit fallthrough is the most-regretted control-flow default
in the language's lineage, and Jai does not have it either. A program that wants two values to share an
arm is served by a future multi-value `case`, recorded in §6 as absent rather than faked with
fallthrough.

### 6. Lowered to a branch chain — no new MIR node

Each arm becomes the `==` comparison and `Terminator::Branch` an `if`/`else if` chain already lowers to,
with every arm's body jumping to one join block. Verified before deciding: `c == .GREEN` compiles and
runs in both engines today, so the pieces exist and neither back end changes.

**A jump table was rejected for this wave.** It is the reason a `switch` is faster than a chain, and it
needs the member values to be dense and the arms sorted — an optimisation over this lowering, not a
different one, so it can be added later without changing what a `switch` means. Recorded because a
reader who benchmarks will ask.

### 7. What is deliberately absent

- **A tagged variant type**, the third of §2.1's W4.5 deliverables and the other half of ADR-0045 §1.
  It needs a representation decision (where the tag lives, how a field read checks it) that is
  independent of `switch` and larger than it. Its own wave, now unblocked by this one.
- **`switch` as an expression** (§1), **patterns, ranges and guards** (§2), **multi-value `case`** (§5),
  **fallthrough** (§5), and a **jump table** (§6).
- **Exhaustiveness for a non-enum** (§3): an `s64` `switch` needs an `else`, and nothing checks a
  `bool` switch names both values — a `bool` scrutinee is an `if`, and treating it as an exhaustible
  set would be a second rule for a case the language already spells better.

## Consequences

- **PLAN.md §2.1's wave order changes**, and its W4.5 row's rationale is now wrong where it says
  "placed after W4". §0 is the argument; the handoff records it so the table and the ADR cannot drift.
- **Three new diagnostic codes**: **E0258** non-exhaustive `switch`, **E0259** a duplicate `case` or
  second `else`, **E0260** an `else` on an already-exhaustive enum `switch`. **E0261 becomes the first
  free code**, and this is the first wave in five to add any.
- **One new keyword each for `switch` and `case`**, so a program using either as an identifier now gets
  a parse error — the cost of any keyword, and the reason the grammar's keyword set is where it is
  visible. `else` is reused rather than a new `default`.
- **No new MIR node, no back-end change, no new pool item.** The lowering is the `if`/`else if` chain
  that exists (§6), which is the evidence this fits the shape the compiler already has.
- **`modules/Basic` is unaffected**; nothing in it wants a `switch` yet, and adding one to prove the
  feature would be a change to a library for a language reason.
- **The formatter must not lose it.** A statement kind absent from `is_stmt_kind` is *silently dropped*,
  which has cost source in eleven of thirteen waves — so `SWITCH_STMT` goes in that predicate in the
  same change that adds the node.
