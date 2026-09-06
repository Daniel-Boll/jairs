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

## Fields, members, and a view's stride

```jr
#import "Basic";

Point :: struct {
    x: s64;
    y: s64;
}

Colour :: enum {
    RED;
    GREEN;
    BLUE;
}

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

    // Every field, in declaration order (ADR-0152).
    p := type_info(Point);
    if p.fields.count == 2 { n = n + 1; }
    if same(p.fields[0].name, "x") && p.fields[0].offset == 0 { n = n + 2; }
    if same(p.fields[1].name, "y") && p.fields[1].offset == 8 { n = n + 4; }

    // Every member of an enum, in declaration order (ADR-0193 §1).
    c := type_info(Colour);
    if c.members.count == 3 { n = n + 8; }
    if same(c.members[1].name, "GREEN") && c.members[1].value == 1 { n = n + 16; }

    // A view's element size, the stride `element` alone cannot give (ADR-0193 §2).
    xs: [4]s64;
    view := xs[];
    vi := type_info(type_of(view));
    if vi.element_size == 8 { n = n + 32; }

    // Whether an integer is signed (ADR-0189 §3).
    if type_info(s64).signed && !type_info(u64).signed { n = n + 64; }

    // Every assertion: 127.
    if n == 127 {
        exit(0);
    }
    exit(1);
}
```

`Type_Info.fields` (ADR-0152) is a view over every field of a struct, union or variant, in
declaration order, each carrying its own `name`, `ty` and byte `offset`. It points into a
read-only table the compiler emits once per type, so it costs nothing to read repeatedly and must
not be written through. `Type_Info.members` (ADR-0193 §1) is the same idea for an enum: its
members' source names, in declaration order, which is what lets `print` show `Colour.GREEN`
instead of `1`. `element_size` (ADR-0193 §2) is the size of one element of an array, view or
dynamic array — needed because `element` is only a type *id* and gives no stride, so without it a
view's elements were unreachable even though its header holds `data` and `count`. `signed`
(ADR-0189 §3) is meaningful only where `kind` is `INTEGER`, and is what lets a formatter print an
unsigned `u64` above 2^63 without guessing from the first letter of its name.

## `type_of` and a pointer type argument

```jr
#import "Basic";

Point :: struct {
    x: s64;
    y: s64;
}

// A polymorphic procedure: the parameter's type has no spelling at the call site,
// so nothing else can name it.
describe :: (v: $T) -> s64 {
    return size_of(type_of(v));
}

main :: () {
    n := 0;

    p: Point;
    ptr := *p;

    // `type_of(x)` is the inverse of `size_of(T)`: it takes a value and gives a type (ADR-0192).
    if type_info(type_of(p)).id == type_info(Point).id { n = n + 1; }

    // Composes with a pointer type argument (ADR-0191): `type_of(ptr)` is `*Point`.
    if type_info(type_of(ptr)).kind == Type_Info_Kind.POINTER { n = n + 2; }

    // A pointer type read directly, the same way any other type argument is.
    if size_of(*Point) == size_of(*u8) { n = n + 4; }

    // Through the polymorphic parameter, where the type has no other spelling.
    if describe(p) == 16 { n = n + 8; }

    // Every assertion: 15.
    if n == 15 {
        exit(0);
    }
    exit(1);
}
```

`type_of(x)` (ADR-0192) is the inverse of every other type argument: `size_of(T)` takes a type and
gives a number, `type_of(x)` takes a value and gives a type. Its main use is inside a polymorphic
procedure, where the parameter's type has no spelling at the call site and nothing else can name
it — `describe` above reads `$T`'s size through `type_of(v)` with no other way to reach it. It
composes with a **pointer type as an intrinsic's type argument** (ADR-0191): `*Point` is not a
`TypeRef` here but a unary address-of applied to a name, so `any_as`, `size_of` and `type_info`
all read it the same way, and it recurses (`**s64` works too).

## What is still absent

`element` and a field's `ty` are opaque type **ids**, not `*Type_Info` pointers, so nothing can
follow one back to a `Type_Info` — a pointer to the element's own info would need that info built
and stored somewhere, and an id needs nothing built. That is the one real gap left: a nested
field cannot be described, only compared against an id the caller already holds. `print` runs
into exactly this — a struct nested inside another prints as `{inner = ..}` rather than recursing
into `inner`'s own fields, because `format_aggregate` has no way to turn `ty` into a `Type_Info`
for the field it is printing.

A structural type's `name` — an array's, a pointer's, a view's, a dynamic array's — is a full
recursive spelling like `[3]s64` or `*Point` (ADR-0193 §3), not the lowercased kind. The one shape
that still falls back to its lowercased kind is a **procedure type**: `type_info(type_of(add)).name`
reads `"procedure"` rather than a rendered signature, because nothing has needed one yet.
