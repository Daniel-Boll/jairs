# ADR-0189: `print` with `%` placeholders, and four compiler defects behind it

- **Status:** Accepted
- **Date:** 2026-09-03
- **Deciders:** dboll

## Context

The library could print a `string` and one non-negative integer. `print_int` could not render
`S64_MIN` — its own docs said so — and there was no way at all to show a `float`, a `bool`, a struct
or a negative number without hand-assembling digits. Every other capability this project has built
sits behind that: a program cannot report what it computed.

The machinery was all present and unused. ADR-0138 and ADR-0139 built the `..Any` variadic and its
call-site packing; ADR-0075 built `Type_Info`; ADR-0076 built `Any` and the `*T`→`Any` erasure;
ADR-0186 gave the language a file-scope mutable variable, which is what a buffer needs. This wave is
the first caller to compose all four, and composing them found four compiler defects — three of them
in code that had been shipped and believed for several waves.

## Decision

### 1. `print` is `(fmt: string, args: ..Any) -> s64`, with Go's `%` and Go's diagnostics

One placeholder character, `%`, taking the next argument whatever its type — matching Jai, whose
`print` is `%`-based and untyped at the placeholder. `%%` is a literal percent.

A wrong argument count is **not** an error. Too few renders `%!(MISSING)`; too many appends
`%!(EXTRA a, b)`. Both are Go's, and both are chosen over a diagnostic for one reason: `print` is a
procedure with nowhere to return an error to, and a `print` that refuses to print is worse than one
that tells you in the output. A caller who miscounts sees it immediately in what was written.

The return value is the byte count, so a caller can accumulate. `valid/140` uses exactly that as its
exit-code checksum, which is what makes a wrong rendering fail a run rather than merely look wrong.

Output goes through a file-scope buffer and reaches `write` once per call. The old `print_int` cost
one syscall **per digit**. The buffer is stated to be **not thread-safe** rather than quietly so: two
threads printing interleave. Jai's own `print` uses per-thread temporary storage, which needs
`#add_context`; ADR-0186 already records that as owed.

### 2. An implicitly coerced argument describes **itself**, not its pointee

This **amends ADR-0076 §1**. That section made `f(*p)`, where `f` wants an `Any`, produce an `Any`
describing `P` — erasing *through* the pointer, identically to `any_of(*p)`. The two spellings were
one operation.

They are now two. `f(*p)` describes the `*Point`; `any_of(*p)` still describes the `Point`.

The reason is that the old rule makes a pointer unprintable. `print("%", p)` on a `*Point` must be
able to say "this is a pointer", and under ADR-0076 §1 it could not — there was no `Any` in the
language whose type was a pointer type. Jai's rule is that an argument describes its own type, and
the escape hatch for the other meaning is to write `any_of` explicitly, which is exactly the
asymmetry a reader can act on: the implicit form is the boring one, the explicit form says something.

ADR-0076 §4 deferred a **bare value** coercion — `print("%", 42)` — because a literal has no address.
That is delivered here: the value is stored into a fresh slot and the slot's address becomes
`Any.data`. The slot is per-coercion and not shared across a call's arguments, which matters
concretely: `print("% %", a, b)` builds two `Any`s that must point at *different* storage, and one
shared slot would make both describe whichever was stored last.

**The migrated test is the interesting artefact.** `a_pointer_coerces_to_any_at_a_call_in_both_engines`
asserted `size == 16` from an implicitly coerced `*Point`. That was not a stale *number* but a stale
*semantics*, and it now asserts the **difference** — 8 for the implicit form, 16 for the explicit one.
Asserting the difference rather than restoring an equality is deliberate: a test that only checked the
two engines agreed would keep passing if both lost the distinction, which is the regression that
matters, because `any_of` is the only thing keeping "describe the pointee" reachable.

### 3. Three `imports.is_empty()` guards made `modules/Basic` unable to use its own library types

