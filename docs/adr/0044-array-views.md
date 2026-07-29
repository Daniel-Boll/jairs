# ADR-0044: `[]T` is a `{data, count}` pair, and an array converts to one only where it is explicitly asked for

- **Status:** Accepted
- **Date:** 2026-07-28
- **Deciders:** dboll
- **Depends on:** ADR-0039, whose `Statement::BoundsCheck` takes its length as an *operand*
  precisely so that a view's runtime length needs no new statement. This is that groundwork
  being spent.

## Context

`PLAN.md` §2.1 lists `[]T` in W1, and `jr-syntax`'s parser refuses it by name with E0124
("array views `[]T` arrive in a later wave"). A view is the reason `[N]T` was built to be
sliced from: a procedure that takes `[4]s64` works for exactly one length, so
`modules/Basic`'s eventual `print_digits` buffer would need a separate procedure per size.

Five facts were established by running things before this ADR was written, and two of them
changed a decision below.

- **An aggregate passes by value in both engines, and the native back end refuses to
  *return* one.** Measured: a `Point` parameter gives exit 7 under `jr run` and 7 from the
  native binary, while `-> Point` fails the build with "returning an aggregate, which needs a
  caller-allocated result slot is not supported by this back end yet". So a view may be a
  parameter and a local this wave, and a procedure returning one would hit a pre-existing
  hole rather than a new one.
- **`Statement::BoundsCheck`'s `len` is already an `Operand`.** ADR-0039 §1 chose that shape
  for this wave by name. A view's check is therefore a `Load` of its `.count` into an operand
  and the *same statement* — no new variant, no second checking path that could disagree with
  the array one.
- **`TypeRef::Array` already uses `len: Option<u64>`, and `None` means "not a usable
  literal"** — the E0233 refusal from ADR-0039 §3a. A view cannot be spelled as
  `TypeRef::Array { len: None }`, because that value already means an error. This is the
  first fact that forced a decision: a view needs its own `TypeRef` variant, not a reused
  field.
- **`string` is already a `{data: *u8, count: s64}` pair whose layout is executable**, in
  `jr-pool`'s `string_data`/`string_count`, and MIR reaches its two halves through
  `Projection::StringData`/`StringCount`. A view is the same shape generalised over the
  element type, and the *second* fact that forced a decision: the machinery to lower a
  two-word aggregate with a pointer and a count already exists and is exercised by every
  `print` call in the corpus.
- **`Shape`/`Repr` classify by `Item` variant exhaustively** in the VM and Cranelift both, so
  a new type variant is a compile error at every site that must decide whether it is a
  register or an aggregate. The house style listing the work, again.

## Decision

### 1. `[]T` is a two-word `{data: *T, count: s64}` aggregate

`Item::ViewType { elem: PoolId }`, interned **structurally** like `PointerType` and
`ArrayType` (ADR-0015 §4): `[]s64` from two files is one type, and `[]s64` and `[]u8` are
different ones. Layout is the pointer, then the count at the next 8-aligned offset — the
*same* computation `string_layout` performs, factored so that one function answers for both.

```jr
sum :: (xs: []s64) -> s64 {
    i := 0;
    t := 0;
    while i < xs.count {
        t = t + xs[i];
        i = i + 1;
    }
    return t;
}
```

**Rejected: a fat pointer that is a distinct MIR/codegen concept** rather than an aggregate —
a pair of registers, the way Rust lowers `&[T]`. Faster: no stack slot, and both halves stay
in registers across a call. Rejected because it needs a *new* value shape in two back ends
and in `Repr`/`Shape`, and because Jairs has no multi-register value today: an aggregate
parameter is a byte copy at both engines. Adding a second calling convention for one type is
a larger change than the whole feature, and the aggregate path is already exercised by
`string`.

**Rejected: `string` becomes `[]u8`.** Tempting — the layouts are identical, and it would
delete `Projection::StringData`/`StringCount`. Rejected because ADR-0015 §2 makes `string` a
*distinct* type on purpose: `string` is UTF-8 by convention and `[]u8` is bytes, and merging
them would make every `[]u8` printable and every string indexable as a number, silently. The
two share a layout and not an identity, exactly as ADR-0004 already says.

**Rejected: `count` as a `u64`.** Defensible — a length is never negative. Rejected for
consistency with `.count` on `[N]T` and on `string`, both of which are `s64` (ADR-0004), and
because a `u64` count would make `i < xs.count` a mixed-signedness comparison, which
ADR-0015's no-coercion rule refuses. The bounds check compares unsigned anyway (ADR-0039 §1),
so a negative count fails it.

