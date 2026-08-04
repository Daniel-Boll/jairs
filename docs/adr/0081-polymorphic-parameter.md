# ADR-0081: A single `$T` parameter, inferred from the call and instantiated structurally

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W5 sub-wave 1.** W5 — Polymorphism is 8–12 weeks (PLAN §2.1), so it follows W4's discipline: sub-waves
  that each deliver a verifiable capability. This is the first — one `$T` parameter — and it is the slice
  that forces every architectural decision while keeping each one small.

## Context

### 0. What running found

`$T` does not lex: `id :: (x: $T) -> T` reports `unexpected character '$'`, and a lexer test *asserts*
`$` is `UNKNOWN`. There is no polymorphism scaffolding anywhere in `crates/`. So W5 is greenfield, and this
sub-wave starts at the lexer — as ADR-0080's `#code` did at the grammar.

Two decisions are already made and hold up:

- **ADR-0005 fixes instantiation *identity*: structural**, keyed on the tuple of resolved, interned
  comptime-argument IDs, so `id(x: Entity)` reached from two files is one instantiation. It does **not**
  fix which *phase* does the work, which is §2's decision.
- **ADR-0005's key is now available.** It said "once `Type` becomes a first-class comptime value (wave W4)
  structural identity is forced anyway" — and W4 delivered exactly that (ADR-0071, ADR-0075): a type is an
  interned `PoolId`, so a type argument keys like any other value. The dependency PLAN §2.1 names ("W4's
  InternPool value identity") is satisfied.

### The shape of the smallest useful slice

A polymorphic procedure differs from an ordinary one in one structural way: **its signature is not
concrete**. `id :: (x: $T) -> T` has no parameter type until a call supplies one, so it cannot be checked
once in the signature phase the way every procedure has been. It is checked *per instantiation*. That is
the single new idea this sub-wave introduces, and everything else follows from giving it the smallest
possible expression: one type variable, inferred from one argument.

## Decision

### 1. `$T` introduces a type variable; a later bare `T` refers to it

`$` lexes as its own token. In a parameter's **type** position, `$T` declares `T` as a polymorphic type
variable bound by that signature; a bare `T` elsewhere in the same signature (another parameter, the return
type) refers to it. `id :: (x: $T) -> T` therefore has one variable `T`, bound from `x`'s argument and
returned.

`TypeRef` gains a `Poly(Symbol)` variant for `$T`. It is distinct from `TypeRef::Name` because a name
resolves to an existing type and a `$T` *binds* one — conflating them would make `$s64` either an error or
a silent rebind of a builtin, and keeping them apart lets sema say which is meant.

**Inference-first, which is why `$T` and not `id<T>`.** `$T` at a use of a type reads "bind `T` from the
argument here", so `id(42)` needs no explicit type argument — the argument's type *is* the binding. Angle
brackets front-load explicit type arguments, the opposite ergonomic default, and Jai (which this language
follows) spells it `$T`. Explicit type arguments are a later sub-wave's concern, not this one's.

### 2. Instantiation happens at the call, in the check phase, keyed structurally

A polymorphic procedure gets **no concrete signature**. When a call reaches one, `check_call`:

1. infers each `$T` from the corresponding argument's type (this sub-wave: exactly one `$T`, one argument
   position binding it);
2. forms the structural key — the tuple of bound type IDs (ADR-0005), which for one `$T` is one `PoolId`;
3. interns the **concrete** `ProcType` (`$T` replaced by the bound type) and, if this key is new, records
   an instantiation to be checked and lowered as that concrete procedure;
4. checks the call against the concrete signature, so a second argument typed `T` must match the binding.

**Why the check phase and not a separate pass.** Checking already knows a call's argument types and already
reports a mismatch there; inferring `$T` from an argument and interning the concrete type is a few lines on
top of machinery `check_call` has. A separate monomorphisation pass would re-walk every call to rediscover
what checking already computed, and MIR-time instantiation would put type inference into a crate ADR-0017 §4
keeps a pure fold. This is ADR-0018 §3's shape reused: the phase that has the information does the work.

**The body is checked per instantiation.** An ordinary procedure's body is checked once against its one
signature; a polymorphic one is checked once *per distinct instantiation*, because `T`'s type differs
between them and a body correct for `s64` may be wrong for a struct. Structural identity (ADR-0005) bounds
this: N distinct argument types give N instantiations, not N calls.

### 3. Instantiations lower as ordinary concrete procedures

Once instantiated, an instantiation *is* a concrete procedure — a `ProcType` with no `$T` left — so it
lowers through the existing path with no new MIR and no engine change. Both engines see ordinary
monomorphic code, which is what makes the differential able to check a polymorphic program at all: there is
nothing polymorphic left by the time either back end runs.

This is the same payoff ADR-0048 got for operator overloading and ADR-0059 for procedure values: the new
surface is resolved away before MIR, so the back ends learn nothing.

### 4. What is deliberately absent

- **`$$T`** — a comptime-only polymorphic parameter, whose *value* (not just type) must be known at compile
  time. Its own sub-wave: it interacts with const-eval, which is its own can of worms.
- **Multiple distinct type variables** — `pair :: (a: $A, b: $B)`. This sub-wave allows one `$T` used
  across several positions (`swap :: (a: $T, b: $T)` is fine — one variable, two uses), but not two
  independent ones. The extension is mechanical once one works and is left to keep this slice small.
- **`#modify`, `#bake_arguments`, `#expand`** — the macro family, each its own decision (PLAN §2.1).
- **Polymorphic structs** — `Array($T)`. A polymorphic *procedure* is the smaller start.
- **Explicit type arguments** — `id(s64, 42)`. Inference covers the common case; explicit arguments are for
  when inference cannot (a type parameter used only in the return), which no example here needs.
- **Instantiation backtraces beyond one frame.** A diagnostic in an instantiated body names the
  instantiation (`id($T = Point)`, ADR-0005's nominal display); a *chain* of instantiations is a later
  concern.

## Consequences

- **The signature phase gains a notion of "not concrete".** A polymorphic procedure is recorded but its
  parameter types are not fully resolved, so a `$T` reaching a place that needs a concrete type (a field, a
  variable) without an instantiation is an error rather than a silent `PoolId::ERROR`. This is the one place
  the phase structure changes.
- **Instantiation caching is the structural key, and nothing more.** ADR-0005 already argued the key;
  building it here is interning the bound-type tuple and looking it up. Cross-file de-duplication comes for
  free because the key is the interned type, not the call site.
- **Both engines are untouched** (§3), so the differential checks a polymorphic program by checking its
  instantiations, which are ordinary. `id(42)` and `id(true)` are two concrete procedures the harness runs
  like any other.
- **This is the first of W5's sub-waves**, and it says so: `$$T`, macros, polymorphic structs and multiple
  type variables are each named as absent (§4) rather than implied, so the next sub-wave starts from a
  stated boundary rather than a guess.
