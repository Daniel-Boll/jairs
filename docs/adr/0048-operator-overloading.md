# ADR-0048: `operator +` is a constant whose name is an operator, and one operand must be local

- **Status:** Accepted
- **Date:** 2026-07-29
- **Deciders:** dboll
- **Depends on:** ADR-0012, which makes every declaration an instance of `name :: value` — this
  reuses that form rather than inventing a declaration. ADR-0014 §3's collision rules apply
  unchanged. ADR-0002's trapping/wrapping split is what §2 turns on.

## Context

`PLAN.md` §2.1 lists operator overloading as the last feature of W1. ADR-0046 §6 deliberately
excluded it from the `xx` wave, on the grounds that it "shares nothing with these two: overloading
is a *name resolution and dispatch* feature", and named four questions it would need to answer.
This ADR answers them.

Six facts were established by reading the code before this ADR was written, and three of them
shaped the decisions.

- **Names are a flat per-file map keyed on `Symbol`.** `FileSignatures::lookup` takes a `Symbol`
  and returns a `SigEntry`; `ItemScope` is what a module exports. There is no namespace, no trait,
  no method table. **This is the fact that decides §1**: a declaration form that lands in that
  map is imported, shadowed and reported-as-ambiguous for free, by machinery ADR-0014 already
  built and tested.
- **`check_binary` already routes every operator through a refusal helper.** `reject_operator`,
  `reject_enum_operator`, `reject_float_operator` and `reject_bitwise` are the four places an
  operator is turned down. So overloading is a *lookup inserted at those points*, not a rewrite of
  operator checking.
- **`unify_operands` refuses unequal operand types** with E0214 before any refusal helper runs.
  That is the hook a mixed-type overload — `Vec2 * float64` — has to reach, and it means the
  lookup must happen *before* unification rather than after it.
- **`parse_item` dispatches on `IDENT`**, and `parse_name` expects an `IDENT`. So an
  `operator + :: …` declaration needs its own arm in both; it cannot fall out of the existing
  const-decl path.
- **`Callee::Direct` names a `(FileId, ProcId)` pair** (ADR-0018 §5), and an imported callee is
  resolved by `jr-db` from the other file's signatures. An overload call is an ordinary direct
  call, so MIR needs **no new callee kind** — this is the same payoff `xx` had.
- **`SigKind` has five variants** and a diagnostic already refuses to call a union a struct
  (ADR-0045 §5). An overload needs its own variant for the same reason.

## Decision

### 1. An overload is `operator OP :: (…) -> T { … }` — a constant whose name is the operator

```jr
Vec2 :: struct { x: float64; y: float64; }

operator + :: (a: Vec2, b: Vec2) -> Vec2 {
    r: Vec2;
    r.x = a.x + b.x;
    r.y = a.y + b.y;
    return r;
}
```

This is ADR-0012's `name :: value` form with an operator where the name goes. `operator` becomes a
keyword; the declaration lowers to an ordinary `Proc` and an ordinary `ConstValue::Proc`.

**The name is a synthetic `Symbol`.** `+` is not a legal identifier, so the interned name is
`"operator+"` — one token, no space — and nothing can collide with it because a user cannot write
that identifier. That single decision is what makes every other piece free:

- **Importing works already.** An overload is exported by `ItemScope` like any constant, so
  `#import "Vectors"` brings its operators with it. No new export mechanism.
- **ADR-0014 §3's collision rules apply unchanged.** A local `operator +` shadows an imported one
  silently; two *different* modules exporting `operator +` for the same operand pair is E0211 at
  the use site. Both fall out of the existing map rather than needing a parallel rule, which is
  what ADR-0046 §6 identified as the hard part and is now the cheap part.

  **One thing does *not* fall out, and implementation is what revealed it.** The name map gives
  imports and shadowing, and it cannot give **duplicate detection**: one operator legitimately has
  many overloads, so `operator * :: (Vec2, s64)` and `operator * :: (s64, Vec2)` both intern to
  `operator*` and `jr-hir`'s duplicate-name scan reported the second as a redefinition. That scan
  now exempts overloads, and the *genuine* duplicate — same operator, same operand pair — is
  reported by `jr-sema` (E0246) where the real key `(operator, lhs, rhs)` exists.

  Exempting the scan without adding the sema check would have been a silent last-write-wins, which
  is why both halves landed together: the exemption was verified to open the hole before the check
  was written to close it.
