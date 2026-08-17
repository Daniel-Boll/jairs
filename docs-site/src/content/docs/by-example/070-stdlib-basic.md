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

print :: (s: string) { … }           // writes a string to stdout
print_error :: (s: string) { … }     // writes a string to stderr
print_line :: (s: string) { … }      // writes a string followed by a newline
print_int :: (n: s64) { … }          // writes a number in decimal
```

`print_int` is worth a look because of what the base subset lacks. It has **no buffer to format into** —
fixed-size arrays arrive in a later wave — so it prints digits by *recursion*: a private `print_digits`
divides first, prints the high digits, then this digit on the way back out, one stack frame per digit (at
most 20 for a 64-bit number). When arrays land this becomes a loop over a `[20]u8` and the recursion goes
away. It is honest about one limit: `print_int` negates a negative to handle the sign, and negating the
most negative `s64` overflows — which is a trap, not a wrap — so it traps on that value rather than
printing a wrong one.

```jr
#import "Basic";

main :: () {
    // Zero, which the `n >= 10` recursion base case reaches without recursing at all.
    print_int(0);
    print("\n");

    print_int(7);
    print("\n");
    print_int(42);
    print("\n");
    print_int(1234567890);
    print("\n");

    // The sign, handled in `print_int` rather than in `print_digits`.
    print_int(-7);
    print("\n");
    print_int(-1234567890);
    print("\n");

    // `s64` max: twenty digits, which is `print_digits`' deepest recursion.
    print_int(9223372036854775807);
    print("\n");

    // `print_error` writes to STDERR, which the harness compares separately — so a swapped file
    // descriptor is caught rather than washing out into stdout.
    print_error("to stderr\n");

    // A checksum of the values above, so a wrong digit changes the exit code as well as the output.
    // 7 + 42 - 7 = 42.
    exit(7 + 42 - 7);
}
```

This program checks **stdout**, not just the exit code: the differential harness compares output between
the two engines, and the digits are where a recursion emitting them in the wrong order, an off-by-one in
the `+ 48` byte arithmetic, or a lost sign would show. `print_error` writes to STDERR, which the harness
compares *separately*, so a swapped file descriptor is caught rather than washing out into stdout. The
exit code (42) is a checksum of the printed values, so a wrong digit is caught twice.

`print_int` was worth a corpus file for a pointed reason: an audit found nothing in the whole tree
*called* it or `print_error` — they appeared only in their own definitions and in comments — so both
engines could have broken them with every gate green. A capability with no program that runs it is the
project's named failure shape.

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
    id: s64;                  // canonical identity — an opaque token, not a number to do arithmetic on
    kind: Type_Info_Kind;     // which shape this type is (an enum, for exhaustive switch)
    name: string;             // the type's source name, or a builtin's spelling ("s64")
    size: s64;                // runtime size in bytes
    alignment: s64;           // runtime alignment in bytes
    count: s64;               // a struct's field count, or an array's length; 0 otherwise
    element: s64;             // an array's element or a pointer's pointee, as a type id; 0 otherwise
}

Any :: struct {
    type: *Type_Info;         // what `data` points at
    data: *u8;                // the value itself, erased — read it back with any_as rather than casting
}
```

`Type_Info` is deliberately **without per-kind detail** — a struct's field list or a procedure's
signature would each be a variable-length member raising a memory-ownership question of its own. `size`
and `alignment` are enough to be useful and the shape extends by adding fields.
