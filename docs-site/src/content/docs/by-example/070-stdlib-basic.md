---
title: "Basic: print, write, exit"
description: The bottom of the Jairs standard library — syscalls, allocation, temporary storage, reflection, and printing, all written in Jairs.
sidebar:
  order: 70
---

`Basic` is the bottom of the Jairs standard library, and it is written **in Jairs**, not in the
compiler. That is what forces `#foreign` into the language early: the bottom of a standard library is a
syscall, and there is no other way to express it. Everything here must be writable in the base Jairs
subset — a constraint that genuinely bites, most visibly in how integer printing is done.

## The foreign floor

`Basic` binds libc and declares the primitives every program stands on:

```jr
libc :: #system_library "c";

/// POSIX write(2): writes `count` bytes from `buf` to the file descriptor `fd`.
write :: (fd: s64, buf: *u8, count: s64) -> s64 #foreign libc "write";

/// Terminates the process with `status`.
exit :: (status: s64) #foreign libc "exit";

/// Allocates `size` bytes and returns a pointer to them, or `null` on failure.
malloc :: (size: s64) -> *u8 #foreign libc "malloc";

/// Releases memory obtained from malloc. Passing null is defined (a no-op).
free :: (p: *u8) #foreign libc "free";
```

A Jairs `string` is `{data: *u8, count: s64}` and is **not** NUL-terminated — which is exactly the shape
`write` wants, a pointer and a length, so no conversion or temporary storage is needed. `exit` gives a
program a way out that does not depend on `main` returning. `malloc` returns `null` on failure and cannot
be called at compile time (a host pointer read through the VM's address space would be a plausible wrong
value).

## Printing

```jr
STDOUT :: 1;
STDERR :: 2;

print :: (fmt: string, args: ..Any) -> s64 { … }        // formats to stdout, returns bytes written
print_error :: (fmt: string, args: ..Any) -> s64 { … }  // the same, to stderr
print_line :: (fmt: string, args: ..Any) -> s64 { … }   // the same, with a newline appended
format :: (buffer: []u8, fmt: string, args: ..Any) -> s64 { … }  // into a caller's buffer
```

All three take a **format string and a variadic** `..Any` (ADR-0189, over the variadic machinery of
ADR-0138/0139/0141), so `print_line("x = %", x)` works and a call with no trailing arguments packs an
empty variadic — which is why every older call site that passed one bare `string` still compiles.

A `%` takes the next argument and renders it by reading its `Type_Info`. A wrong argument count is not a
compile error and not a trap: too few renders the placeholder as `%!(MISSING)` and too many appends
`%!(EXTRA a, b)`, both of them Go's spelling. That is what a diagnostic looks like inside a procedure
which cannot fail — visible in the output, and impossible to miss.

`print_error` is a **second name** rather than `print`'s `to_standard_error := false` parameter, which
is what Jai's takes. The reason is Jairs' own: a defaulted parameter after a variadic can never be
reached, because every trailing argument is packed into the variadic (ADR-0139 §1).

```jr
// examples/08-print-formatted.jr
#import "Basic";

Point :: struct {
    x: s64;
    y: s64;
}

main :: () {
    // `%` takes the next argument and asks it what it is. There is no `%d` or `%s`, because every
    // argument already carries its own `Type_Info` — a type letter would be information the callee
    // has and the caller can get wrong.
    print("int %, float %, bool %, string %\n", 42, 1.5, true, "text");

    // Every integer width, signed and unsigned. Including the two a naive renderer gets wrong:
    // `S64_MIN` cannot be negated, and `U64_MAX` does not fit in an `s64`.
    biggest: u64 = 18446744073709551615;
    print("smallest s64 %\n", -9223372036854775807 - 1);
    print("largest u64  %\n", biggest);

    // A struct prints one level deep, by field name.
    p: Point;
    p.x = 3;
    p.y = 4;
    print("point %\n", p);

    // `%%` is a literal percent.
    print("100%% covered\n");

    // A wrong argument count is not an error. `print` has nowhere to return one to, so it says so in
    // the output where you are already looking.
    print("missing %\n");
    print("spare\n", 9);

    // The return value is the byte count, so output can be measured.
    written := print("counted\n");
    print_line("that line was % bytes", written);

    exit(0);
}
```

A struct prints one level deep, by field name; an enum member prints by **name** rather than as an
ordinal, which is what `Type_Info.members` is for. `S64_MIN` and `U64_MAX` are in that program on
purpose: they are the two values a naive renderer gets wrong, and one of them is what the old
`print_int` trapped on.

What is worth knowing about the implementation, because both facts are caller-visible:

