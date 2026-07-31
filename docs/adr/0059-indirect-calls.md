# ADR-0059: A procedure is a value you can call through a pointer

- **Status:** Accepted
- **Date:** 2026-07-30
- **Deciders:** dboll
- **Resolves the plan contradiction** ADR-0058 §7 recorded and PLAN.md §7 carried: the rest of W3 —
  allocators, then temporary storage — needs calls through a procedure pointer, and §2.1 assigned
  indirect calls to no wave. This is that wave, placed inside W3 because everything after it depends
  on it.
- **Third feature of W3.**

## Context

`f := add; f(1, 2)` is refused today, and the refusal is unusually well-prepared. Six facts were
established by reading the tree, and four shaped the decisions.

- **`Callee::Indirect(Operand)` already exists in MIR**, with an arm in every pass that matches on a
  callee — `inline`, `dce`, `ssa`, `constprop`, `verify`, `dump` — and in *both* engines. Each of the
  two engine arms **refuses**, and each names the identical blocker: a procedure interns as
  `Item::ProcValue { decl: DeclId }`, but a call needs a `ProcRef`, and nothing bridges the two.
  **So this wave adds no MIR variant and no engine dispatch site**; it fills in two refusals.
- **The type side already works.** `jr-sema` types a bare `add` as its `Item::ProcType`, which
  ADR-0001 built with a `ContextKind` and ADR-0015 §4 made a first-class type. `f: s64 = add` is
  already E0214 "expected `s64`, found `(s64, s64) -> s64`" — so the checker knows the type; only
  lowering and the engines do not know the value.
- **A procedure-pointer *type* has no syntax.** `parse_type_inner` handles `*T`, `[N]T`, `[]T`,
  `struct`, `union`, `enum` and a name — not `(T, T) -> T`, which is what a proc-pointer parameter
  needs. This is the one genuinely new piece of surface, and §3 is about it.
- **`Item::ProcValue` carries a `DeclId`, not a `ProcRef`.** The bridge is a single local lookup:
  `hir.items[decl.index]` is a `ConstValue::Proc(ProcId)`, and `ProcRef::new(decl.file, proc)` is the
  answer. Both engine refusals said exactly this, so §2 is short.
- **`scan` refuses a proc-valued item used as anything but a direct callee** — the
  `"a file-level item has no value until jr-vm"` arm — because `reach.callees` did not contain it.
  §2 has to teach `scan` that a procedure name is now a legitimate *value*, which is the delicate part:
  the failure mode this project names first is a value that lowers to a placeholder.
- **A proc pointer's bits are not observable.** Nothing in Jairs-0 prints one, compares two, or casts
  one to an integer. Only *calling through it* is observable. **This is the fact that decides §4.**

## Decision

### 1. A procedure name is a value; a call through one is `Callee::Indirect`

```jr
add :: (a: s64, b: s64) -> s64 { return a + b; }

main :: () {
    f := add;              // a value of type (s64, s64) -> s64
    exit(f(20, 22));       // 42, called indirectly
}
```

`f := add` lowers `add` to an operand — `Item::ProcValue`, a constant. `f(20, 22)` lowers to
`Rvalue::Call { callee: Callee::Indirect(f), args }`, which both engines already have an arm for.

**Same-file targets only.** A `ProcValue`'s `DeclId` names a file and an index; bridging it to a
`ProcRef` is a local lookup only when the declaration is in *this* file. A cross-file procedure value
needs the other file's HIR, which is the same boundary an imported constant crosses (ADR-0055) and is
deferred for the same reason. Refused with a message, not miscompiled.

### 2. The `DeclId → ProcRef` bridge, and `scan` learns a procedure is a value

The two engine refusals become the same three lines: read `ProcValue { decl }`, look up
`hir.items[decl.index]` for its `ConstValue::Proc(proc)`, build `ProcRef::new(decl.file, proc)`.

**Native emits a real code address; the VM encodes the `ProcRef`.** §3 of the fork settled this: a
proc pointer's value is not observable, so the two engines need not agree on its bits. Native emits
`func_addr` for the callee's declared function and `call_indirect` with its signature. The VM encodes
the resolved `ProcRef` as a scalar and decodes it at the indirect call. Neither maintains a lookup
table, and the differential harness holds them equal on *behaviour*, which is all that is observable.

**`scan` must accept a procedure name as a value without opening the placeholder door.** ADR-0017 §4
and this project's first named failure mode both say: a construct the grammar allows, with no
representation, filled with a legitimate-looking value, is a silent miscompile. A procedure value is
*not* that — it has a real representation now — but the way `scan` learns it must be exact. A
procedure name resolves to `Res::Item`; the new rule is that an item whose `ConstValue` is a `Proc`
**is a value**, whether or not it is in `reach.callees`. The refusal that remains is for a *`#foreign`*
procedure taken as a value (§5) and for a *cross-file* one (§1) — both refuse, neither invents.

