---
title: Unions & variants
description: The untagged union that reinterprets bits, and the tagged variant with a checked read.
sidebar:
  order: 25
---

Jairs has two overlapping-storage types. A `union` (ADR-0045) is **untagged**: all fields share offset 0, and reading a field you did not write reinterprets the bytes. A `variant` (ADR-0068) is a **tagged** union: a write records which case is live, and reading the wrong one traps. They are separate declaration forms because they make different bargains.

## Unions reinterpret bits

```jr
#import "Basic";

/// Two fields of the same width.
Bits :: union {
    signed: s64;
    unsigned: u64;
}

/// Fields of *different* widths.
Mixed :: union {
    byte: u8;
    word: s64;
}

Tagged :: struct {
    which: s64;
    value: Bits;
}

main :: () {
    n := 0;

    // Reinterpretation: -1 as an `s64` is every bit set.
    b: Bits;
    b.signed = -1;
    if b.unsigned == 18446744073709551615 {
        n = n + 1;
    }

    // Every field is at offset 0: the narrow field sees the wide field's low byte.
    m: Mixed;
    m.word = 511;
    if m.byte == 255 {
        n = n + 4;
    }

    // A default-initialised union is zeroed like any other aggregate.
    z: Bits;
    if z.signed == 0 {
        n = n + 16;
    }
    // ...
    if n == 1023 {
        exit(0);
    }
    exit(1);
}
```

A union is a struct's shape with one layout rule changed: **all fields sit at offset 0**, and the union is the size of its largest field. Writing one field then reading another reinterprets the bits — `b.signed = -1` (every bit set) reads back through `b.unsigned` as `18446744073709551615`. With mixed widths, the narrow field aliases the wide field's low byte: `m.word = 511` (`0x1FF`) makes `m.byte` read `255`.

This is untagged **by decision**, not by oversight (ADR-0045 §1). A tag's value comes almost entirely from exhaustive destructuring; a tag would also make the union wider than its largest field — the one property a systems programmer reaches for a union to get. So the hazard is real and documented rather than prevented.

A union defaults to zero like any aggregate, nests inside a struct (its own following field sits after the union's full width), passes across procedure boundaries by copy, and supports pointers with auto-dereferencing field access — all shown in the full corpus file.

## Variants are tagged and checked

```jr
#import "Basic";

/// A variant with two cases of the same type, deliberately.
V :: variant {
    i: s64;
    f: s64;
}

/// A union of the same two cases, for contrast.
U :: union {
    i: s64;
    f: s64;
}

/// Which case is live, as a number.
which :: (v: V) -> s64 {
    r := 0;
    switch v {
        case .i;
            r = 1;
        case .f;
            r = 2;
    }
    return r;
}

main :: () {
    n := 0;

    // A write sets the tag; reading the same case reads the value back.
    a: V;
    a.i = 7;
    if a.i == 7 {
        n = n + 1;
    }
    if which(a) == 1 {
        n = n + 2;
    }

    // Writing the other case **moves** the tag.
    a.f = 9;
    if which(a) == 2 {
        n = n + 8;
    }

    // The union still reinterprets: no trap, no tag.
    u: U;
    u.i = 5;
    if u.f == 5 {
        n = n + 32;
    }

    if n == 63 {
        exit(0);
    }
    exit(1);
}
```

A `variant` is the tagged form that ADR-0045 §1 said should arrive "as a different declaration form, the way `enum_flags` is different from `enum`" once pattern matching existed to *ask* which case is live. Now that it does (via `switch`), the variant is here — bigger than the equivalent union by the width of its tag, a cost the program chooses to pay.

Its rules:

- **A write sets the tag**, and reading the same case reads back what was written.
- **`switch` over a variant compares the tag** — `case .i` runs exactly when `i` is live. The bare `.i` is the same spelling enums use, not a new rule.
- **Writing a second case moves the tag**, so a later `switch` takes the other arm. The tag is read each time, not folded once. And it belongs to the *object*, not the type: two `V` values can have different live cases.
- **A `switch` over a variant is exhaustive over its cases.**

The two same-typed cases in `V` are deliberate: if the tag were ignored, reading `f` after writing `i` would return `7` rather than trapping, and same-typed cases make that mistake indistinguishable from a correct read — so this shape only works if the tag works. The `U` union alongside is the contrast: writing `i` and reading `f` reinterprets and never traps. That is ADR-0045's bargain, still on offer, and one word smaller.

The file deliberately does **not** read the wrong case — that traps, and a corpus program must run to completion. The wrong-case trap is exercised elsewhere (in the differential harness), where both engines' stderr is compared byte-for-byte, including the trap's exact wording.
