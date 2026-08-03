# ADR-0071: a type is a compile-time value, and using one at run time is refused

- **Status:** Accepted
- **Date:** 2026-07-31
- **Deciders:** dboll
- **W4 sub-wave 3, scoped to `Type` values.** §2.1 lists RTTI as "`Type` values, `type_info()`, `Any`";
  §4 explains why `Any` is a *later* sub-wave rather than this one, and the reason is a layout fact
  rather than a scheduling preference.
- **Closes a silent miscompile**, not only a gap: §0 records what running found.
- **This draft's §1 was wrong and is replaced.** It proposed making `Type` spellable so that a constant
  could be annotated with it — but `T : Type : Point;` does not parse (E0100: the parser has no
  annotated-`::` form), and *no* type annotation can resolve to `PoolId::TYPE`, because
  `resolve_type_name` answers with a `SigEntry`'s `type_value` and no writable declaration has
  `type_value == PoolId::TYPE`. So the spelling would have had no position that wanted it and a set of
  refusals guarding positions that cannot arise. §1 now says what is true instead: a type is bound with
  `::` and is not spellable as a type. Found by running, before any code was written.

## Context

### 0. A type in a runtime body was a well-typed placeholder

`t := Point;` — a bare type name bound to a local — **type-checks cleanly today and compiles in both
engines**. Its MIR is:

```text
proc main -> void {
  slots:
    s0: type          // a slot of a type with no runtime layout
  bb0(v0: *Context):
    v1: type = undef  // a placeholder for a category error
    store s0 <- v1
```

That is exactly the shape PLAN.md §5's first failure mode names: *a construct the grammar allows, no
representation on the lowering path, filled in with a placeholder that is a legitimate value.*
`Rvalue::Undef` is a legitimate value — `c: s64 = ---;` produces one — so neither the verifier nor
ADR-0017 §4's poison gate can catch it. Both engines accepted the program and exited 0, because nothing
read the slot.

The type it stores into has **no runtime representation at all**: `layout_of` answers
`LayoutError::ComptimeOnly` for `Item::TypeType`, whose own docs say these "exist only during
compilation… asking for their runtime size is a category error, distinguished from `Layout::ZERO`
deliberately". So a `type`-typed slot is not a small slot — it is a slot of something that cannot be
stored, and the compiler was making one anyway.

Found by running rather than by reading: sema reports no error, both engines exit 0, and only the MIR
dump shows it. This is the third wave in a row where a claim survived because nothing displayed the
thing that would have contradicted it.

### And a type alias does not work

The other half, also found by running:

```jr
Point :: struct { x: s64; }
T :: Point;                  // E0230: compile-time evaluation failed:
                             //        a file-level item has no value yet
```