### 2. There is **no implicit conversion**. A view is made with `[]`, an explicit slice operator

This is the fork that mattered, and it is decided against implicitness.

```jr
buf: [4]s64;
xs := buf[];        // a `[]s64` over all of `buf`
n  := sum(buf[]);   // explicit at the call site
n  := sum(buf);     // *refused* — E0240
```

`buf[]` is postfix `[]`, at the same precedence as `[i]`, `.field` and a call. It yields
`{data: *buf[0], count: N}`.

**Rejected: implicit array→view coercion at every expectation site**, so `sum(buf)` works.
This is what Jai does, and what Go does for `[N]T` → `[]T` via `arr[:]`… actually only with
the explicit slice, which is the point. Rejected for three reasons, in order of weight:

1. **It would be the language's first implicit conversion, and Jairs has spent five ADRs
   refusing them.** ADR-0015 refuses struct-to-struct, ADR-0016 §1 makes even an integer
   literal's type contextual rather than converted, ADR-0037 §2 requires `cast` for
   `s32`→`s64` — a *widening*, which is lossless — and ADR-0040 §6 refuses `int`→`float`.
   Adding an implicit conversion for arrays while `cast(s64, x)` is mandatory for a widening
   integer would make the rule "no implicit conversions, except one" — and the exception
   would be the hardest one to see, because it changes a value's *size* from `N*8` bytes to 16
   and takes an address.
2. **The address-taking is invisible.** `sum(buf)` under coercion silently takes `*buf[0]`
   and passes a pointer into the caller's frame. Today `sum(buf)` copies. Two readings of one
   line differing in whether the callee can *write through* to the caller's array is exactly
   the kind of implicitness ADR-0011 rejected when it made `*` and `.*` separate operators.
3. **`escape.rs` would need to know, and would have no way to find out.** A local whose
   address is taken is not promotable (ADR-0017 §2), and an implicit coercion takes an address
   at a site containing no `AddrOf` in the HIR. `escape.rs` walks for `UnOp::AddrOf`, so it
   would miss it.

   Stated precisely, because the strong form of this claim is **not** true today: an array is
   never promotable in the first place — `is_register_representable` answers `false` for
   `Item::ArrayType`, so `buf` gets a slot whether or not anything takes its address. So
   treating `Expr::Slice` as an escape is *defence in depth* rather than a live bugfix, and a
   test pins it as such rather than pretending otherwise. What makes the argument bite is the
   direction of the risk: the escape walk is a *syntactic* over-approximation, and an implicit
   coercion is invisible to it by construction, so the safety would depend on an unrelated
   classification decision staying the way it is. An explicit operator makes the address-taking
   visible to the walk the same way `*buf` is, and stays correct if arrays ever become
   register-representable.

The cost, stated plainly: every call site writes three more characters, and a reader who
knows Jai will try `sum(buf)` first. E0240 is therefore a *specific* diagnostic that names
`buf[]` in its help, rather than a generic mismatch — the ADR-0043 lesson that an accurate
diagnostic can still be useless.

**Rejected: `[..]` or `slice(buf)` as the spelling.** `[..]` is Go's and Rust's `[:]`/`[..]`
and it reads well, but `[..]T` is *already* reserved by the parser for dynamic arrays
(E0124), so `buf[..]` and `[..]s64` would be the same two tokens meaning a slice in one
position and a resizable array in the other. `buf[]` has no such collision, and it matches
`[]T` — the type and the operator that produces one are spelled with the same brackets.

### 3. Sub-slicing is **not** in this wave

`buf[1..3]` does not parse. Only `buf[]`, the whole-array view, does.

A range needs a range *expression* — a new node, a new precedence question (is `a..b` an
expression or only a slice index?), decisions about open ends (`buf[2..]`), and its own
bounds-check shape (two comparisons, and `lo <= hi`). None of that is needed by what this
wave exists to unblock, which is a procedure that works for any length. Recorded as owed
rather than smuggled in: **`buf[lo..hi]` is a separate ADR**, and until it exists a program
that wants a sub-range passes the view and an index.

### 4. `xs[i]` and `xs.count` — the `[N]T` operations, with the length loaded rather than folded

`xs.count` is a **load** of the second word, where `buf.count` is a constant folded from the
type (ADR-0039 §5). That difference is the whole point of a view, and it is the reason
`.count` needs a new `Projection`: `Projection::ViewCount`, beside `StringCount`.

`xs[i]` emits the same `Statement::BoundsCheck` an array index does, with `len` being an
operand that loaded `xs.count` instead of a constant. **One checking path**, which is what
ADR-0039 §1's operand-shaped `len` bought.

