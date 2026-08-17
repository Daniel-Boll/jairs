---
title: "#expand macros"
description: "A procedure marked #expand is a macro whose body splices into the caller's scope instead of being called."
sidebar:
  order: 60
---

A procedure marked `#expand` is a **macro**: a call to it does not *call* the body, it **splices the
body into the caller's scope** (ADR-0090). The statements land where the call was written, so they see
and modify the caller's own locals — deliberately unhygienic, matching Jai and matching what `#insert`
already does. That is exactly what makes a macro useful for a custom loop or an in-place mutation a
call could never perform.

## Declaring a macro

A `#expand` procedure parses, formats, lowers and type-checks like any other declaration. It can carry
a return type, several parameters, and other attributes beside it, in either order.

```jr
#import "Basic";

/// The canonical macro: doubles its argument. Its body will be spliced at each call site.
double :: (x: s64) -> s64 #expand {
    return x * 2;
}

/// `#expand` beside `#no_abc`, in that order.
indexed :: (n: s64) -> s64 #expand #no_abc {
    return n + 1;
}

/// And in the other order, because the attribute loop takes either.
indexed_too :: (n: s64) -> s64 #no_abc #expand {
    return n + 2;
}

/// Several parameters, all used — a real body rather than a trivial shape.
combine :: (a: s64, b: s64, c: s64) -> s64 #expand {
    return a * 100 + b * 10 + c;
}

main :: () {
    n := 9;
    exit(n);
}
```

The attribute loop takes `#expand` and `#no_abc` in **any** order — the ordering rule was deliberately
never added. Notice that this program declares macros but does not *call* any of them; the point of the
example above is that declaring one type-checks cleanly, which before the feature existed it did not.

### Why the refusal shipped with the surface

Before `#expand` was understood, it parsed and was then **silently ignored** — a macro behaved like an
ordinary procedure, so `double(21)` returned 42 by *calling* rather than *splicing*. A directive that is
accepted and does nothing is worse than one that is rejected, because nothing tells the writer their
intent did not land. So the first landing of the surface *refuses* a call to a macro; the splice itself
arrives next.

## The splice: a call runs the body in place

Once the splice landed, a macro call *runs*: its body is spliced into the caller, so it can touch the
caller's locals. That is the whole point — `add_to_total(10)` below modifies `main`'s `total`, which no
call could do.

```jr
#import "Basic";

/// A void macro: no return type, so it produces no value. Its body reaches the *caller's* `total`, which
/// is what makes it a macro rather than a procedure.
add_to_total :: (x: s64) #expand {
    total = total + x;
}

/// A value macro: its tail `return` becomes an assignment to the call's result local.
double :: (x: s64) -> s64 #expand {
    return x * 2;
}

main :: () {
    total := 0;

    // Statement position, twice: each splices into `main` and modifies `main`'s own local.
    add_to_total(10);
    add_to_total(6);
    // total == 16

    // Expression position, through the generated result local.
    a := double(21);
    // a == 42

    // An argument that is an *expression*: the prelude binds its value, so `1 + 2` is evaluated once.
    b := double(1 + 2);
    // b == 6

    // Two macro calls in one expression — two result locals, no collision.
    c := double(4) + double(5);
    // c == 8 + 10 == 18

    // 16 + 42 + 6 + 18 == 82, plus 14 to make the total distinctive.
    exit(total + a + b + c + 14);
}
```

### How the splice is built

A macro is not a call, and it reuses the same splice machinery `#insert` already had. Each call
generates text — a `name := arg;` prelude that binds each parameter to its argument, then the body — and
hands it to the expander. The prelude is why **each argument is evaluated exactly once**: substituting
the argument at every use of a parameter would re-evaluate a side-effecting argument per use, a wrong
answer rather than a slow one. That is why `double(1 + 2)` binds the *value* `3` rather than pasting the
text `1 + 2` at each occurrence.

For **expression** position a result local is generated. `exit(double(21))` becomes, in effect,
`__macro_0 := 0; x := 21; __macro_0 = x * 2; exit(__macro_0);`. So one mechanism serves both statement
and expression positions, and a `return` in **tail** position means "assign the result". Two macro calls
in one expression get two independent result locals, so `double(4) + double(5)` cannot collide.

An **early** `return` in a macro body is refused rather than silently falling through, and a void macro
used in expression position is refused too.

The exit code is the observable checksum: the assertions above sum to 96 (`16 + 42 + 6 + 18 + 14`), and
both engines must agree byte-for-byte. A splice that ran a body twice, bound an argument wrongly, or
leaked a result local between calls would each change the total.
