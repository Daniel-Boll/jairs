---
title: Procedures
description: Parameters, multiple return values, named and default arguments, and procedures as values.
sidebar:
  order: 4
---

A procedure is declared with the constant form: a name, `::`, a parameter list, an optional
return type, and a body.

```jr
add :: (a: s64, b: s64) -> s64 {
    return a + b;
}

greet :: () {              // no arrow means no return value
    print("hi\n");
}
```

The parameter list is `(name: T, …)`. The return type follows `->`; omit the whole `-> T`
for a procedure that returns nothing.

## Multiple return values

A procedure can return several values, written as a parenthesised list. This is the
foundation of Jairs' error handling: instead of throwing, a procedure returns its result
*and* a flag.

```jr
divide :: (a: s64, b: s64) -> (s64, bool) {
    if b == 0 {
        return 0, false;       // couldn't divide
    }
    return a / b, true;
}

main :: () {
    q, ok := divide(10, 2);    // bind both results
    if ok {
        print_int(q);
    }
    _, valid := divide(1, 0);  // `_` discards a result you don't need
}
```

The caller must name every result (or discard it with `_`). There is a planned `#must`
attribute that will make *ignoring* the flag a compile error — the other half of Jairs' error
model — but it is <span class="jairs-status absent">absent</span> today and owed its own
design decision.

## Named and default arguments

Arguments can be passed by name, in any order, and a parameter can have a **literal** default:

```jr
box :: (width: s64, height: s64 = 1, label: string = "box") -> s64 {
    return width * height;
}

main :: () {
    a := box(4);                       // height and label default
    b := box(4, height = 3);           // one named
    c := box(label = "x", width = 2);  // all named, reordered
}
```

Defaults must be literals for now; a non-literal default, a named argument on a *cross-file*
call, and a named argument inside a `#run` are each <span class="jairs-status absent">absent</span>.

## Aggregate returns

A procedure can return a whole struct, not just a scalar:

```jr
Point :: struct { x: s64; y: s64; }

make_point :: (x: s64, y: s64) -> Point {
    p: Point;
    p.x = x;
    p.y = y;
    return p;
}
```

Under the hood the native back end returns the aggregate through a caller-allocated pointer,
uniformly by size — a register fast path for small structs is a later optimisation. From your
side it just works, in both engines.

## Procedures as values

A procedure is a value, so you can store it in a variable, pass it as a parameter, or keep it
in a struct field, and then call through it:

```jr
add :: (a: s64, b: s64) -> s64 { return a + b; }

apply :: (f: (s64, s64) -> s64, a: s64, b: s64) -> s64 {
    return f(a, b);        // call through the parameter
}

main :: () {
    op := add;             // op : (s64, s64) -> s64
    r := apply(op, 2, 3);  // r == 5
}
```

The type of a procedure value is written `(param-types) -> return-type`, or `(T)` with no
arrow for one that returns nothing. This is what makes the allocator protocol and callbacks
like `Sort.sort(xs, less)` possible.

Some procedure-value cases are still <span class="jairs-status absent">absent</span>: a
*cross-file* or `#foreign` procedure used as a value, comparing or printing a procedure
value, and a `#c_call` procedure-pointer type.

## The implicit context

Every ordinary Jairs procedure receives a hidden trailing parameter — the `context` — passed
by pointer. You never write it in the parameter list, but it is how allocation travels down a
call chain without every procedure taking an allocator argument. A `#c_call` procedure opts
out and gets none. The context has its own chapter, [Memory](/language/memory/).

Next: [Control flow](/language/control-flow/) — `if`, `while`, `for`, `switch`, `defer`, and
labelled loops.
