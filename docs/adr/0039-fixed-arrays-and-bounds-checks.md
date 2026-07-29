# ADR-0039: `[N]T` fixed arrays, and the `bounds_check` op ADR-0003 asked for

- **Status:** Accepted
- **Date:** 2026-07-28
- **Deciders:** dboll
- **Amends:** ADR-0003's "into the slice" follow-on item, which was never done. The
  decision it records is honoured here unchanged; only its schedule was wrong.

## Context

`PLAN.md` §7 put arrays first inside W1 for a reason visible in the code:
`modules/Basic`'s `print_digits` recurses one stack frame per digit, and a comment says
it does so because there is no `[20]u8` to format into.

Two things were checked before this ADR was written, and one of them contradicts the
plan.

**There is no `bounds_check` operation in MIR, and there never was.** ADR-0003 decided
bounds checking is a build setting carried as an *explicit* MIR operation, and listed
under "follow-on work this forces":

> **Into the slice:** MIR's design must include a `bounds_check` operation and a
> build-config stripping pass, even though Jairs-0 has no arrays to index — the
> representation cannot be retrofitted cheaply.

It was not built. `grep` for `bounds_check`, `BoundsCheck` or `no_abc` across
`crates/` finds two unrelated hits: the lexer's reserved-directive list, and a `jr-vm`
test about host pointers. There is no `Projection::Index` either.

Meanwhile §7 said arrays "bring ADR-0003's `bounds_check` MIR ops into play, **which
nothing has exercised**" — which reads as though the ops exist and lack tests. They do
not exist. That is this project's second named failure mode: a plan describing work as
done-but-untested when it was never started. Recorded here rather than quietly fixed,
because the next reader of §7 would have made the same assumption.

**The retrofit is cheap, and ADR-0003's stated reason for the deadline was wrong.**
`Statement` has four variants and `Projection` four; adding one to each is a compile
error at every exhaustive match, which is exactly what the house style's ban on `_`
arms buys. What ADR-0003 could not have known is that MIR would stay small enough for
this to be a mechanical change. The decision it made is still right; the urgency was
not.

## Decision

### 1. The bounds check is an explicit `Statement`, exactly as ADR-0003 said

```rust
Statement::BoundsCheck {
    index: Operand,
    len:   Operand,
    span:  MirSpan,
}
```

Lowering `a[i]` emits the check as a *statement preceding* the access, then a `Place`
with `Projection::Index`. The check is a separate operation that a build-config pass
can strip as a unit and const-prop can delete when it proves the index in range —
both of which ADR-0003 named as the point.

`len` is an `Operand` rather than a `u64` baked into the statement. For a `[N]T` the
length is a constant and always will be, so a field would be smaller. It is an operand
anyway because `[]T` views and `[..]T` dynamic arrays arrive in later waves with a
length that is *loaded from the value*, and a check whose length is a constant field
would have to be replaced wholesale rather than extended. One shape now, for the cost
of one interned constant per check.

**Rejected: fold the check into `Projection::Index`.** Smaller — no new statement, and
no exhaustive match to update. This is the option ADR-0003 rejected by name, and its
argument still holds: a check that exists only as an implicit consequence of an index
projection cannot be stripped as a unit, cannot be deleted individually by
const-prop, and makes the build setting a special case inside each back end rather
than one pass. Re-deciding it the other way would need a new ADR *and* a reason, and
nothing found while surveying supplies one.

**Rejected: emit no check this wave.** The smallest wave, and the worst: an
out-of-range index would read or write arbitrary memory with no diagnostic, and the VM
(a bounds-checked linear region) and native code (a raw address) would disagree about
what the program does. That is the silent-miscompile shape `AGENTS.md` names, with the
two engines' disagreement as the symptom.

### 2. A failed check traps, with the same machinery ADR-0002's overflow uses

A new `TrapKind::IndexOutOfBounds` and a matching `jr-vm` `Trap`, whose message is
the one both engines print. ADR-0020 §2's single formatter in `jr-base` means the
wording cannot drift, and `differential.rs` compares a failing program's output — so
an index trap is checked in both engines the day a corpus file writes one.

**The message is static — "index out of bounds" — and does not name the index.** That is
a concession, and it is forced rather than chosen: `TrapKind::reason()` returns
`&'static str`, and native code raises a trap by calling a helper with a pointer to a
constant string, so there is no formatting step to interpolate a runtime value into.
Making one exist means the trap helper takes the index and the length as arguments and
formats at run time in generated code, which is a larger change than this wave should
make on the trap path both engines share.

