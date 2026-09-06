---
title: String
description: Byte-wise operations on a string — a non-allocating half that only inspects, and an allocating half that produces new strings the caller frees.
sidebar:
  order: 71
---

The `String` module operates byte-wise on a `string`, which is `{data: *u8, count: s64}` — bytes, with no
notion of encoding. It exists because comparing two strings for equal *contents* needs a byte loop, and a
byte loop is a library's job, not an operator's: `==` on two strings is deliberately refused, since "same
storage" and "same contents" are both plausible readings and picking one silently would make the other a
bug that looks like working code. `String.equal` is the real answer that refusal points at.

The module splits cleanly into a **non-allocating half** (inspection only) and an **allocating half**
(each result is a new string the caller frees). It imports nothing at all — even its allocation reaches
through `context`, which is a language facility rather than a library name.

## The non-allocating API

```jr
/// The byte at `index`, or -1 when the index is out of range.
byte_at :: (s: string, index: s64) -> s64

/// Whether `s` has no bytes.
is_empty :: (s: string) -> bool

/// Whether `a` and `b` have the same bytes.
equal :: (a: string, b: string) -> bool

/// Negative when `a` sorts before `b`, zero when equal, positive when after (byte order).
compare :: (a: string, b: string) -> s64

/// Whether `s` begins with `prefix`.
starts_with :: (s: string, prefix: string) -> bool

/// Whether `s` ends with `suffix`.
ends_with :: (s: string, suffix: string) -> bool

/// The index of the first occurrence of `needle` in `haystack`, or -1.
find :: (haystack: string, needle: string) -> s64

/// Whether `needle` occurs anywhere in `haystack`.
contains :: (haystack: string, needle: string) -> bool
```

`byte_at` exists because `s.data[i]` **does not compile** — `data` is a `*u8` and a pointer is not
indexable — so reading a byte takes `(s.data + i).*` and a cast to `s64`. `byte_at` is that expression
with a name, honest about being a workaround until pointer indexing arrives. Its out-of-range answer is
**-1 rather than a trap**, unlike an out-of-range array index: an array's bound is known to the compiler
and indexing past it is a *mistake*, while scanning a string until the bytes run out is an ordinary way
to write a loop.

```jr
#import "Basic";
#import "String";

main :: () {
    n := 0;

    // `equal` — the reason this module exists. Identical, differing at a byte, and differing in length.
    if equal("abc", "abc") && !equal("abc", "abd") && !equal("abc", "ab") {
        n = n + 1;
    }

    // `compare`'s outcomes, the last being the **prefix** case a length-only comparison gets wrong.
    if compare("a", "b") < 0 && compare("b", "a") > 0 && compare("abc", "abc") == 0 {
        if compare("ab", "abc") < 0 && compare("abc", "ab") > 0 {
            n = n + 2;
        }
    }

    // `starts_with`, including the empty pattern (true) and an over-long one (false).
    if starts_with("hello", "he") && starts_with("hello", "") && !starts_with("he", "hello") {
        n = n + 4;
    }

    // `ends_with`, reading from a different starting offset than `starts_with`.
    if ends_with("hello", "lo") && ends_with("hello", "") && !ends_with("lo", "hello") {
        n = n + 8;
    }

    // `find` at the start, in the middle, and at the end.
    if find("hello", "he") == 0 && find("hello", "ll") == 2 && find("hello", "lo") == 3 {
        n = n + 16;
    }

    // `find` when absent (-1) and with an empty needle (0, since every string starts with nothing).
    if find("hello", "xyz") == -1 && find("hello", "") == 0 {
        n = n + 32;
    }

    // `contains` must agree with `find`.
    if contains("hello", "ell") && !contains("hello", "xyz") {
        n = n + 64;
    }

    // `byte_at` in range and out of it — -1 rather than a trap. "abc" is 97, 98, 99. And `is_empty`.
    if byte_at("abc", 1) == 98 && byte_at("abc", 3) == -1 && byte_at("abc", -1) == -1 {
        if is_empty("") && !is_empty("x") {
            n = n + 128;
        }
    }

    exit(n);
}
```

The exit code is **255** — eight independent groups, each contributing one bit. Negative cases are folded
in with `&&` rather than added separately, so a wrong answer *clears* a bit rather than pushing the total
past 255 where it could wrap and coincide with a passing value. `find` returns `-1` rather than a second
return value because a caller almost always feeds the result straight into `if find(h, n) >= 0`, and the
sentinel is outside the domain of valid indices so it cannot be mistaken for one.

## The allocating API

Each of these produces a **new** string and the **caller frees** it with `free_string` — except the last
pair, which mutate in place and allocate nothing at all:

