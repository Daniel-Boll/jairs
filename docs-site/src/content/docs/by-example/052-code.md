---
title: "#code"
description: "Unquoted source spliced into the enclosing scope — an #insert without the quoting."
sidebar:
  order: 52
---

`#code { … }` (ADR-0080) is unquoted source that splices into the enclosing scope. It is the last
member of the `#insert` family: `#code { n := 7; }` is exactly `#insert "n := 7;"` written without
quotes.

```jr
#import "Basic";

main :: () {
    n := 0;

    // The simplest form: a local declared inside the body is visible after it.
    #code {
        seven := 7;
    }
    if seven == 7 {
        n = n + 1;
    }

    // Several statements, which is what a braceless form could not express.
    #code {
        a := 2;
        b := a * 3;
    }
    if b == 6 {
        n = n + 2;
    }

    // The body reads an enclosing local, so the splice is genuinely in this scope.
    outer := 10;
    #code {
        from_outer := outer + 5;
    }
    if from_outer == 15 {
        n = n + 4;
    }

    // A body containing a string literal, which in `#insert` form would need escaping.
    #code {
        greeting := "hi";
    }
    if greeting.count == 2 {
        n = n + 8;
    }

    // An empty body splices nothing and is legal.
    #code {
    }
    n = n + 16;

    // Control flow inside a body.
    #code {
        counted := 0;
        i := 0;
        while i < 4 {
            counted = counted + i;
            i = i + 1;
        }
    }
    if counted == 6 {
        n = n + 32;
    }

    // Every assertion: 63.
    if n == 63 {
        exit(0);
    }
    exit(1);
}
```

## What `#code` buys

The advantage over `#insert "…"` is real but narrow, and it is exactly two things:

- **No quoting or escaping.** Code that itself contains a string, or a nested insert, needs none.
  Escaping is what made a written nest unpleasant beyond a single line — it doubles the text at
  every level. Here `#code { greeting := "hi"; }` carries a plain string literal that survives the
  splice intact.
- **The body is parsed where it is written.** A syntax fault inside a `#code` block is an ordinary
  parse error at an ordinary position, not an offset into a string.

## It splices into the enclosing scope

Like `#insert`, a `#code` block's statements land in the **enclosing** scope, not a nested one.
This is the whole reason it reuses `#insert`'s lowering path rather than lowering the block it
already parsed: a block's statements go into a nested *name scope*, so a local the body declares
would be invisible afterwards. Here `seven`, `b`, `from_outer`, `greeting` and `counted` are all
declared inside `#code` blocks and read by real code after the block closes — which proves the
splice is genuinely in `main`'s scope. The block can also read an enclosing local (`outer`).

An **empty** `#code { }` block is legal and splices nothing — worth an assertion because
"expands to zero statements" and "was never expanded" were a real bug in the computed-insert wave,
distinguishable only by a field that had to be cleared.

## What `#code` is *not*

`#code` is **not** a `Code` *value*. There is no `Code` type, no pool variant, and `#code` is a
statement, not an expression (ADR-0080 §3). A quoted syntax tree is only worth representing once
something can inspect or transform it — and a value that can only be spliced is what a `string`
already is. When a macro eventually needs to read its argument, that will supersede this with the
real representation.

Braces are **required**. A braceless `#code n := 7;` would have to decide where the quoted region
ends, and "until the next `;`" cannot express two statements. The braceless form is a parse error,
covered by a `jr-syntax` test rather than the type-error corpus (which requires its files to
parse).

## Observing the result

The `n` accumulator sums to `63` when all six assertions pass. Each check adds a distinct power of
two, so the `exit` value encodes exactly which passed — making the result observable, so `jr run`
and `jr build` can be asserted to agree byte-for-byte.