The consequence, stated so it is not discovered: a program that traps on an index tells
you the line but not the value. The line is what ADR-0020 already delivers, and for a
single access per line that is enough to find it. If it turns out not to be, the fix is
a formatting trap helper and it applies to every trap kind at once — which is a better
change than a special case for this one.

### 3. `[N]T` is a structural pool type; `N` is part of its identity

`Item::ArrayType { elem: PoolId, len: u64 }`, interned structurally like
`PointerType` (ADR-0015 §4). `[4]s64` and `[5]s64` are different types, and `[4]s64`
from two files is one type. This follows ADR-0015's existing split: nominal for
`struct`, structural for everything built out of another type.

Layout is `elem.size * len`, aligned to `elem.align`, computed in `jr-pool`'s
`layout_of` — the one place layout may be computed (ADR-0018 §2). There is **no
padding between elements**: the element stride is `elem.size` rounded up to
`elem.align`, which for every type Jairs has today equals `elem.size`.

**Rejected: `N` outside the type identity.** Would make `[4]s64` and `[5]s64` the same
type and push the length into sema, which is how a language ends up unable to say what
a value's type is.

#### 3a. `N` must be an integer *literal* in this wave

`[20]u8` works; `[COUNT]u8` does not, and is refused with E0232 naming the reason.

This is not a preference, it is where const-eval lives. ADR-0018 §3 puts constant
evaluation in `jr-db`, running MIR through the bytecode VM — *downstream* of `jr-sema`,
which is where a type annotation is resolved. `jr-sema` cannot ask for `COUNT`'s value
without inverting that dependency, and `jr-sema`'s `Cargo.toml` deliberately does not
depend on `jr-vm`.

So the length is parsed as an expression (§3's node shape is unchanged, and stays
right), and only a literal is accepted. `[COUNT]u8` becomes possible in the wave
that makes sema and comptime mutually recursive, which `PLAN.md` §2.1 schedules as W4
and §5 lists as the project's top risk. Refusing it with a message that says so is
better than a `[COUNT]u8` that silently resolves to a wrong length.

**`jr-hir` reads the literal; `jr-sema` reports a bad one.** The split looks redundant
and is not. Lowering is the only phase holding the literal *token*, so it is where the
value can be read at all — but the first draft also raised the diagnostic there, and that
broke a contract stated in a test: `tests/corpus/type-errors/` requires every file in it
to lex, parse, lower and resolve **cleanly** and be rejected by sema alone, so that a file
in that directory cannot accidentally be testing the parser. A lowering error made
`[COUNT]u8` untestable in the one directory where every other rejected type lives.

`TypeRef::Array` therefore carries `len: Option<u64>` *and* `len_span`, and sema raises
E0233 when the length is `None`. Rejecting a type is a semantic judgement anyway, so this
is where it belonged; the test is what made that visible rather than arguable.

A length that is negative, or larger than `u64`, is the same refusal — the literal is a
signed `i128` since ADR-0038, so both are visible where the check happens.

The code is **E0233**, not E0232. `AGENTS.md` says "E0232 is the first free code" and it
was not: `jr-sema` took E0232 for a non-integer `cast` in ADR-0037's wave, and the note
was not updated. Caught here by reading `jr-sema`'s `code.rs` rather than trusting the
note — which is the same "do not believe a handoff, open the file" rule the previous
wave added, applied to a different file.

### 4. Zero-initialised by default, `---` still means uninitialised

`buf: [20]u8;` is zeroed. This differs from a scalar, and the difference is
deliberate: a scalar declaration without an initialiser lowers to `Rvalue::Undef`,
whose read is E0227 statically and a trap dynamically. Applying that to an array
would make `buf: [20]u8;` followed by `buf[0] = 65;` an uninitialised *read* of the
whole array on the first partial write, because MIR tracks definedness per slot and
not per element.

So an array declaration without an initialiser zeroes the slot. `buf: [20]u8 = ---;`
opts out and gets `Undef`, matching what `---` means everywhere else.

**Rejected: per-element definedness tracking.** Correct, and what E0227 would want.
Rejected as a whole wave's work in the SSA construction for a diagnostic nobody has
asked for, on a construct whose usual first act is a partial write.

**Rejected: leave an array uninitialised like a scalar.** It makes the common case —
declare a buffer, fill part of it — trap or fail E0227, which would have made
`print_digits`'s buffer unusable and defeated the wave's purpose.

#### 4a. Zeroing needs a MIR statement, because "codegen's job" was a miscompile

