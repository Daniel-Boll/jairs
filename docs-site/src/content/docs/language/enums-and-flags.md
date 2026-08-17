---
title: Enums and flags
description: Nominal enumerations, their numbering rules, and flag sets.
sidebar:
  order: 7
---

Jairs has two enumeration forms: `enum` for a set of alternatives, and `enum_flags` for a set
of bits you combine. They share a spelling but behave differently on purpose.

## enum

An `enum` is a **nominal** type whose members are **namespaced**:

```jr
Colour :: enum {
    RED;
    GREEN;
    BLUE;
}

main :: () {
    c := Colour.GREEN;         // members are always qualified…
    d: Colour = .RED;          // …or bare, when the type is known from context
    if c == Colour.GREEN { … }
}
```

Members never enter the enclosing scope — you write `Colour.RED`, never bare `RED` in an
open context — so adding a member can never shadow an existing name. Where the expected type
is already known (an annotation, a `switch` case, a comparison against a `Colour`), the bare
form `.RED` works.

### Numbering: the rule that surprises people

Members auto-number from 0 in declaration order. An explicit value is allowed, and **later
members continue from it**, not from their position:

```jr
Status :: enum {
    OK :: 200;
    MISSING :: 404;
    NEXT;              // 405 — one past MISSING, NOT its index (2)
    ALSO_OK :: 200;    // duplicate values are legal
}
```

So `enum { A; B :: 10; C; }` numbers 0, 10, 11 — not 0, 10, 2. A member's value may also
**name a constant** whose initialiser is a literal, and auto-numbering continues from it. A
value that needs *evaluation* (`2 + 2`, a `#run`, another file's constant) or that names a
*sibling* member is <span class="jairs-status absent">absent</span>.

### What enums refuse

An enum is **not** an integer. You cannot pass a bare number where a `Colour` is expected,
and to get the underlying number you `cast`:

```jr
n := cast(s64, Colour.BLUE);   // 2
```

Only `==` and `!=` are defined between two enum values. **Ordering and arithmetic are
refused** (`Colour.RED < Colour.GREEN` is an error), because with auto-numbering that
comparison would be true by an accident of declaration order — a fact about the source file,
not about colours. And a plain `enum` refuses `|`: combining members is what `enum_flags` is
for.

An enum's chief payoff is [`switch` exhaustiveness](/language/control-flow/#switch): because
the compiler knows the member set, it can tell you when a `switch` has missed one.

## enum_flags

`enum_flags` numbers its members by **powers of two**, so they combine cleanly with the
bitwise operators:

```jr
Perm :: enum_flags {
    READ;      // 1
    WRITE;     // 2
    EXEC;      // 4
}

main :: () {
    p := Perm.READ | Perm.WRITE;         // a set: value 3
    can_read := (p & Perm.READ) == Perm.READ;
}
```

A flag value stays a `Perm` through `& | ^ ~`, keeping a *set of permissions* distinguishable
from a bare integer.

### Things worth knowing

- **A combination names no member.** `READ | WRITE` is 3, and no member has value 3 — that is
  the design. The type's job is keeping the set distinct from an integer, not naming every
  subset. You test a flag with `(p & Perm.READ) == Perm.READ`, which composes: `p & (A | B)`
  tests two at once.
- **The power-of-two rule after an explicit value** goes by the *value*, not the index: after
  `B :: 8` the next flag is 16. And after a named mask `AB :: 3` (not itself a power of two)
  the next flag is 4.
- **There is no way to build a flags value from a computed integer.** `cast(Perm, 3)` is
  refused — most integers are valid flag sets, so a wrong one would look right. You combine
  members with `|` instead.

## Bitwise precedence is not C's

Because flags lean on `&`, `|`, `^`, it is worth stating here: Jairs binds **bitwise tighter
than comparison**, so `flags & MASK == 0` means `(flags & MASK) == 0` — the reading almost
everyone intends, and the one C got wrong for historical reasons. The full operator table is
in [Operators and overloading](/language/operators-and-overloading/).

Next: [Arrays and views](/language/arrays-and-views/).
