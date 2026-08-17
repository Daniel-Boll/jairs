---
title: if & while
description: Braces required but parentheses not; single-statement bodies; `while`, `break`, `continue`, and nested block scopes.
sidebar:
  order: 14
---

Jairs has two control constructs in this slice: `if`/`else` and `while`. Both share a rule that
distinguishes the language from C — the condition needs **no** parentheses, but the body
normally needs braces.

## `if` and `else`

```jr
classify :: (n: s64) -> s64 {
    // Braces are required; a parenthesised condition is not.
    if n < 0 {
        return 0 - 1;
    } else if n == 0 {
        return 0;
    } else {
        return 1;
    }
}

single_statement :: (n: s64) -> s64 {
    // A single statement may follow the condition without braces.
    if n > 0 return n;
    return 0;
}
```

`if n < 0 { ... }` has no parentheses around the condition, and the branches chain with
`else if` and `else` as you would expect. Braces are the norm — but there is one relaxation:
a **single statement** may follow the condition without braces, as in `if n > 0 return n;`.

## `while`, `break`, and `continue`

```jr
sum_to :: (n: s64) -> s64 {
    total := 0;
    i := 1;
    while i <= n {
        total = total + i;
        i = i + 1;
    }
    return total;
}

with_break :: () -> s64 {
    i := 0;
    while true {
        i = i + 1;
        if i == 10 {
            break;
        }
        if i == 3 {
            continue;
        }
    }
    return i;
}
```

`while` follows the same shape — condition without parentheses, braced body. `sum_to`
accumulates a running total, and `with_break` shows the two loop-control statements: `break`
leaves the loop entirely, `continue` jumps to the next iteration. `while true { ... }` is the
idiomatic infinite loop you exit with `break`.

## Block scopes

```jr
main :: () {
    outer := 1;
    {
        inner := 2;
        outer = outer + inner;
        {
            // Shadowing an outer name is permitted; wave W2 decides
            // whether to warn.
            inner := 3;
            outer = outer + inner;
        }
    }
}
```

A bare `{ ... }` introduces a nested scope. A name declared inside is visible only within that
block, and an inner block may **shadow** a name from an outer one — the inner `inner := 3` is a
new variable distinct from the outer `inner := 2`. Shadowing is permitted here; whether the
compiler warns about it is a decision deferred to the wave labelled W2.

See also [Book I — The Jairs Language](/language/introduction/).
