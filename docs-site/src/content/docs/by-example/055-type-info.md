---
title: "type_info: reflection"
description: What the compiler knows about a type, returned as an ordinary value a program can read.
sidebar:
  order: 55
---

`type_info(T)` (ADR-0075 §2) returns what the compiler knows about a type, as a value a program
can read: its size, alignment, name, kind, and per-kind details like a struct's field count. It is
the first half of runtime type information — `Any` is the other.

```jr
#import "Basic";

/// Two `s64` fields: size 16, alignment 8.
Point :: struct {
    x: s64;
    y: s64;
}

/// An enum, so the `ENUM` kind has something to report.
Colour :: enum {
    RED;
    GREEN;
    BLUE;
}

/// Compares two strings by contents, since `name` is text.
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

    // A declared struct: name, size and alignment from the layout the rest of the compiler uses.
    p := type_info(Point);
    if same(p.name, "Point") { n = n + 1; }
    if p.size == 16 { n = n + 2; }
    if p.alignment == 8 { n = n + 4; }
    if p.kind == Type_Info_Kind.STRUCT { n = n + 8; }

    // A builtin, which has no declaration at all.
    i := type_info(s64);
    if same(i.name, "s64") { n = n + 16; }
    if i.size == 8 { n = n + 32; }

    // An enum, so `kind` distinguishes two named types.
    c := type_info(Colour);
    if c.kind == Type_Info_Kind.ENUM { n = n + 64; }

    // A `Type_Info` is an ordinary value: it copies, and a copy reads the same.
    q := p;
    if q.size == 16 { n = n + 128; }

    // Per-kind detail: a struct's field count, and a scalar's absence of one.
    if p.count == 2 { n = n + 256; }
    if i.count == 0 { n = n + 512; }

    // Every assertion: 1023.
    if n == 1023 {
        exit(0);
    }
    exit(1);
}
```

## `Type_Info` is declared in Jairs, not in the compiler

The describing struct `Type_Info` lives in `modules/Basic`, written in Jairs — not baked into the
compiler. The reason is that it must be **spellable**: a program that reflects needs to write
`info: Type_Info` and pass one around. Probing found that *no* compiler-declared type is spellable
(`t: Type;` and `c: Context;` both fail), because such a type has no declaration for name
resolution to find. Declaring `Type_Info` in Jairs makes it an ordinary nominal struct, so field
access, layout, and pointers all work with no new machinery.

The price is that the compiler depends on a declaration it does not own — and that price is paid
honestly. The `type_info` lookup **validates** `Basic`'s `Type_Info` field names, types and order.
Editing that struct produces a diagnostic naming the mismatch rather than a read of whatever now
sits at the old offset. A wrong offset would be a silent wrong value, this project's named failure
mode; a refusal is not.

## It returns a value, not a pointer

`type_info` returns a `Type_Info` **by value**. An earlier design said pointer, and the MIR
verifier caught the problem within minutes: the folded result is an aggregate *constant*, which
has no address, so `info := type_info(Point)` reported a deref of a non-pointer. A `*Type_Info`
would need the pointee to live somewhere — a stack slot dangles on return, and per-type static
data was a storage decision this wave declined to make. By value needs neither, since an aggregate
return already works. That is why the assertion `q := p; q.size == 16` matters: it would fail if a
`Type_Info` were a pointer into a dead frame.

## Builtins need no declaration

`type_info(s64)` works even though `s64` has no declaration. The builtin names are ordinary
identifiers, resolved through the same path a type annotation uses — which is what makes
`type_info(s64).size` and `x: s64` agree by construction rather than through a second table.
Notice that `s64`'s field `count` is `0`: that is a real answer, not a sentinel.

## Observing the result

Ten assertions each add a distinct power of two, summing to `1023`. The `exit` encodes precisely
which held, so `jr run` and `jr build` can be asserted to agree byte-for-byte on a reflected
program — including the fixed per-kind facts `count` (field count) and, elsewhere, `element`.

## What is still absent

The variable-length field **list** is not here yet: exposing a struct's fields one by one wants a
memory-ownership decision of its own (ADR-0078 §4). Reflection currently gives you the fixed facts
about a type, not a walk over its members.