```jr
/// Two strings joined into a new one, allocated through context.allocator.
concat :: (a: string, b: string) -> string

/// The `count` bytes of `s` starting at `start`, as a new string (out-of-range is clamped).
substring :: (s: string, start: s64, count: s64) -> string

/// A copy of `s` with ASCII lowercase letters raised to uppercase; other bytes unchanged.
to_upper_copy :: (s: string) -> string

/// A copy of `s` with ASCII uppercase letters lowered; other bytes unchanged.
to_lower_copy :: (s: string) -> string

/// The in-place twin of to_upper_copy — no allocation, mutates s byte by byte.
to_upper_in_place :: (s: string)

/// The in-place twin of to_lower_copy.
to_lower_in_place :: (s: string)

/// Releases a string this module allocated. Safe on a "" result.
free_string :: (s: string)
```

`to_upper_copy`/`to_lower_copy` are not called `to_upper`/`to_lower`: `Basic` already has a `to_upper`
and a `to_lower`, the byte classifiers a lexer wants (`to_upper :: (c: u8) -> u8`), and `#import` is flat
(ADR-0166 §7), so a file importing both modules unqualified got `E0211` on every use (ADR-0197 §7). Both
collisions resolved into **Jai's own naming** rather than a compromise invented to dodge the error:
`to_upper_copy` is what Jai calls exactly this procedure, and the suffix earns its keep independently — it
says at the call site that the routine allocates, which `to_upper_in_place` beside it does not.

The memory comes from `context.allocator`, and the convention is caller-frees. That was a deliberate
choice: not temporary storage (a result that silently expires on an unrelated
`reset_temporary_storage()` is a trap), and not an explicit allocator parameter on every routine (the
context exists to carry exactly this — install an arena once and every routine uses it). **A caller must
install an allocator first**: `context.allocator` is null until then, and calling a null one *traps*, so
concatenating without installing an allocator gives a trap naming the null pointer rather than a silent
wrong answer. A failed allocation returns `""`, because a trap is for a *program* error and running out
of memory is not one.

```jr
#import "Basic";
#import "String";

/// The allocate half of an allocator: a wrapper around libc malloc, because a #foreign procedure cannot fill
/// a procedure-pointer field directly.
libc_alloc :: (n: s64) -> *u8 {
    return malloc(n);
}

/// The release half.
libc_free :: (p: *u8) {
    free(p);
}

main :: () {
    context.allocator = libc_alloc;
    context.allocator_free = libc_free;

    n := 0;

    c := concat("ab", "cd");
    if c.count == 4 && equal(c, "abcd") {
        n = n + 1;
    }
    free_string(c);

    // Empty cases allocate nothing; free_string is a no-op on them.
    left := concat("", "xy");
    right := concat("xy", "");
    if equal(left, "xy") && equal(right, "xy") {
        n = n + 2;
    }
    free_string(left);
    free_string(right);

    mid := substring("hello", 1, 3);
    if equal(mid, "ell") {
        n = n + 4;
    }
    free_string(mid);

    // A copy, since to_upper_in_place mutates in place and a literal's bytes are read-only —
    // mutating "ell" itself would trap.
    owned := substring("hello", 1, 3);
    to_upper_in_place(owned);
    if equal(owned, "ELL") {
        n = n + 8;
    }
    to_lower_in_place(owned);
    if equal(owned, "ell") {
        n = n + 16;
    }
    free_string(owned);

    // Clamped past the end: "up to 99 bytes from index 2" is "llo".
    clamped := substring("hello", 2, 99);
    if equal(clamped, "llo") && clamped.count == 3 {
        n = n + 32;
    }
    free_string(clamped);

    u := to_upper_copy("aB3z");   // "AB3Z" — the digit is left alone
    if equal(u, "AB3Z") {
        n = n + 64;
    }
    free_string(u);

    l := to_lower_copy("aB3z");   // "ab3z"
    if equal(l, "ab3z") {
        n = n + 128;
    }
    free_string(l);

    exit(n);
}
```

The allocation wrappers exist because a `#foreign` procedure cannot fill a procedure-pointer field
directly, so `libc_alloc`/`libc_free` wrap `malloc`/`free`. `substring` **clamps** an out-of-range
request rather than trapping — asking for more than remains gives what remains — for the same reason
`byte_at` returns `-1`. Every result is freed, so under the differential harness a leak, a double-free,
or a wrong copy is a different exit status in one engine. The exit code is **255**.

## The C string boundary

A Jairs `string` is counted, and a C string is NUL-terminated — the two conventions do not coerce into
each other, and the boundary is real enough that it has caught two independent bugs while this library
was written (recorded in AGENTS.md and ADR-0198). **A string literal's bytes are not followed by a NUL.**
Passing `"PATH".data` straight to a C function like `getenv` reads whatever byte the linker happened to
place next — no diagnostic, no trap, just a wrong answer, and one both engines agree on, since both are
reading past the same string. That is worth knowing on its own: it means the differential harness that
catches every other divergence in this project cannot catch this one.

```jr
/// Length of a NUL-terminated C string.
c_style_strlen :: (str: *u8) -> s64

/// Borrows a C string as a string.
to_string :: (str: *u8) -> string

/// NUL-terminated copy for C; caller frees.
to_c_string :: (s: string) -> *u8
```