A type alias is a natural thing to write, and the message is a const-eval internal. The cause is
narrow: `file_consts` deliberately does not treat a struct as an evaluation target (its `wanted` docs
say a struct's "value is a declaration rather than something to compute"), so the thunk finds no entry
for `Point` and refuses with a message about const-eval rather than about types.

**The pool is already ready for both.** `Item::TypeValue(PoolId)` exists, `Pool::type_value` interns one,
and `Pool::type_of` answers `PoolId::TYPE` for it. Nothing in the front end has ever produced one.

## Decision

### 1. A type is bound with `::`, and `Type` is deliberately **not** spellable

```jr
T :: Point;            // a constant whose value is a type — this is the whole spelling
```

There is no annotated form, and that is a decision rather than an omission. Two facts, both established
by running before this was written:

- **`T : Type : Point;` does not parse.** The parser has no annotated-`::` declaration form at all — it
  reports E0100 "expected `;`, found `:`" — so there is no position in the grammar where a `Type`
  annotation could appear.
- **No annotation can resolve to `PoolId::TYPE`.** `Ctx::resolve_type_name` answers a name by reading a
  `SigEntry`'s `type_value`, which for every nominal declaration is the *declared* type — `Point`, not
  `type`. Nothing sets `type_value` to `PoolId::TYPE`, so even adding `"Type" => PoolId::TYPE` beside
  `bool` and `string` would only make `t: Type;` reachable, which §3 then has to refuse.

So making `Type` spellable would add one spelling whose every use is an error. Left out, and E0212
continues to report `t: Type;` as an unknown type name — which is the accurate answer, since Jairs has no
such type annotation.

**What this costs, stated plainly:** a diagnostic cannot point at a spelling of the type of a type. §3's
message therefore says "a type is a compile-time value" and names the positions that accept one, rather
than naming a type the reader could write. That is the better message anyway: the fix is to move the
type into a `::`, not to annotate it differently.

### 2. A type-valued constant is evaluated, and its value is an `Item::TypeValue`

`file_consts` gains a target kind: a constant whose initialiser resolves to a type. Its value is
`Pool::type_value(ty)`, the interning that has existed unused since the pool was written.

**The thunk asks `FileSignatures` rather than `ConstValues`.** `SigEntry::type_value` already holds the
resolved type of every nominal declaration, computed in the signature phase — which const-eval is
*downstream* of (ADR-0018 §3), so this reads a value that already exists rather than inverting a phase.
That is the same move ADR-0070 §1 made for an array length, and it is available for the same reason.

**Rejected: making a struct an ordinary const-eval target.** It would give `Point` a `ConstValues` entry
and let the existing name path find it. Rejected because a struct's value genuinely is a *declaration*
— `wanted`'s docs argue this, and `Callee::Direct` names a procedure without one — so it would mean
evaluating something with nothing to evaluate, and the thunk would still need to know it had produced a
type rather than a number.

### 3. A type used where a runtime value is expected is **E0261**

```jr
main :: () {
    t := Point;        // E0261: a type is a compile-time value
    u: Type;           // E0261, likewise
}
```

The refusal is in `jr-sema`, at the point a name's type comes back as `PoolId::TYPE` in a *body*. The
message says a type is a compile-time value and names where one may appear — a constant, a type
annotation — because "cannot be stored" without saying where it *can* go is a diagnostic a reader cannot
act on.

**Refused in sema rather than in lowering**, for the reason ADR-0039 §3a gave for array lengths and
ADR-0017 §4 gives generally: rejecting a construct is a semantic judgement, and a lowering refusal makes
a well-formed-looking program report a compiler-internal message — which is exactly what §0's alias case
was doing. `tests/corpus/type-errors/` files must lower cleanly, so the diagnostic has to be sema's.

**This is the wave's actual correctness content.** §1 and §2 add a capability; §3 removes a silent
miscompile, and it is the reason this sub-wave is worth shipping separately rather than folded into a
larger RTTI change.

### 4. `type_info()` and `Any` are a later sub-wave, for a layout reason

§2.1 groups all three, and they divide cleanly on one question: *does the value exist at run time?*

- A `Type` **does not** (§0: `ComptimeOnly`), which is why §1–§3 can ship now — every type value is
  consumed by the compiler and none reaches a back end.
- `type_info()` returns a *struct describing* a type, which does exist at run time — so it needs that
  struct declared in `modules/Basic`, populated by the compiler, and a layout. That is a representation
  decision of its own.
- `Any` is a `{type, pointer}` pair, so it needs the same runtime type representation *plus* a rule for
  what may be put in one and how it is read back out.

So `Any` is not "more RTTI", it is the first construct that makes a type into runtime data — and it is
what §5's "sema and the VM become mutually recursive" is really about. Splitting here keeps this
sub-wave's claim checkable: after it, a type is a compile-time value and nothing else.

### 5. What is deliberately absent

- **`type_info()` and `Any`** (§4), and any runtime representation of a type.
- **A type as a procedure parameter or result.** That is the polymorphism W5 owns (`$T`), and giving a
  procedure a `Type` parameter now would be a second route to it.
- **Comparing two types** (`T == U`). It is decidable and cheap — a `PoolId` comparison — but its
  *meaning* is ADR-0015's identity question, and answering it in passing would settle a design question
  this ADR has no argument for.
- **A type alias chain** (`A :: Point; B :: A;`), for the reason ADR-0070 §4 refused a length chain: one
  level is a lookup, a chain needs a fixpoint and a cycle check.

## Consequences

- **A silent miscompile becomes a diagnostic**, which is the wave's point. The corpus gains the refusal,
  so the placeholder cannot come back unnoticed.
- **One new diagnostic code, E0261**, for a type used at run time. **E0262 becomes the first free code.**
- **`Type` becomes a reserved type name** in the sense `bool` and `string` are: a program using `Type` as
  a *type* name now resolves to the builtin. It was previously E0212, so nothing that compiled changes.
- **`file_consts` gains a target kind**, its third after ADR-0069 §2's body `#run`. The round-robin and
  the cycle detector are unchanged, because a type-valued constant is a target like any other.
- **`Item::TypeValue` gets its first producer** since the pool was written, which is worth noting: the
  representation was designed for this and has been dead code for twenty-odd waves.
- **No MIR change and no back-end change.** A type value never reaches either — that is what §4's split
  buys, and it is why this sub-wave adds no engine risk.