- **Hover, goto-definition and rename work** on the declaration, because the LSP reads the same
  signatures.

**Rejected: a `#operator(+)` attribute on an ordinary procedure.** `add_vec :: (…) #operator(+)`
keeps a callable name and needs no keyword. Rejected because it creates *two* ways to invoke one
thing — `p + q` and `add_vec(p, q)` — and because the overload table then has to be a side map
keyed on operator-plus-operand-types, which duplicates the resolution ADR-0014 already does. The
side map is where a second set of shadowing and ambiguity rules would have had to be invented.

**Rejected: methods or traits.** Jairs has neither and adding one to serve operators would be a
far larger language decision than the feature justifies. Jai has no traits either.

### 2. `+ - * / %` and `== != < <= > >=` may be overloaded; the wrapping forms may not

**Overloadable:** the five arithmetic operators and the six comparisons.

**Not overloadable, and each for its own reason:**

- **`+% -% *%`** mean "wrap the machine integer at this width" (ADR-0002). That is a statement
  about a two's-complement representation, not an abstract operation, so it has no meaning for a
  user type. A `Vec2 +% Vec2` would be asking which of the struct's bits wrap.
- **`& | ^ ~ << >>`** are reserved by ADR-0043 for `enum_flags`, whose whole design is that
  `&` on a flags type yields the flags type. Letting a user redefine them would put a second
  meaning on the operators that carry the flags semantics.
- **`&& || !`** are control flow, not operators: MIR has no `BinOp` for them at all
  (`build.rs` lowers them as branches), so an overload could not short-circuit. Overloading them
  would silently make both sides evaluate.

**A comparison overload returns whatever it declares**, and sema then requires `bool` where a
condition is wanted — so `operator == :: (a: Vec2, b: Vec2) -> s64` compiles and then fails at
`if`, with the existing E0222. Deliberately *not* a special rule forcing `-> bool`: the check
already exists one layer up, and duplicating it would give two diagnostics for one mistake.

**This unblocks a decision ADR-0044 §5 deferred.** `==` on a view was refused because "same
storage" and "same contents" are both plausible and Jairs would not pick. A user may now pick, for
their own types — the builtin refusal for views stands, because §3's orphan rule keeps `[]T` out
of reach.

### 3. At least one operand type must be declared in the same file

An **orphan rule**, checked in sema. `operator + :: (a: s64, b: s64) -> s64` is refused: neither
operand is local, so this file may not say what `+` means for them.

```jr
// Legal: Vec2 is declared here.
operator * :: (a: Vec2, b: float64) -> Vec2 { … }

// Refused (E0246): neither operand is declared in this file.
operator + :: (a: s64, b: s64) -> s64 { … }
```

The rule is deliberately about the *declaration site*, which for a nominal type is exactly what
`DeclId` records (ADR-0015 §1) — so the check is a `DeclId.file` comparison rather than anything
new. A structural type (`*T`, `[N]T`, `[]T`) is declared nowhere and therefore never satisfies it,
which is what keeps `[]T` equality builtin-only per §2.

**Rejected: anything goes, last definition wins.** Simplest, and closest to Jai. Rejected because
an `#import` could then silently change what `+` means for `s64` — action at a distance, which is
the objection ADR-0014 §3 already made about import order deciding behaviour. It would also make
two modules overloading the same builtin pair a conflict with no principled winner.

**Rejected: builtin types are closed, user types only.** Strictest, and it removes the orphan
question entirely. Rejected because it forbids `Vec2 * float64` — a scalar multiply is the
most-wanted mixed case, and refusing it would make the feature feel arbitrary rather than
principled.

### 4. Resolution: exact operand types, no conversion, and one candidate

An operator expression looks for an overload **before** `unify_operands` runs, because unification
is what refuses unequal types and a mixed-type overload must be reachable.

Resolution requires an **exact** match on both operand types. No implicit conversion, no
promotion, no ranking — consistent with a language that requires `cast` for a widening integer
(ADR-0037 §2). So `Vec2 * float64` and `float64 * Vec2` are two declarations, and writing only one
means only one order works. That is a real cost and it is the right one: an overload set with
ranking rules is where C++ became unteachable.

