---
title: "Literals: integers, strings, comments"
description: Integer bases and digit separators, string escapes and the string's layout, and Jairs' nesting comments and doc comments.
sidebar:
  order: 16
---

This page collects the lexical building blocks: how integers, strings, comments, and doc
comments are written.

## Integer literals

```jr
main :: () {
    decimal := 1234567890;
    zero := 0;

    hex := 0xdead_beef;
    binary := 0b1010_1010;
    octal := 0o755;

    // Underscores are permitted as digit separators anywhere after the
    // first digit.
    grouped := 1_000_000;

    // The largest s64.
    max := 9223372036854775807;
}
```

Integers come in four bases: decimal, hexadecimal (`0x`), binary (`0b`), and octal (`0o`).
Underscores may be used as digit separators anywhere after the first digit — so `0xdead_beef`,
`0b1010_1010`, and `1_000_000` are all legal and mean exactly what they would without the
underscores. The last line writes out the largest `s64`.

## String literals

```jr
main :: () {
    plain := "simple";
    empty := "";
    escapes := "tab:\there\nnewline above\r\n";
    quoted := "she said \"hello\"";
    slash := "back\\slash";
    zero := "embedded\0nul";
    unicode := "caf\u00e9 \u4e2d\u6587";

    // Strings are `{data: *u8, count: s64}` and are NOT NUL-terminated
    // (ADR-0004). Interop with C goes through `to_c_string`.
    n := plain.count;
    d := plain.data;
}
```

String literals support the usual escapes: `\t`, `\n`, `\r`, an escaped quote `\"`, an escaped
backslash `\\`, a NUL byte `\0` (which may appear *inside* a string, not just at the end), and
Unicode escapes written as \uXXXX (here `\u00e9` for é and `\u4e2d\u6587` for 中文).

The layout of a string is the important design point (ADR-0004): a `string` is a
`{data: *u8, count: s64}` pair — a pointer to bytes plus a length — and it is **not**
NUL-terminated. That is why you can embed `\0` in the middle of one. The `count` and `data`
fields are readable directly, as `plain.count` and `plain.data` show. Passing a Jairs string to
C, which does expect NUL termination, goes through `to_c_string`.

## Comments

```jr
// A line comment.

/* A block comment. */

/*
    Block comments /* nest */, unlike C. The lexer tracks depth so that
    commenting out a region containing a comment does the obvious thing.
*/

// Trailing comment with no newline after it.
```

Jairs has line comments (`//`) and block comments (`/* ... */`). Unlike C, block comments
**nest**: the lexer tracks depth, so commenting out a region that already contains a comment
does the obvious thing rather than ending early at the first `*/`.

## Doc comments

```jr
//! Doc comments, from the file's own downwards (ADR-0027).

#import "Basic";

/// A point in the plane.
///
/// Documented across several lines, so that the blank `///` in the middle is exercised
/// rather than assumed to work.
Point :: struct {
    /// A field's doc comment attaches to no *item*, so `file_docs` does not record it —
    /// but `jr fmt` must still keep it, and did not until this file existed.
    x: s64;
    y: s64;  // An ordinary trailing comment, on the same line.
}

/// The message this program prints.
MESSAGE :: "doc comments are trivia\n";

//// ------------------------------------------------------------------
//// Four slashes are a rule, not documentation, and this line proves it
//// parses as an ordinary comment.
//// ------------------------------------------------------------------
```

Doc comments are a distinct trivia kind (ADR-0027). `//!` documents the enclosing item from the
inside — at the top of a file it documents the file itself. `///` documents the item that
follows it, and may span several lines, including a blank `///` in the middle.

Two honesty markers from the corpus file are worth carrying over:

- A `///` on a **struct field** attaches to no *item*, so the file-level documentation record
  does not keep it — but `jr fmt` must still preserve it verbatim (and once did not, which is
  why this file exists).
- **Four** slashes (`////`) are *not* a doc comment — they parse as an ordinary comment. The
  banner above proves it.

Crucially, doc comments are trivia: they cannot change what a program does. The corpus file
executes precisely to guarantee that the program's output is identical to the same program with
every doc comment deleted.

See also [Book I — The Jairs Language](/language/introduction/).