- **Output is buffered** through a file-scope global and reaches `write` once per call rather than once
  per byte. The old `print_int` cost one syscall *per digit*.
- **It is therefore not thread-safe.** Two threads printing at once interleave inside one buffer. Jai's
  `print` uses the context's temporary storage and so is per-thread; matching that needs `#add_context`,
  which is <span class="jairs-status absent">absent</span>. Until then a threaded program prints from one
  thread.

A float prints to nine rounded fraction digits rather than shortest-round-trip: a correct `dtoa` (Ryū)
is <span class="jairs-status absent">absent</span>, and it is the same missing algorithm that stops
`modules/JSON` serialising.

`print_int :: (n: s64)` still exists, as one line — `print("%", n)` — kept because programs call it.
The historical version is worth a sentence for the reason it is gone: it printed digits by *recursion*,
one stack frame per digit, because the base subset had no buffer to format into; and it **trapped** on
the most negative `s64`, which it negated to handle the sign. That was the first value anybody tested.

## Temporary storage

`Basic` also provides a per-context bump-allocated scratch arena (ADR-0065):

```jr
TEMP_REGION_SIZE :: 65536;

/// Allocates `n` bytes from the per-context temporary-storage arena.
talloc :: (n: s64) -> *u8 { … }

/// Rewinds the temporary-storage cursor, freeing everything talloc handed out at once.
reset_temporary_storage :: () { … }
```

`talloc` bumps a cursor into a lazily-`malloc`'d 64 KiB region and returns `null` when full or on a
failed allocation; `reset_temporary_storage` rewinds the cursor to 0 (it does *not* release the region —
reuse is the point of an arena). Because `talloc` reads `context`, it travels with the call: a callee's
`talloc` uses its caller's arena.

## Reflection

Two types are declared here **in Jairs** rather than in the compiler, so a program can *name* them:
`Type_Info` (what `type_info(T)` returns) and `Any` (a value carried with its type). The compiler
*validates* their fields on lookup — editing them is a diagnostic naming the mismatch, not a silent read
at a wrong offset.

```jr
Type_Info :: struct {
    id: s64;                       // canonical identity — an opaque token, not a number to do arithmetic on
    kind: Type_Info_Kind;          // which shape this type is (an enum, for exhaustive switch)
    name: string;                  // the type's source name, or a builtin's spelling ("s64")
    size: s64;                     // runtime size in bytes
    alignment: s64;                // runtime alignment in bytes
    count: s64;                    // a struct's field count, or an array's length; 0 otherwise
    element: s64;                  // an array's element or a pointer's pointee, as a type id; 0 otherwise
    fields: []Type_Info_Field;     // every field of a struct, union or variant, in declaration order
    signed: bool;                  // whether an integer type is signed; false for every other kind
    members: []Type_Info_Member;   // an enum's members, in declaration order
    element_size: s64;             // one element's size for an array, view or dynamic array; 0 otherwise
}

Any :: struct {
    type: *Type_Info;         // what `data` points at
    data: *u8;                // the value itself, erased — read it back with any_as rather than casting
}
```

`fields` and `members` are **views into a read-only table the compiler emitted** (ADR-0152, ADR-0193),
so they are valid for the life of the program and must not be written through. Both are empty for a kind
that has neither, and empty is a real answer rather than a sentinel: a scalar has no fields, and an enum
is the only shape whose values have names. `members` is what lets `print` show `Colour.BLUE` rather than
`2` — before that table existed, an ordinal was the most anything could say.

Two fields exist for reasons worth knowing, because each closed a real defect. `signed` is what makes
printing an integer correct: width comes from `size`, and without a sign flag a caller had to read the
first byte of `name` and hope it was `s` or `u` — which works only because a builtin cannot be aliased
at file scope, and would print a large `u64` as negative the day aliasing started working. And
`element_size` exists because `element` is an opaque **id**, so nothing could compute a *stride*: a
fixed array escaped that by arithmetic (`size / count`) and a view could not, because a view's `size` is
its header's.

What remains <span class="jairs-status absent">absent</span> is following an `element` or a field's `ty`
**back to a `Type_Info`**: both are ids, and ADR-0077 §1 makes an id opaque. The visible consequence is
that a struct prints **one level deep** — `{i = .., xs = ..}` for a struct holding a struct and an array,
verified by running it — because the renderer can see that a field exists and not what shape it is.
A structural type's `name` is *not* affected: `type_info(type_of(xs)).name` is `[3]s64`, a view's is
`[]s64` and a pointer's is `*s64`, because those spellings are built where the type is interned rather
than by following an id. Lifting the nesting limit is one change — a `*Type_Info` per type — and it is
recorded rather than half-built.
