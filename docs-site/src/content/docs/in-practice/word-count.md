---
title: Counting words and lines
description: A mini wc that scans a string once, counting bytes, words and lines.
sidebar:
  order: 1
---

Our first real program is a miniature `wc`: given a block of text, count its bytes, words and
lines in a single pass. It leans on the [`String`](/language/the-standard-library/#string)
module for byte access, and it shows the shape of a scanning loop in Jairs.

```jr
#import "Basic";
#import "String";

// A byte is whitespace if it is a space, tab, newline or carriage return.
is_space :: (b: s64) -> bool {
    return b == 32 || b == 9 || b == 10 || b == 13;
}

Counts :: struct {
    bytes: s64;
    words: s64;
    lines: s64;
}

// Scan `text` once, counting bytes, words and lines. A "word" is a maximal run
// of non-whitespace bytes, so words are counted at each non-space -> space edge.
count_text :: (text: string) -> Counts {
    c: Counts;
    c.bytes = text.count;

    in_word := false;
    i := 0;
    while i < text.count {
        b := byte_at(text, i);
        if b == 10 {
            c.lines = c.lines + 1;
        }
        if is_space(b) {
            in_word = false;
        } else if !in_word {
            in_word = true;
            c.words = c.words + 1;
        }
        i = i + 1;
    }
    return c;
}

main :: () {
    text := "the quick brown fox\njumps over\nthe lazy dog\n";
    c := count_text(text);

    print("lines: ");
    print_int(c.lines);
    print("\nwords: ");
    print_int(c.words);
    print("\nbytes: ");
    print_int(c.bytes);
    print("\n");
}
```

Running it:

```
lines: 3
words: 9
bytes: 44
```

## How it works

**Bytes come for free.** A `string` is a `{data, count}` pair, so `text.count` is the byte
length directly — no scanning needed. We store it first.

**Reading one byte at a time.** `byte_at(text, i)` returns the `i`-th byte as an `s64`. This
is the `String` procedure that exists precisely because `text.data[i]` does not compile — a
`*u8` is not indexable (see [Values and types](/language/values-and-types/#strings)). Every
comparison in `is_space` is against a byte's numeric code: 32 is space, 9 is tab, 10 is
newline, 13 is carriage return.

**Counting words with an edge detector.** The subtle part of any word counter is not
double-counting. We track whether we are currently *inside* a word with `in_word`. A word is
counted only on the transition from "not in a word" to "in a word" — the `else if !in_word`
branch — so a run of several spaces, or several letters, each counts once. Lines are simpler:
one per newline byte.

**Returning a struct.** `count_text` returns a whole `Counts` struct by value, which
[Procedures](/language/procedures/#aggregate-returns) covers. The caller binds it with `:=`
and reads its fields.

## What it demonstrates

- The `String` module and byte-level scanning without heap allocation.
- A `struct` used as a small bundle of results, returned by value.
- The `bool` state machine that makes single-pass counting correct.

Everything here runs identically under `jr run` and a native `jr build`. Next:
[a bracket checker built on a stack](/in-practice/balanced-brackets/).
