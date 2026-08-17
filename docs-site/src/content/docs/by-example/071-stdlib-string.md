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

Each of these produces a **new** string and the **caller frees** it with `free_string`:

```jr
/// Two strings joined into a new one, allocated through context.allocator.
concat :: (a: string, b: string) -> string

/// The `count` bytes of `s` starting at `start`, as a new string (out-of-range is clamped).
substring :: (s: string, start: s64, count: s64) -> string

/// A copy of `s` with ASCII lowercase letters raised to uppercase; other bytes unchanged.
to_upper :: (s: string) -> string

/// A copy of `s` with ASCII uppercase letters lowered; other bytes unchanged.
to_lower :: (s: string) -> string

/// Releases a string this module allocated. Safe on a "" result.
free_string :: (s: string)
```

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

    // Clamped past the end: "up to 99 bytes from index 2" is "llo".
    clamped := substring("hello", 2, 99);
    if equal(clamped, "llo") && clamped.count == 3 {
        n = n + 16;
    }
    free_string(clamped);

    u := to_upper("aB3z");   // "AB3Z" — the digit is left alone
    if equal(u, "AB3Z") {
        n = n + 64;
    }
    free_string(u);

    l := to_lower("aB3z");   // "ab3z"
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
or a wrong copy is a different exit status in one engine.

Deliberately **absent**: `split`, and anything else that would need a second allocation decision beyond
what `concat`/`substring`/`to_upper`/`to_lower` already settled.
