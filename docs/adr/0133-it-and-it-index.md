# ADR-0133: `it` and `it_index` — the nameless `for` at last

- **Status:** Accepted
- **Date:** 2026-08-17
- **Deciders:** dboll
- **Wave 4 of eight.** ADR-0128 was wave 1 (instantiation backtraces), ADR-0129 wave 2 (enum member
  from a constant), ADR-0130–0132 wave 3 (`Math`'s vec/mat/quat). This is the **sixth** of
  ADR-0127 §3's six unkept promises to be kept, and the wave `for xs { it }` had to survive several
  passes of documentation before anyone wrote the two lines it turned out to need.
- No design fork was put to the decider. §7 of PLAN.md had already recorded the shape — *ordinary
  injected locals, **not** reserved keywords* — and the wave that lifts the refusal implements that
  decision. §2 records what "ordinary" turned out to force in practice.

## Context

### The refusal, and why it lasted this long

`for xs { it }` failed to parse. The parser's `parse_for_stmt` required an IDENT after `for` (and
after an optional `<`), then a `:`, then an iterable — so `for xs { }` read as `for xs:` with the
colon missing. E0122's own note said "arrives in wave W2" long after W2 shipped, which is exactly
the shape ADR-0127 identified as *the highest-value thing an audit can find*: an expired
deferral that reads as a considered decision while being false. That one, this wave closes.

### Why the machinery was almost there already

`Stmt::For` in HIR carries `value: LocalId` and `index: Option<LocalId>`, and MIR's `for_stmt`
already handles the four combinations of (has-index, is-range). The old parser rejected the shape
that reads them; the injection is a **binding**, not a new statement kind. That is why the wave is
one parser change plus one lowering change, without new IR.

## Decision

### 1. `it` and `it_index` are **ordinary injected locals** — bindings, not reserved keywords

In a nameless `for xs { … }` the HIR lowering allocates a local named `it` for the element (and,
for a sequence iterable, a second local named `it_index` for the 0-based iteration counter). Both
are ordinary locals in every other sense: name resolution sees them, a body can declare `it := 5`
to shadow the injection, and a nested nameless `for` inside another rebinds `it` to the inner
element for the duration of its own body. That is the shape §7's table decided against making `it`
a reserved keyword, and this wave delivers.

**Rejected: reserve `it` as a keyword.** It reads better as reserved (a reader knows the name is
special) and it would prevent a caller from accidentally shadowing it. Two prices this wave
declined to pay: every existing program using `it` as a local name would fail (`Grafana::it_over_...`
in some field of some struct is exactly the kind of collision that finds itself), and the reader
would then have two names — `it` and `it_index` — carved out for a construct that many programs
never use. Making them ordinary locals means the surface change is *smaller than* the interior of
the feature, and the reader who never writes a nameless `for` never meets the name.

**Rejected: inject `it` as a hidden well-known symbol not spellable by a program**, the way
`operator+` is (ADR-0048). That would prevent shadowing at the price of a shadowing that no test
of nameless-for currently forbids. It also would not compose with a nested `for xs { for ys { it }
}`, where the inner `it` genuinely wants to shadow the outer one.

### 2. `it_index` is **absent for a range**, not just useless

`for 0..5 { it }` binds `it` to the current value (0, 1, 2, 3, 4). It does *not* also inject
`it_index`, because a range's counter *is* its value in the MIR MIR already builds — the case
`(None, false) => Counter::Local(value)` in `for_stmt`. Injecting `it_index` for a range would
require MIR to also allocate a separate counter that mirrors the value, and the wave that lifts
`for x, i: a..b` — which is *also* an uninitialised-value bug today for the same reason — will
lift `for 0..5 { it_index }` too. This wave does not touch MIR at all, which is what makes the
change one lowering line and not a range-counter refactor.

**Rejected: inject `it_index` for ranges by making MIR allocate a shadow counter.** It would work
and it would be self-consistent (`it_index` always available), and it would be a range-loop MIR
change disguised as an `it`-injection wave. Kept separable so a regression in either stays
attributable. Named to the wave that lifts `for x, i: a..b`, because the two forms fail for the
same reason and should be fixed together.

### 3. The parser is disambiguated by **fixed lookahead**, not by a token class

The three shapes a `for` can take now are:

    for x: iter { … }              // named element
    for x, i: iter { … }           // named element and index
    for iter { … }                 // nameless

They are pointwise disambiguated by lookahead:

- `IDENT :` → named
- `IDENT , IDENT :` → named with index
- anything else → nameless (`iter` starts here, whatever it is)

The nameless form's `iter` starts with `IDENT` in most real programs (`for xs { … }`), so the
disambiguation is on *what follows the IDENT*, not on whether an IDENT is present. If the next
token is `:` or `,`, it's a name; otherwise it's the iterable. This is the same shape
`parse_labelled_loop` uses for `outer: for` — looking two tokens ahead of an IDENT `:` to decide
whether it is a declaration or a label.

**Rejected: require `for it := iter { … }` — explicit declaration of `it`.** It reads well and
matches Rust, and it would need no injection at all. It also loses the *point* of `it` in Jai and
Odin: the nameless form is a shorthand, and requiring the name written out reduces to the named
form we already have. If a caller wants the name written out, `for x: iter` already works.

### 4. What "ordinary" turns out to force — the range-shadowing gotcha

Shadowing works: `for xs { it := 5; … }` declares a new local `it` in the body's scope, which
shadows the loop's `it`. That is a mechanical consequence of the injection being an ordinary
binding — HIR's scope stack handles it — and the corpus file pins it.

**The one wart**: `for 0..5 { it_index }` is E0201 (unresolved name), not "you're inside a range,
`it_index` doesn't exist here". That is a worse diagnostic than the feature deserves, and it is
what §2's separation costs. Fixing it means either (a) still injecting `it_index` for a range with
a "not usable, ask by the range's value name" error at any reference to it, or (b) a note on E0201
that recognises `it_index` inside a range. Both belong in the wave that lifts `for x, i: a..b`,
which will already be writing MIR for the range-with-index case.

## Consequences

- **The eight-wave programme is 5 of 8 done.** Waves 5–7 remain (nested procedures, `[..]T` dynamic
  arrays, `$$T`, and `print(fmt, ..Any)` — four remaining, since 5 was renumbered as wave 8 for
  print — see PLAN §7).
- **1010 tests, unchanged; +1 corpus file** — `valid/106-it-and-it-index.jr`. The pattern for library
  waves (ADR-0130/0131/0132) recurs here, because the coverage a corpus file adds *is* the
  differential's, and neither the parser nor the HIR change deserves a Rust unit test that a
  corpus file cannot exercise.
- **Parser fix: `reserved_keyword_for_is_rejected` moved** to check `for;` (still an error, still
  a check that `for` is reserved) — the previous input `for x { }` is now a valid nameless-for,
  which is precisely the wave's point.
- **Tree-sitter grammar updated** so gate 6 still passes; the two-form disambiguation reads there
  as `optional(seq(name, optional(seq(",", index)), ":"))` around the iterable.
- One item is **deferred rather than declined**: `for x, i: a..b` and `for 0..5 { it_index }` want
  the same MIR fix (a separate counter for the "iteration count" beside a range's "current
  value"), and that is one wave rather than being scattered across this one.
