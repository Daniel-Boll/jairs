---
title: Procedures
description: Declaring procedures with parameters and return types, calling and nesting them, and the void procedure that returns nothing.
sidebar:
  order: 11
---

A procedure is a constant whose value is a function: `name :: (params) -> ReturnType { ... }`.
The parameter list, the return arrow, and the body are all optional in different combinations,
which gives a small family of shapes.

## Parameters and return types

```jr
add :: (a: s64, b: s64) -> s64 {
    return a + b;
}

// Parameters of the same type still each need their own annotation in
// Jairs-0; parameter grouping arrives with wave W2.
clamp_low :: (value: s64, floor: s64) -> s64 {
    if value < floor {
        return floor;
    }
    return value;
}

// A procedure returning nothing omits the arrow entirely.
discard :: (unused: s64) {
    return;
}
```

Each parameter is written `name: Type`, and the return type follows a `-> ` arrow. A few things
worth noting:

- Even when two parameters share a type, each still carries its own annotation — there is no
  `a, b: s64` grouping in this slice of the language (that arrives in the wave labelled W2).
- A procedure that returns nothing simply **omits the arrow**, as `discard` does. A bare
  `return;` with no value is how you leave such a procedure early.

## Calling, nesting, and discarding

```jr
zero :: () -> s64 {
    return 0;
}

one :: (a: s64) -> s64 {
    return a;
}

three :: (a: s64, b: s64, c: s64) -> s64 {
    return a + b + c;
}

main :: () {
    x := zero();
    y := one(1);
    z := three(1, 2, 3);

    // Nested calls.
    w := three(one(1), zero(), one(y));

    // A call whose result is discarded is a statement on its own.
    zero();
}
```

Calls use the familiar `name(args)` form and can nest arbitrarily: `three(one(1), zero(),
one(y))` passes the results of three calls as the arguments to a fourth. A call whose result
you don't need is a statement on its own — the final `zero();` computes and throws away its
result. (A *void* procedure's call must be written this way, since its non-result cannot be
bound.)

## The empty procedure

```jr
// A procedure with no parameters and no return value.
noop :: () {
}
```

The smallest procedure takes no parameters, returns nothing, and has an empty body.

See also [Book I — The Jairs Language](/language/introduction/).