Deciding §4 turned up a live bug older than this wave. `build.rs` emitted **nothing** for
a default-initialised aggregate, above this comment:

> A non-promotable local needs no zero store to avoid a false report … Emitting the
> zeroing that a struct or ADR-0004's `{data, count}` actually requires is codegen's job,
> because it needs the layout this crate deliberately does not have (ADR-0017 §5).

Neither back end does it. Measured on the commit before this wave, with
`Point :: struct { x: s64; y: s64; }` and `p: Point; exit(p.x + p.y);`:

| Engine | Exit status |
|---|---|
| `jr run` | **0** |
| `jr build` then run | **184**, then **200** on a rebuild |

The VM zeroes a freshly allocated frame, which `jr-vm`'s own docs call "not
load-bearing — `jr-mir` emits an explicit store". It does not. Cranelift's
`ExplicitSlot` is raw stack, so the native binary read whatever the last call left
there — a different answer per build, which is why the status changed between runs.

The two engines disagreed about a legal program and nothing caught it, because
`differential.rs` compares *observable* output and no corpus program observed a
default-initialised aggregate. This is exactly the failure mode `AGENTS.md` names: a
construct the grammar allows, no representation on the lowering path, and a
legitimate-looking value — an absent store — standing in for the missing one.

So `Statement::Zero { place, span }` is added, and `build.rs` emits it for a
default-initialised aggregate. It carries no size, keeping ADR-0017 §5 intact: both back
ends already know the slot's type, so each computes the byte count from the layout it
already asks `jr-pool` for.

Arrays are the reason this had to be fixed rather than noted — §4 makes zeroing the
*defined* behaviour of `buf: [20]u8;`, so leaving it to a back end that does not do it
would have shipped the same bug in a construct built to rely on it.

### 5. `a[i]` is a place, and `.count` is a comptime constant

Indexing yields a location, so `buf[0] = 65` and `p := *buf[0]` both work, on the
same `is_place` rule field access already follows.

`buf.count` is `N` as an untyped integer constant folded during checking. It is *not*
a `Projection`, and nothing is loaded: the length is in the type. This is one line in
sema and it is what makes an array usable in a `while` loop without writing the bound
twice — which, with no `for` until W2, is every loop over an array.

**`.data` is deliberately absent.** On a `string` it exists because ADR-0004 fixes
that layout. Giving an array a `.data` would hand out a `*T` into a stack slot with
no way to bound it, one wave after adding the check — and pointer arithmetic is in no
wave's list, so the pointer could not be used for anything but escaping the check.

### 6. No array literals this wave

`buf[0] = 65;` is how an array gets its contents. `[1, 2, 3]` needs decisions about
inferred versus declared length, whether elements must be comptime-constant, and how
ADR-0016 §1's context typing reaches an element — a separate ADR's worth, and nothing
in this wave needs one.

### 7. `#no_abc` stays reserved

ADR-0003 pairs the build setting with a local `#no_abc` opt-out. The lexer already
reserves the directive. Neither the build setting nor the local opt-out is *wired*
here: there is no `--no-bounds-check` flag and no way to strip the checks, because
there is no `opt_level` or build-configuration surface at all yet (`PLAN.md` §1.5
lists optimisation levels as not started).

Stated plainly so it does not become another ADR-0003: **the op exists and the pass
that strips it does not.** Adding the pass without a flag to drive it would be
untestable code, and the wave that adds build configuration is the wave that should
add it.

## Consequences

- **`print_digits` can stop recursing** — but does not, in this wave. The buffer
  needs a loop that writes digits backwards and then a second pass to print them,
  which is a `modules/Basic` change with its own differential evidence to produce. The
  comment there stays accurate, and the array it names now exists.
- **Every exhaustive match over `Statement` and `Projection` must handle the new
  variants** — `verify`, `dce`, `constprop`, `forward`, `inline`, `escape`, `ssa`,
  `dump`, both back ends. That is the house style working: the compiler lists the
  sites.
- **DCE may not delete a `BoundsCheck`.** It is a statement with no result whose
  effect is a possible trap, so it is exactly as undeletable as a trapping `Add`
  (ADR-0022's `can_trap`). Getting this wrong would delete the check and leave the
  access.
- **A `[N]T` is an aggregate**, so it takes the aggregate path everywhere `struct`
  already does: it never promotes to a register, it always gets a slot, and
  `jr-codegen-clif` refuses to return one by value exactly as it refuses a struct.
- **The trap count grows by one**, and `differential.rs` gains a program that traps
  on an out-of-range index, which is how the two engines' wording is held equal.