`to_c_string` and `to_string` are the whole FFI boundary, and they are not symmetric. `to_c_string`
**allocates and terminates** — it copies `s`'s bytes and appends the NUL a C function will scan for. It
returns a raw `*u8`, not a `string`, so the caller releases it through `context.allocator_free` (or
`free`, when the allocator is `malloc`'s) rather than `free_string` — the same obligation `concat` and
`substring` carry, spelled differently because the result is a C string rather than a Jairs one.
`to_string` goes the other way and **cannot** allocate: it just `borrow`s the C string's bytes back as a
counted `string`, because there is nowhere to put a NUL that is not someone else's byte, and a
*borrowed* string has no NUL to put anywhere.

```jr
#import "Basic";
#import "String";

libc_alloc :: (n: s64) -> *u8 {
    return malloc(n);
}

libc_free :: (p: *u8) {
    free(p);
}

main :: () {
    context.allocator = libc_alloc;
    context.allocator_free = libc_free;

    n := 0;

    // to_c_string allocates one byte more than the string and writes the NUL itself.
    terminated := to_c_string("hi");
    if c_style_strlen(terminated) == 2 {
        n = n + 1;
    }

    // to_string is the mirror: it borrows a C string's bytes back as a counted string, stopping
    // at whatever NUL it finds rather than at any particular buffer size.
    borrowed := to_string(terminated);
    if borrowed.count == 2 && equal(borrowed, "hi") {
        n = n + 2;
    }
    free(terminated);

    // The boundary, built by hand: a buffer with a NUL in the middle and a live byte after it.
    // c_style_strlen stops at the NUL and never reads the trailing byte — which is exactly what
    // makes ".data" on a Jairs literal dangerous: a literal's bytes have no NUL to stop at, so a
    // C function that scans for one reads on into whatever the linker placed next.
    buf := context.allocator(4);
    bytes := view(buf, 4);
    bytes[0] = cast(u8, 97);
    bytes[1] = cast(u8, 98);
    bytes[2] = cast(u8, 0);
    bytes[3] = cast(u8, 99);
    if c_style_strlen(buf) == 2 {
        n = n + 4;
    }
    context.allocator_free(buf);

    exit(n);
}
```

The exit code is **7**. `adopt` and `borrow` are the two names over the same construction that `to_string`
and every non-allocating slice routine builds on — `adopt` for a caller that owns the bytes it is handing
over, `borrow` for one that does not — and neither can manufacture a NUL any more than `to_string` can,
for the identical reason.

## Splitting, joining, trimming, and the rest

The library's algorithm surface grew well past the six routines above (ADR-0197 §7, ADR-0198 §1) —
`split` and `join` both exist, and are each other's inverse:

```jr
/// Every piece of `s` between occurrences of `separator`.
split :: (s: string, separator: string) -> []string

/// `pieces` concatenated with `separator` between them.
join :: (pieces: []string, separator: string) -> string

/// `s` with leading and trailing whitespace removed. Borrows `s`.
trim :: (s: string) -> string
```

```jr
#import "Basic";
#import "String";

libc_alloc :: (n: s64) -> *u8 {
    return malloc(n);
}

libc_free :: (p: *u8) {
    free(p);
}

main :: () {
    context.allocator = libc_alloc;
    context.allocator_free = libc_free;

    n := 0;

    // split's pieces borrow the original string; join is its inverse for the same separator.
    pieces := split("a,b,c", ",");
    if pieces.count == 3 && equal(pieces[0], "a") && equal(pieces[1], "b") && equal(pieces[2], "c") {
        n = n + 1;
    }
    back := join(pieces, ",");
    if equal(back, "a,b,c") {
        n = n + 2;
    }
    free_string(back);

    // trim removes whitespace from both ends without allocating — it borrows.
    trimmed := trim("  hi  ");
    if equal(trimmed, "hi") {
        n = n + 4;
    }

    exit(n);
}
```

The exit code is **7**. `trim` sits beside `trim_left`, `trim_right`, and the `_chars` family
(`trim_chars`, `trim_left_chars`, `trim_right_chars`) that all three borrow, so a caller trimming a
specific set of bytes rather than whitespace has the same shape available. `find_nocase` and
`contains_nocase` close the case-insensitive search pair that `equal_nocase`/`compare_nocase` started.
`string_to_float` is `to_integer`'s sibling, reading a leading float and the remainder the same way.
`wildcard_match` does glob matching with `*` and `?` (no escape, matching Jai) — what a build script uses
to select files.

**Path operations still do not live in `String`, and that hasn't changed**: `path_filename`,
`path_extension`, `path_join` and the rest live in `modules/File_Utilities` instead (as `base_name`,
`extension`, `path_join`, `directory_name`, `stem`, `is_absolute`, `normalise`), because a path is a
filesystem concept and adding a second set of path routines here would give a reader two answers to one
question. `File_Utilities.join` was itself renamed to `path_join` in the same wave that renamed
`to_upper`, and for the identical reason: `String` gained its own `join` (the one above), and the flat
import collided.

See also [Book I — The Jairs Language](/language/introduction/).