`xs.data` is **absent**, exactly as `buf.data` is (ADR-0039 §5): it would hand out an
unbounded `*T`, and there is no pointer arithmetic to use it with. `string.data` exists only
because ADR-0004 fixed that layout for the FFI boundary, and a `[]T` crosses no such
boundary yet.

Assignment through a view — `xs[0] = 5` — **works**, and writes through to whatever the view
was made from. A view is a *pointer* to storage, not a copy of it, and that is what makes it
worth passing. There is no `const`-ness in Jairs to express otherwise, and inventing one here
would be a language-surface decision wider than this wave.

### 5. A view is an ordinary value: assignable, comparable to nothing, and never returned

- **Assignable** between views of the same element type: `ys := xs;` copies two words.
- **`==` is refused.** Two views could mean "same storage" or "same contents", and picking
  one silently is worse than refusing. E0241, with a note saying so.
- **Returning one is refused for now, by a pre-existing hole rather than a new rule.** The
  native back end cannot return an aggregate at all (measured above, and §7's list has
  carried it for waves). `-> []s64` therefore fails at codegen with the *existing* message,
  not a sema refusal — a view is not special here, and adding a sema check that says "this
  one aggregate may not be returned" would be a rule that outlives its cause.

  Stated so it is not mistaken for a design decision: a procedure returning a view **should**
  work, and will, in the wave that gives Cranelift a caller-allocated result slot.

### 6. A view of a view, and a pointer to a view, both work by construction

`[][]s64` interns (structurally, nesting like `PointerType`), and `*[]s64` is an ordinary
pointer to a two-word aggregate. Neither needs a rule: they fall out of §1's structural
interning and the existing pointer machinery. Named here only because "does it nest" is the
first question a reader asks about a new type constructor, and the answer is yes for the same
reason `**T` and `[2][3]u8` already work.

`[]` on something that is not an array is refused — including on a view, so `xs[]` is an
error rather than an identity. A no-op operator that silently does nothing is how a reader
concludes it did something.

## Consequences

- **`Item::ViewType` is the third structural type constructor**, so every exhaustive match
  over `Item` gains an arm: `is_type`, `type_of`, `layout_of`, `describe`, `Shape`, `Repr`,
  `escape.rs`'s register test, MIR's `dump`, the LSP's `render` and `completion`, and the
  places `jr-pool`'s `foreign_library_name` matches for totality. Twenty-odd sites, each one a
  compile error until it is handled — the house style's ban on `_` arms doing the work of a
  checklist.
- **`string_layout` becomes a special case of a shared two-word layout.** ADR-0004's
  `{data, count}` is now the *second* user of that shape rather than the only one, so the
  offsets are computed once. `string` keeps its own identity and its own projections; only
  the arithmetic is shared, which is ADR-0022 §2's rule applied to layout.
- **`Projection` grows two variants** — `ViewData` and `ViewCount` — rather than reusing
  `StringData`/`StringCount`. Reuse would type-check and would be wrong: the projection's
  *result type* differs (`*T` versus `*u8`), and the place-typing functions in both engines
  derive the type from the projection alone.
- **A new postfix operator.** `buf[]` is the first postfix operator added since the slice, so
  it needs the Pratt loop's postfix position, a `SyntaxKind` (`SLICE_EXPR`), an AST node,
  `jr-fmt`'s `is_expr_kind` *and* an emitter arm, `jr-hir`'s `Expr::Slice`, and the
  tree-sitter grammar. `jr-fmt` deleting it is the failure mode ADR-0042 hit twice; checked
  explicitly.
- **`escape.rs` treats `Expr::Slice` as an escape**, exactly like `UnOp::AddrOf` — and this is
  belt-and-braces rather than a fix, because an array is not register-representable and so was
  never promotable anyway (§2's third point states this precisely). A test asserts the escape
  set contains the sliced local, so the guarantee is pinned at the level where it is actually
  true rather than inferred from a program that would behave identically without it.
- **Two new diagnostic codes**: E0240 (an array where a view was expected, whose help names
  `buf[]`) and E0241 (`==` on a view). **E0242 is the first free code** after this wave.
  E0124's message loses its `[]T` clause and keeps the `[..]T` one, since dynamic arrays are
  still a later wave.
- **The bounds check gains its first non-constant length**, which means `constprop`'s
  ability to delete a provably-in-range check now has a case it *cannot* fold — and that is
  the correct outcome, not a regression. A view's length is unknown at compile time by
  definition.
