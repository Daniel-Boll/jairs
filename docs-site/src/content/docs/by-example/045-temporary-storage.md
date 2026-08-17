---
title: Temporary storage
description: "`talloc` — a per-context bump arena with no per-piece free, that travels with the call and rewinds on reset."
sidebar:
  order: 45
---

Temporary storage is the payoff for the previous three features rather than new machinery. `talloc(n)`
hands out `n` bytes from a per-context bump arena that stays valid until the next
`reset_temporary_storage()`, with no per-piece free. It is `malloc` for the region, a cursor advanced
with pointer arithmetic, and two context fields to hold them — assembled, not invented.

```jr
#import "Basic";

/// Allocates from the temporary arena without having created it — it reads
/// the context and gets whatever region its caller set up.
fill_via_talloc :: (value: u8) -> *u8 {
    p := talloc(8);
    if p == null {
        return null;
    }
    p.* = value;
    return p;
}

main :: () {
    n := 0;

    // First allocation creates the region (it was null in main's zeroed context).
    a := talloc(8);
    if a == null {
        exit(1);
    }
    a.* = 11;

    // A callee allocates from the same arena — it never saw the region created.
    b := fill_via_talloc(22);
    if b == null {
        exit(1);
    }

    // Distinct cells: writing `b` did not disturb `a`.
    if a.* == 11 {
        n = n + 1;
    }
    if b.* == 22 {
        n = n + 2;
    }

    // Reset rewinds the cursor; the region is reused, so the next talloc aliases `a`.
    reset_temporary_storage();
    c := talloc(8);
    if c == null {
        exit(1);
    }
    c.* = 33;
    if a.* == 33 {
        n = n + 4;
    }

    // Overflow returns null rather than trapping.
    big := talloc(TEMP_REGION_SIZE + 1);
    if big == null {
        n = n + 8;
    }

    if n == 15 {
        exit(0);
    }
    exit(1);
}
```

## Distinct cells from a bumping cursor

Successive `talloc` calls return different cells, because each one advances the arena's cursor. `a`
and `b` do not alias, so writing `b` with `22` leaves `a` reading `11`. That the two survive
independently is the first thing the file checks.

## The arena travels with the context

`fill_via_talloc` allocates without ever having created a region. It reads `context.temp_data` and
gets the one `main` set up on its first `talloc`. This is the same argument the [allocator
page](/by-example/041-allocators/) made for `alloc_through_context`: a resource installed on the
context is reachable from any callee, so temporary storage is shared across the call graph for free.

## Reset rewinds, it does not free

`reset_temporary_storage()` moves the cursor back to the start of the region rather than freeing the
region itself. The next `talloc` therefore reuses the same memory: after the reset, `c` aliases `a`,
so writing `c.* = 33` makes `a.*` read `33`. This is the arena discipline — allocate freely within a
phase, reset once at the end, pay nothing per piece.

## Overflow returns null

Asking for more than the region holds — `talloc(TEMP_REGION_SIZE + 1)` — returns null rather than
trapping, the same way `malloc` reports failure. So a program can check the result and recover
instead of dying. `TEMP_REGION_SIZE` is the arena's fixed size, and the region is 64 KiB.

## A note on element width

`talloc` returns a `*u8` — a byte buffer — so `fill_via_talloc` stores a `u8`. Storing a wider type
through the pointer would need a pointer cast, which the language does not have yet. A byte arena
holding bytes is the shape available now, and it exercises the arena all the same.

## Teeth

Four assertions add `1 + 2 + 4 + 8` to reach `15`. A shared cursor that failed to bump, a reset that
did not rewind, or a callee that allocated its own region instead of sharing `main`'s would each
change the total and the exit status. Both engines draw the region from their own `malloc` source,
so the addresses differ, but the bytes written and read back — what the assertions compare — do not.

See also [Book I — The Jairs Language](/language/introduction/).
