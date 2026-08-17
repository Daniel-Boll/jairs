---
title: Flag enums
description: enum_flags with power-of-two numbering, the & test idiom, and the bitwise operators they build on.
sidebar:
  order: 24
---

An `enum_flags` is an enum whose members are meant to combine (ADR-0043). Its members are numbered by powers of two so that `|` unions them and `&` tests them. This page pairs it with the bitwise operators (ADR-0042) it relies on, including two precedence rules that deliberately differ from C.

## Power-of-two numbering

```jr
#import "Basic";

/// Auto-numbered: 1, 2, 4.
Perm :: enum_flags {
    READ;
    WRITE;
    EXEC;
}

/// The three numbering edge cases, in one declaration.
Edge :: enum_flags {
    NONE :: 0;
    A;
    B :: 8;
    // 16, not 4: the next power of two above 8.
    C;
    // A named mask, deliberately not a power of two.
    AB :: 3;
    // 4 -- the next power of two above 3.
    D;
}

main :: () {
    n := 0;

    // A combination keeps the flags type.
    both := Perm.READ | Perm.WRITE;
    if cast(s64, both) == 3 {
        n = n + 8;
    }

    // Testing a flag: the `&` idiom.
    if (both & Perm.READ) == Perm.READ {
        n = n + 16;
    }

    // `^` and `~` also keep the type.
    toggled := both ^ Perm.EXEC;
    if cast(s64, ~Perm.READ) == -2 {
        n = n + 128;
    }
    // ...
    if n == 32767 {
        exit(0);
    }
    exit(1);
}

can_write :: (p: Perm) -> bool {
    return (p & Perm.WRITE) == Perm.WRITE;
}
```

The numbering has three parts that are each easy to get wrong:

- Members are **auto-numbered by powers of two from 1** (`READ`, `WRITE`, `EXEC` are 1, 2, 4). Sequential numbering would make `READ | WRITE` equal `0 | 1` = 1, which *equals* `WRITE` and makes the type useless.
- An **explicit value** is allowed, and a later member continues from the next power of two **strictly above the previous value** — not above its index. In `Edge`, `C` follows `B :: 8` and so is `16`, not `4`.
- That holds even when the previous value is not itself a power of two. `AB :: 3` is a legal named mask, and `D` after it is `4` (the next power of two above 3), so a non-power-of-two predecessor does not simply double.

**Zero is never auto-created** — `NONE :: 0` is written out explicitly, and it leaves the sequence alone.

A combination like `READ | WRITE` keeps the flags type and names no single member — that is correct, not a gap. Testing a flag is the `&` idiom: `(both & Perm.READ) == Perm.READ`. This composes where a dedicated binary operator would not — `(toggled & (READ | EXEC)) == (READ | EXEC)` tests two flags at once. The operators `^` (toggle) and `~` (complement) also preserve the type. A plain `enum` is unaffected by all of this: it stays sequential from 0 and refuses `|`.

## The bitwise operators, and two non-C precedence rules

```jr
#import "Basic";

main :: () {
    a := 6;
    n := 0;

    if (a & 3) == 2 {
        n = n + 1;
    }
    if (a << 2) == 24 {
        n = n + 8;
    }

    // `~` complements on the type's *own* width.
    if ~cast(u8, 0) == 255 {
        n = n + 32;
    }

    // `>>` is arithmetic for a signed type, logical for an unsigned one.
    s: s8 = -8;
    if (s >> 1) == -4 {
        n = n + 64;
    }
    u: u8 = 240;
    if (u >> 4) == 15 {
        n = n + 128;
    }

    // Precedence: bitwise binds tighter than comparison.
    if a & 3 == 2 {
        n = n + 256;
    }

    // Shifts above `+`: `1 + 1 << 3` is `1 + 8`, not `2 << 3`.
    if 1 + 1 << 3 == 9 {
        n = n + 512;
    }

    // `|` loosest, then `^`, then `&`: this is `(6 & 3) | (1 ^ 2)` = 3.
    if (6 & 3 | 1 ^ 2) == 3 {
        n = n + 1024;
    }

    // Compound assignment, all five forms.
    f := 1;
    f <<= 3;
    f |= 1;
    f &= 12;
    f ^= 4;
    f >>= 1;
    if f == 6 {
        n = n + 2048;
    }

    if n == 4095 {
        exit(0);
    }
    exit(1);
}
```

The six operators are `& | ^ ~ << >>`, plus the five compound-assignment forms (`<<=`, `|=`, `&=`, `^=`, `>>=`).

Two precedence rules are **not C's**:

- **Bitwise binds tighter than comparison**, so `a & 3 == 2` parses as `(a & 3) == 2`. C reads it the other way — a choice Ritchie later called a mistake kept only for compatibility. Under C's ordering, `a & 3 == 2` would be `a & (3 == 2)`, i.e. `a & false` — a type error here rather than a wrong answer, so C's ordering would actually make Jairs *refuse* a line that reads correctly.
- **Shifts sit between `+` and `*`**, so `1 + 1 << 3` is `1 + (1 << 3)` = 9. C puts shifts below `+` and would read it as `(1 + 1) << 3`.

Among the bitwise binary operators themselves, `|` is loosest, then `^`, then `&`, so `6 & 3 | 1 ^ 2` is `(6 & 3) | (1 ^ 2)` = `2 | 3` = 3.

Two more behaviours are type-directed. `~` complements over the type's **own** width, so `~cast(u8, 0)` is `255`, not a truncated `-1`. And `>>` is **arithmetic** for a signed type (sign-preserving: `-8 >> 1` is `-4`) but **logical** for an unsigned one (`240 >> 4` is `15`) — the type decides, exactly as it does for `/`. The shift count is a separate integer and need not share the value's type — the one binary form where that is true.