`library_struct`, `library_enum` and `any_struct_quiet` each began with
`if self.imports.is_empty() { return None; }`, above the lookup. The comment explained it well: a
checker run without module resolution cannot find `Type_Info`, which lives in `Basic`, and reporting
E0265 there would be inventing a library error out of a missing input. `jr-sema`'s own corpus test
runs exactly that way on purpose.

The guard is right about *reporting* and wrong about *looking*. `modules/Basic` imports nothing and
**declares `Type_Info`, `Type_Info_Kind` and `Any` itself**, so an empty import list is also the
signature of the declaring file — and the lookup three lines below already falls back to `self.sigs`,
which is where a declaring file's own types live. The guard was doing nothing but hiding them.

The cost was invisible until this wave, because nothing in `Basic` had ever *used* reflection —
seventeen occurrences of `type_info(` in that file, and all seventeen were doc comments. The first
code use produced `warning[E0245]: the compiler could not lower the body of format_field`, blaming
the body. `print("%", n)` inside `Basic` was `variadic argument expected Any, found s64` while the
identical call in an importing file worked.

The fix is to do the lookup first and apply the silence rule **only when it misses**. A file with no
imports that does not declare the type misses both lookups and still says nothing, so the property
the guard existed for is unchanged.

**The general shape, which this project has now met four times:** a cheap proxy standing in for the
condition actually meant. Here `imports.is_empty()` proxied for "module resolution did not run", and
it is also true of the one file that needs no resolution. ADR-0178 §2's `TrapKind::ALL` length
assertion proxied for exhaustiveness; ADR-0176 §6's `file_consts` early-out proxies for "this file
uses a comptime feature" via a hand-maintained list. **A proxy is not wrong until something legitimate
sits on the other side of it**, which is why these survive review and surface as a defect in a
program nobody suspected.

### 4. `print_int` delegates, and both old helpers are deleted

`print_int` is now `print("%", n)`. It renders `S64_MIN` because the formatter goes through an
unsigned magnitude and has no negation to overflow — which is precisely the fix its own docs had named
("an unsigned path or `-%`").

`print_digits` and `put_byte` are **deleted**, not left unused. Keeping them would leave a second
route to decimal digits beside the real one, and a second route is a second chance to disagree — which
these two demonstrably did, on the one value most likely to be tested first. ADR-0009's
one-implementation rule, applied to a library rather than to layout.

`print_int` is kept as a *name* because programs and the corpus call it, and because `print("%", n)`
needs a format string where this needs nothing.

### 5. What the formatter can and cannot reach, and the one root cause

Delivered: every integer width signed and unsigned including `S64_MIN` and `U64_MAX`, `float32` and
`float64` to shortest-ish decimal, `bool`, `string`, pointers as hex, a struct/union/variant one level
deep by field name, and a fixed-size array's elements.

Not delivered, all four for **one** reason: an enum prints its ordinal rather than its member name, a
nested aggregate field prints `…`, a view prints `<view>`, and a structural type's `name` is the
lowercased kind (`array`, not `[3]s64`).

The root cause is that **`Type_Info` carries type *ids*, not `*Type_Info`** — `Type_Info_Field.ty` and
`Type_Info.element` are both `s64` ids, and ADR-0077 §1 makes an id deliberately opaque. An id answers
"are these the same type?" and nothing else, so nothing can recurse into an element type or look up a
member table. `format_field` works around it with a ladder of thirteen comparisons against
`type_info(T).id` for each builtin, which the compiler folds to constants — that covers scalars and
stops there.

A fixed array escapes only by arithmetic: its stride is `size / count`, which is exact because a fixed
array's size *is* `count` strides. A **view** cannot use that trick — its `size` is the 16-byte header
— which is why a view is unreachable for a different reason than a procedure is, and the code says so.

Lifting all four is one change: emit a `*Type_Info` per type, using the same compiler-emitted
static-data table ADR-0152 §3 built for `fields`. That is a wave, and it is recorded rather than
half-built.

