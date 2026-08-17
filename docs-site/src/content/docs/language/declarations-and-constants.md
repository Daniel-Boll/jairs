---
title: Declarations and constants
description: The three declaration forms, and why procedures and structs are just constants.
sidebar:
  order: 3
---

Jairs has no `let`, `var`, `const`, `fn`, or `struct` keyword. Instead it has **three
declaration forms**, and everything — variables, constants, procedures, types — is spelled
with one of them. Learning these three forms is most of learning how Jairs code is shaped.

## The three forms

```jr
NAME :: value;       // constant  — known at compile time, never reassigned
name := value;       // inferred  — a variable whose type comes from the value
name: T = value;     // typed     — a variable with an explicit type
name: T;             // typed, default-initialised (zeroed)
```

Read the punctuation literally:

- **`::`** introduces a **constant**. The value is fixed at compile time. `MESSAGE ::
  "hi";`, `MAX :: 100;`.
- **`:=`** introduces a **variable** and **infers** its type from the initialiser. `i := 0;`
  gives `i` the type `s64`.
- **`: T`** introduces a variable with an **explicit type**, optionally initialised. `x: s64
  = 5;`, or just `count: s64;` which zero-initialises.

```jr
main :: () {
    MAX :: 100;          // a local constant
    i := 0;              // inferred s64
    total: s64 = 0;      // explicitly typed
    seen: bool;          // typed, defaults to false
}
```

## Default initialisation, and opting out

A declaration without an initialiser is **zeroed**. An `s64` starts at 0, a `bool` at
`false`, an array of `[20]u8` is twenty zero bytes, a struct has all fields zeroed.

If you deliberately want *uninitialised* memory — because you are about to fill it and the
zeroing would be wasted — you write `---`:

```jr
buf: [4096]u8 = ---;     // not zeroed; you promise to write before you read
```

Use `---` sparingly. Reading uninitialised memory is a bug the compiler tracks per slot, and
`---` opts out of the protection.

## Procedures and structs are constants

Here is the idea that makes the whole scheme cohere: **a procedure is a constant whose value
is a procedure, and a struct is a constant whose value is a type.** There is no separate
declaration syntax for them — they use `::` like any other constant.

```jr
add :: (a: s64, b: s64) -> s64 {   // a constant named `add`
    return a + b;
}

Point :: struct {                  // a constant named `Point`
    x: s64;
    y: s64;
}

Colour :: enum { RED; GREEN; BLUE; }   // likewise

PI :: 3.14159;                     // and an ordinary value constant
```

This is why you never see a `fn` or `class` keyword: the left of the `::` is always a name,
and the right is always a value — a number, a string, a procedure, a type. It is also why a
type can be *bound to a name* and passed around at compile time (`T :: Point;`), which the
chapter on [compile-time execution](/language/compile-time-execution/) builds on.

## Constants run at compile time

Because a constant's value is known at compile time, its initialiser can be *computed* at
compile time — including by running ordinary Jairs code:

```jr
COMPUTED :: #run add(2, 3);        // COMPUTED is 5, computed while compiling
```

`#run` is covered fully in [Compile-time execution](/language/compile-time-execution/). For
now, notice only that the same `add` you call at run time is the one the compiler executes to
produce the constant.

## What a name resolves to

Names are resolved by scope, and Jairs allows shadowing: an inner block may declare a name
that hides an outer one. Declaration order matters *within a body* — you cannot use a local
before it is declared — but top-level constants and procedures see each other regardless of
order, which is why `main` can call an `add` declared below it.

The next chapter, [Procedures](/language/procedures/), takes the procedure form apart in
detail: parameters, multiple return values, named and default arguments, and passing a
procedure as a value.
