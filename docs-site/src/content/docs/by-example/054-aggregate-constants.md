---
title: Compile-time aggregates
description: "A #run that returns a struct or array becomes a compile-time aggregate constant — and such an aggregate may even hold strings."
sidebar:
  order: 54
---

A `#run` need not return a scalar. It can return a struct or an array, and the result becomes a
compile-time **aggregate constant** (ADR-0074). This is the prerequisite that reflection and `Any`
were really blocked on: a struct describing a type, returned from the compiler, *is* an aggregate
constant.

## Structs, arrays, and nesting

```jr
#import "Basic";

Point :: struct {
    x: s64;
    y: s64;
}

Nested :: struct {
    inner: Point;
    scale: s64;
}

/// Returns a struct, so its value must be interned as an aggregate.
make_point :: () -> Point {
    p: Point;
    p.x = 3;
    p.y = 4;
    return p;
}

/// Returns a fixed array.
make_sizes :: () -> [3]s64 {
    a: [3]s64;
    a[0] = 5;
    a[1] = 6;
    a[2] = 7;
    return a;
}

/// A struct holding a struct: the element is itself an aggregate.
make_nested :: () -> Nested {
    n: Nested;
    n.inner.x = 1;
    n.inner.y = 2;
    n.scale = 10;
    return n;
}

POINT :: #run make_point();
SIZES :: #run make_sizes();
NESTED :: #run make_nested();

main :: () {
    // A struct constant, read field by field.
    total := POINT.x + POINT.y;

    // An array constant, read by index.
    total = total + SIZES[0] + SIZES[1] + SIZES[2];

    // A nested aggregate, two levels deep.
    total = total + NESTED.inner.x + NESTED.inner.y + NESTED.scale;

    // And through a local copy, which exercises the whole aggregate rather than one field.
    p := POINT;
    total = total + p.x + p.y;

    // 3+4 + 5+6+7 + 1+2+10 + 3+4 = 45.
    exit(total);
}
```

### The representation is values, not bytes

The constant is stored as its **element values, in order** — deliberately not as the byte image
the VM happened to have. The constant pool is target-independent, so interning raw bytes would
bake one target's padding and pointer width into a shared table, and a cross-compile would then
read plausible wrong values rather than fail. Instead each engine turns the values into bytes
itself, at the point that knows the target: the VM writes them at each field offset, and the
native back end materialises a stack slot the same way it does for a string's `{data, count}`
pair. Two materialisations from one shared value is how the engines are kept honest, and the
`exit(45)` is what asserts they agree.

Nesting needs no special case — an element is an interned value like any other, so a struct
holding a struct recurses by construction. Reading `POINT.x` required giving an aggregate-valued
constant a *place*: a constant is an operand with no address, but a field projection needs one, so
the constant is spilled into a slot (the `p := POINT` copy exercises exactly this path).

## Aggregates may hold strings

The follow-on wave (ADR-0075 §1) established that a compile-time aggregate may hold a `string`.
A `Type_Info` carrying a type's *name* is exactly this shape, so reflection depended on it.

```jr
#import "Basic";

/// A string beside an integer: the ordering that catches a wrong offset.
Named :: struct {
    name: string;
    size: s64;
}

/// One level down, so the recursion has something to recurse into.
Inner :: struct {
    label: string;
    n: s64;
}

/// Two strings and a nested struct that itself holds one — three strings at three offsets.
Outer :: struct {
    head: string;
    in: Inner;
    tail: string;
}

mk_named :: () -> Named {
    n: Named;
    n.name = "s64";
    n.size = 8;
    return n;
}

mk_outer :: () -> Outer {
    o: Outer;
    o.head = "H";
    o.in.label = "mid";
    o.in.n = 5;
    o.tail = "T";
    return o;
}

/// An array of aggregates each holding a string.
mk_pair :: () -> [2]Named {
    a: [2]Named;
    a[0].name = "first";
    a[0].size = 1;
    a[1].name = "second";
    a[1].size = 2;
    return a;
}

NAMED :: #run mk_named();
OUTER :: #run mk_outer();
PAIR :: #run mk_pair();

/// Compares two strings by contents.
same :: (a: string, b: string) -> bool {
    if a.count != b.count {
        return false;
    }
    i := 0;
    while i < a.count {
        p := a.data + i;
        q := b.data + i;
        if p.* != q.* {
            return false;
        }
        i = i + 1;
    }
    return true;
}

main :: () {
    n := 0;

    if same(NAMED.name, "s64") { n = n + 1; }
    if NAMED.size == 8 { n = n + 2; }

    if same(OUTER.head, "H") { n = n + 4; }
    if same(OUTER.tail, "T") { n = n + 8; }

    if same(OUTER.in.label, "mid") { n = n + 16; }
    if OUTER.in.n == 5 { n = n + 32; }

    if same(PAIR[0].name, "first") { n = n + 64; }
    if same(PAIR[1].name, "second") { n = n + 128; }
    if PAIR[0].size + PAIR[1].size == 3 { n = n + 256; }

    // Every assertion: 511.
    if n == 511 {
        exit(0);
    }
    exit(1);
}
```

### Why the string case was a real problem

The same struct had always worked at *run* time — the gap was comptime-only. The compile-time
reducer copied its result out as a flat byte image, and a `string` field's bytes are a
`{data, count}` pair pointing *into the VM's memory*. Interning happens after the VM is dropped,
so by then the pointer dangled. Refusing was correct; the fix was to stop the image being flat.

The reduced aggregate now holds a **tree of reduced elements** rather than bytes, walked while the
VM is still alive — so a string element is read out at the one moment it can still be read. That
is not a new mechanism: the reducer already did exactly this for a *top-level* string. The
decision was only to apply it one level down.

The assertions place three strings at three different offsets and one string beside an `s64`
(where a wrong offset would read the integer's bytes), and an array of structs each holding a
string (where a wrong stride would repeat one element). The sum is `511`, so a silently dropped or
duplicated string changes the number. The native engine has to materialise a string constant
nested inside an aggregate, which is the part that could fail independently of the VM — so the
byte-for-byte agreement between `jr run` and `jr build` is a real check.

## What is absent

- A struct or array **literal** (`P.{1, 2}`, `[1, 2, 3]`) still does not parse. This feature gives
  a `#run` *result* somewhere to live; it does not add a way to write an aggregate directly.
- A **union** constant is refused: untagged storage makes "which field is valid" unanswerable
  (ADR-0074 §4). That refusal lives in the type-error corpus.