### 6. A pointer's value has no home in `tests/corpus/valid/`

`valid/140` prints a pointer nowhere, deliberately. The bytecode VM's addresses are region-relative
and a native binary's are real, so the two engines legitimately disagree — and `valid/`'s whole
premise is that they agree. Verified by hand in both instead (`0x59` under `jr run`, `0x1042df804`
native). This is the same call ADR-0126 made for the foreign-call pointer span.

### 7. The inliner creates cross-file global references, and ADR-0186 §3 assumed it could not

`Compiler::global_data` compared a `GlobalRef`'s file against the body's own and refused a mismatch as
an **internal** error, resting on ADR-0186 §1's same-file contract. Three engines had recorded that
contract; the VM's version was the one with a check.

The contract is false, and not because a program can write a cross-file reference. **The inliner
creates them.** `Basic.print` reads the output buffer, and inlining that body into a caller in another
file copies the `GlobalRef` unchanged — which ADR-0186 §3 chose *deliberately*, because a `GlobalRef`
is absolute. So a host body legitimately contains a global whose file is not its own, and the moment
the standard library used a global, an ordinary `print` call reported
`internal compiler error: a cross-file global reference, which this engine does not yet support`: a
message about a feature nobody had asked for.

The fix is a phase split matching `build_object`'s. `add_file_globals` records every global before any
body compiles, and the compiler resolves a `GlobalRef` against the **program's** table rather than one
file's. Four drivers call it, including the two comptime ones.

**A flag was tried and removed**, and the reason is worth keeping. Marking a comptime program
"globals unobservable" so a comptime read got ADR-0186 §2's honest message instead of a lookup miss
moved the refusal from **execution** to **assembly** — and assembling `modules/Basic` then failed
outright, so every constant in the file reported "a global variable's current value cannot be read
here". A comptime program must still *type* globals so that bodies holding one compile; whether a
`#run` may read one is enforced upstream, where it already was.

### 8. `print("%", f())` was refused

The value coercion is recorded against the argument *expression*, and the coercion check excluded
`Expr::Call` on the ground that a call handles its own `any_op`. True for `any_of`/`any_as`, which
*are* calls; false for the implicit coercion, which has no call node of its own and is merely recorded
against whatever expression the argument is. So an argument that happened to be a call fell through to
the intrinsic path and refused the body with
`a value coercion to `Any` recorded against a call`.

`print("%", f())` is the shape every caller writes. Found by writing it.

## Consequences

- `print`, `print_line` and `print_int` are one renderer. A program can report a `float`, a negative
  number, a `bool` and a struct for the first time.
- `S64_MIN` prints. The library's oldest documented gap is closed.
- Three shipped `imports.is_empty()` guards no longer hide a declaring file's own types, so
  `modules/Basic` can use `type_info` and `Any`.
- A global reaches a body in another file through the inliner, so the standard library may hold state.
- Owed, in one change: a `*Type_Info` per type, which lifts enum member names, nested aggregate
  fields, view elements and structural type names together.
- Owed, unchanged from ADR-0186: `#add_context`, without which `print`'s buffer is process-wide rather
  than per-thread.

## Alternatives considered

**Typed placeholders (`%d`, `%s`).** Rejected: every argument already carries its `Type_Info`, so a
type letter is information the callee has and the caller can get wrong. Jai does not have them.

**A diagnostic for a wrong argument count.** Rejected above — `print` has nowhere to return an error,
and both wrong counts are visible in the output where a caller is looking.

**Keeping `print_digits` beside the formatter.** Rejected: two routes to decimal digits, one of which
was already wrong about `S64_MIN`.

**Faking enum member names from the type name.** Rejected. There is no member table, and inventing
`Colour.1` or guessing from `name` would print something a reader would trust.

**Ordering the comptime globals question by flag** (§7). Tried, measured, removed.