**A builtin meaning always wins.** `s64 + s64` never consults the overload table, so no overload
can slow down or change arithmetic on builtin types — and §3's orphan rule means none can be
declared for them anyway. The lookup happens only where the builtin path was going to *refuse*.

**Resolution order for the lookup itself** is ADR-0014 §3's, unchanged: this file first, then
imports, with two imports offering the same overload being E0211 at the use site.

### 5. Lowering is an ordinary direct call

`p + q` with an overload lowers to `Rvalue::Call { callee: Callee::Direct(…), args: [p, q] }`.
No new MIR node, no new callee kind, no change to either back end.

That is worth stating as evidence the design is right rather than as a convenience: a feature
whose lowering needs nothing new is one that fits the existing shape. It also means an overload is
inlinable by ADR-0021's inliner on the same terms as any small procedure, with no special case.

`jr-sema` records which overload each operator expression resolved to, in a side map on
`CheckOutput`, because MIR must not re-run resolution — two implementations of one rule are two
chances to disagree, which is why `jr-mir` reads `TypeMap` rather than recomputing types.

### 6. What is deliberately absent

- **No unary operator overloading.** `-a` and `!a` and `~a` stay builtin. Unary `-` on a `Vec2` is
  a reasonable wish; it needs its own declaration form (`operator -` with one parameter, which
  collides with the binary form's name) and that ambiguity deserves its own decision.
- **No `[]` or `()` overloading.** Indexing is tied to `Statement::BoundsCheck` (ADR-0039 §1) and
  a user-defined index would have no length to check against. A callable object needs a decision
  about what a procedure *is*.
- **No compound-assignment overloading.** `v += w` is not sugar that consults an overload here.
  It could desugar to `v = v + w` and reuse the `+` overload, which is probably right — recorded
  as owed rather than assumed, because it interacts with whether an overload may take its left
  operand by pointer.

## Consequences

- **`OPERATOR_KW` is a new keyword**, so it needs the lexer table, `from_keyword`, `static_text`,
  the tree-sitter grammar and the highlight query — and it goes *outside* `is_reserved_keyword`'s
  range, because it was never reserved (ADR-0043's lesson, and `xx` was the fourth keyword to make
  the reverse trip).
- **`parse_item` and `parse_name` both need an arm.** `parse_item` dispatches on `IDENT` and would
  otherwise treat `operator` as a stray token; that is the same class as `TYPE_START` missing three
  keywords in ADR-0045.
- **`jr-fmt` needs the declaration form**, in the const-decl dispatch *and* the kind predicate —
  sixth consecutive wave, and the fifth deleted every `xx`.
- **One new diagnostic code, E0246** (an overload for two foreign types), making **E0247 the first
  free code**. `PLAN.md` §7 claimed E0245 was free *after* ADR-0047 took it; that was caught by
  grepping rather than trusting the line, and the line is corrected in the same commit.
- **`SigKind::Operator`** joins the five existing variants, so a diagnostic can say "`+` is an
  operator, not a procedure" rather than mislabelling it.
- **A corpus program must exercise a *mixed-type* overload and both orders**, because §4's
  no-ranking rule is only visible when one order is missing. And a differential test must prove
  the overload runs identically in both engines, since it lowers to an ordinary call and a
  disagreement would be about the call rather than the operator.
- **Every overload in the corpus returns a scalar, and that is forced rather than chosen.** The
  native back end cannot return an aggregate at all — measured: a `Vec2`-returning `operator +`
  gives 37 under `jr run` and fails `jr build` with "returning an aggregate … is not supported by
  this back end yet". So the corpus program would have tested that pre-existing hole instead of the
  operator. Recorded because the natural first example of an overload *is* `Vec2 + Vec2 -> Vec2`,
  and the next reader will try it.
- **`jr-mir`'s dump names an overload `operator + #3`.** The interned name is shared by every
  overload of one operator, so printing it would make four distinct procedures indistinguishable in
  a snapshot — which is the one thing a snapshot exists to prevent. The index disambiguates without
  printing a `FileId`, which `AGENTS.md` forbids because load order renumbers it.
- **Const-eval gets an empty overload map, deliberately.** It runs before the check phase, so the
  map does not exist yet, and asking for it would make const-eval depend on checking — the cycle
  ADR-0018 §3 avoided by putting const-eval downstream of *signatures*. The consequence: an
  overload cannot be used in a `#run` or a `::` constant, and that is a refusal rather than a wrong
  answer.
