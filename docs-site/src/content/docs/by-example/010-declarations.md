---
title: Declarations & constants
description: How Jairs binds names — compile-time constants with `::`, typed and inferred variables, and the one grammar ambiguity after `::`.
sidebar:
  order: 10
---

Jairs has one uniform declaration syntax built from a colon. A double colon `::` binds a
compile-time constant, a single colon with a type declares a typed variable, and `:=` declares
a variable whose type is inferred from its initialiser.

## Constants with `::`

```jr
// `::` binds a compile-time constant. The right-hand side must be
// evaluable at compile time.
MAX_ENTITIES :: 4096;
GREETING :: "hello";
DEBUG :: false;

// Constants may refer to earlier constants; declaration order does not
// matter, because semantic analysis is lazy and on-demand.
LIMIT :: MAX_ENTITIES;
DERIVED :: MAX_ENTITIES + 1;
```

The `::` form binds a name to a value that must be known at compile time — an integer, a
string, a boolean, or an expression over other constants. Because the compiler resolves names
lazily and on demand rather than top to bottom, a constant may name another constant declared
later in the file; `LIMIT` and `DERIVED` above both build on `MAX_ENTITIES`, and it would not
matter if that declaration came afterwards.

## Typed variables

```jr
main :: () {
    // Explicit type, explicit value.
    a: s64 = 7;

    // Explicit type, default-initialised to the type's zero value.
    b: s64;

    // Explicitly uninitialised: the compiler will not zero this, and reading
    // it before assignment is an error caught in wave W3.
    c: s64 = ---;

    d: bool = true;
    e: string = "text";
    f: *s64 = *a;
    g: u8 = 255;

    b = a;
}
```

A `name: Type` declaration gives the type explicitly. Three initialisation forms sit side by
side:

- `a: s64 = 7` — explicit type and explicit value.
- `b: s64` — no initialiser, so the variable takes its type's zero value.
- `c: s64 = ---` — the `---` token means *explicitly uninitialised*. The compiler will not zero
  the storage, and reading the variable before it is assigned is a diagnostic (caught in the
  wave labelled W3). This is the escape hatch for when you know you will write before you read
  and do not want to pay for a zeroing you don't need.

The remaining lines show that the same form works for `bool`, `string`, a pointer type `*s64`
(here taking the address of `a`), and the byte type `u8`.

## Inferred variables with `:=`

```jr
main :: () {
    // `:=` infers the type from the initialiser.
    count := 10;
    flag := false;
    label := "inferred";

    // Inference propagates through calls.
    doubled := twice(count);
}

twice :: (n: s64) -> s64 {
    return n + n;
}
```

`:=` drops the type annotation entirely and infers it from the right-hand side: `count` is an
integer, `flag` a `bool`, `label` a `string`. Inference also flows through calls — `doubled`
takes the return type of `twice`, which is `s64`.

## The one ambiguity: `::` before a parenthesis

```jr
// A constant whose value is a parenthesised expression.
//
// This is the one genuinely ambiguous point in the Jairs-0 grammar: after `::`,
// a `(` may open a procedure's parameter list or a parenthesised expression.
// Both parsers resolve it by looking past the matching `)` -- a parameter list
// can only be followed by `->`, `{`, or `#foreign`.
GROUPED :: (1 + 2) * 3;
NESTED :: ((1));
CHAINED :: (1 + 2) * (3 - 4);
NEGATED :: -(1 + 2);

// For contrast, the procedure forms that start the same way.
no_args :: () {
}
with_ret :: () -> s64 {
    return 1;
}
```

Because a procedure is also introduced with `::`, an open parenthesis right after `::` is
genuinely ambiguous: it could begin a procedure's parameter list, or it could be a
parenthesised expression that happens to be the constant's value. Both the hand-written parser
and the tree-sitter grammar resolve it the same way — they look past the matching `)`. A
parameter list can only be followed by `->`, `{`, or `#foreign`; anything else means the
parenthesis opened an expression. So `GROUPED :: (1 + 2) * 3` is a constant, while
`no_args :: () { }` is a procedure.

Inside a procedure body there is no ambiguity at all, because the right-hand side of `:=` is
always an expression:

```jr
main :: () {
    // In expression position there is no ambiguity, because the right-hand
    // side of `:=` is always an expression.
    local := (1 + 2) * 3;

    // `no_args()` returns nothing, so its result cannot be bound
    // (ADR-0016 §2). Calling it as a statement is the way to write this.
    no_args();
}
```

Note the last line: `no_args()` returns nothing, so its result cannot be bound to a variable
(ADR-0016 §2). A call to a void procedure is written as a statement on its own.

See also [Book I — The Jairs Language](/language/introduction/).