### 3. Procedure-pointer types: `(T, T) -> T`

```jr
apply :: (fn: (s64, s64) -> s64, a: s64, b: s64) -> s64 {
    return fn(a, b);
}
```

`parse_type_inner` gains an arm for `(`: a parenthesised, comma-separated list of parameter types,
then `->` and a return type. It lowers to a new `TypeRef::Proc { params, ret }`, which `jr-sema`
resolves to the **same** `Item::ProcType` a declared procedure has — so `fn`'s type and `add`'s type
are one interned entry, and passing `add` where `fn` is expected is an ordinary type match with no
special case.

**`ContextKind::Jairs`, always, in this wave.** The type syntax carries no `#c_call`, so every
proc-pointer type is a Jairs-convention one. A `#foreign` procedure's type is `CCall` and therefore a
*different* interned type, which is what makes §5's refusal fall out of the type system rather than
needing a check: `add`'s type matches `fn`'s and `write`'s does not.

**Rejected: reusing the results-list `(…)` syntax.** `TypeRef::Results` is `(s64, bool)` and is
reachable *only* as a return type (ADR-0052 §4). A proc-pointer type is `(s64, bool) -> T` — the `->`
is what distinguishes them, and the parser commits to one or the other only after it has seen whether
an arrow follows the closing `)`. They do not share a node, because a results list has no return type
and a consumer meeting one where the other belongs has found a bug.

### 4. A proc pointer is one machine word, and its bits are unspecified

The parameter `fn: (s64, s64) -> s64` is one pointer-sized value, passed in a register like any
scalar. Its bit pattern differs between the engines — a real code address natively, an encoded
`ProcRef` in the VM — and that is not a compromise: **nothing observes the bits.** A program that
printed a proc pointer, compared two, or cast one to an integer could tell the engines apart, and none
of those is expressible in Jairs-0. The differential harness compares *what calling through it does*,
which agrees.

**Rejected: a uniform encoded `ProcRef` in both engines.** It would make a stored or compared proc
pointer bit-identical across engines. That buys a property nothing in scope observes, and costs native
a `ProcRef → address` table and an indirection at every indirect call. When comparison or printing of
a proc pointer becomes expressible, that is the ADR that revisits this — with a program that can see
the difference, which this wave does not have.

### 5. A `#foreign` procedure cannot be taken as a value yet — E0256

```jr
write :: (…) -> s64 #foreign libc "write";

g := write;   // E0256: a #foreign procedure cannot be used as a value yet
```

A `#foreign` procedure's type is `ContextKind::CCall` (ADR-0001), and the VM reaches it through a
separate libffi path rather than a `ProcRef`. An indirect call to one is therefore a *second*
mechanism, not a special case of the first, and this wave implements one convention. Refused with its
own code so the message can say "yet" rather than reporting a type mismatch a reader cannot act on.

**Why a check and not a type mismatch.** `g := write` would otherwise type fine — `write` has *a*
proc type — and fail only at the indirect call, or worse, lower to a `ProcValue` the VM's `ProcRef`
path could not resolve. E0256 catches it at the point of taking the value, where the reader wrote the
mistake.

### 6. What is deliberately absent

- **Cross-file procedure values** (§1). Same boundary as ADR-0055's imported constants.
- **`#foreign` targets** (§5).
- **Comparing or printing a proc pointer** (§4). No syntax expresses it, so the unspecified-bits
  decision is not yet observable.
- **`#c_call` proc-pointer types.** The type syntax has no attribute, so every proc-pointer type is
  Jairs-convention. This is the `ContextKind`-in-a-pointer-type distinction ADR-0057 §4 anticipated
  and this wave still does not need, because §3 makes the two conventions different interned types
  rather than one type with a flag to check.

## Consequences

- **Two refusals become implementations**, one per engine, and no new MIR variant or dispatch site —
  the payoff for `Callee::Indirect` having been carried since the slice.
- **One new `TypeRef` variant, `Proc`**, so every exhaustive match over `TypeRef` in `jr-hir` and
  `jr-sema` is a compile error until taught. That is the house style working.
- **One new diagnostic code, E0256**, for a `#foreign` procedure taken as a value. **E0257 is the
  first free code.**
- **`jr-fmt` needs the `(T, T) -> T` type**, and the formatter has lost a construct in eight of the
  last nine waves. A test must assert survival *and* canonicalisation.
- **The tree-sitter grammar needs the type too.** It is a genuinely new shape — `(` in type position —
  so a missing arm is an ERROR node gate 6 catches, not the silent `context`-shaped drift.
- **`native` and the VM disagree about a proc pointer's bits, on purpose (§4)**, so no snapshot may
  print one and the differential harness is what proves calling through it agrees.
- **An allocator is now expressible in principle.** A struct of procedure pointers is the shape, and a
  proc-pointer struct *field* is the one step past this wave's slice — which is where the allocator
  wave picks up.
