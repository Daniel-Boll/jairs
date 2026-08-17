---
title: push_context
description: A block with its own copy of the context — writes inside stay inside, and the enclosing context is restored on exit.
sidebar:
  order: 43
---

Because the context is shared by pointer, a callee's write to a field is visible to its caller.
`push_context` is the construct that scopes that: it gives a block its own copy of the context, so
writes inside the block are the block's own, and the enclosing scope's context is restored when the
block exits.

```jr
#import "Basic";

/// An allocator that records how many bytes it was asked for.
recording_alloc :: (n: s64) -> *u8 {
    context.allocator_data = context.allocator_data + n;
    return malloc(n);
}

/// The release half.
recording_free :: (p: *u8) {
    free(p);
}

/// A callee that writes the context. It never sees the push_context.
callee_writes :: () {
    context.allocator_data = 42;
}

main :: () {
    n := 0;

    // Before the block: set the field to 7 in the caller's own context.
    context.allocator_data = 7;

    push_context {
        // Inside: this is a *copy*. Writing it does not touch the caller's context.
        context.allocator_data = 99;

        // A callee's write is still visible to us — sharing is scoped, not turned off.
        callee_writes();
        if context.allocator_data == 42 {
            n = n + 1;
        }

        // Install an allocator in the copy, allocate through it, and free at block exit.
        context.allocator = recording_alloc;
        context.allocator_free = recording_free;
        context.allocator_data = 0;
        p := context.allocator(10);
        defer context.allocator_free(p);
        if p == null {
            exit(1);
        }
        // The recording allocator moved the copy's state word.
        if context.allocator_data == 10 {
            n = n + 2;
        }
    }

    // After the block: the caller's context is restored. allocator_data is 7 again.
    if context.allocator_data == 7 {
        n = n + 4;
    }

    // `n` accumulated across the block boundary — an ordinary local.
    if n == 7 {
        exit(0);
    }
    exit(2);
}
```

## Writes inside stay inside

`main` sets `allocator_data` to `7`, the block sets it to `99` (and later to other values), and
after the block it is `7` again. That is the isolation and the whole point: everything the block
wrote — its `99`, the callee's `42`, the allocator's `10` — lived in the copy, which is discarded
when the block exits. The enclosing context is restored untouched.

## Sharing is scoped, not switched off

`push_context` does not disable the by-pointer sharing described on the [context
page](/by-example/042-context/) — it scopes it. Inside the block the context is still shared
*downward*: `callee_writes` sets `allocator_data` to `42` and the block sees it, exactly as it would
without a `push_context`. The construct changes which object callees below share, not whether they
share it.

## `defer` runs against the pushed context

The `defer context.allocator_free(p)` runs at block exit — and crucially, *before* the copy is torn
down, while it is still the current context. So the free reaches `recording_free` through the same
allocator that allocated `p`. A `defer` that ran against the restored outer context would free
through the wrong allocator; this one does not.

## Ordinary locals cross the boundary freely

The copy is a lowering-time concern that touches only the context, not ordinary variables. `n`
accumulates inside the block and is read after it with no trouble — a value computed inside and read
outside is fine. Only the context is copied and restored.

## Teeth

Both engines lower the copy as the same aggregate load/store that a plain `b := a` produces, which
both back ends already memcpy — so there is no engine-specific path to diverge. The exit status
encodes the result: a leaked write, a lost restore, or a `defer` that ran against the wrong context
would each change it. Note this file uses `exit(2)` for the failure arm rather than `exit(1)`,
because `exit(1)` is already spent on the allocation-failed guards.

See also [Book I — The Jairs Language](/language/introduction/).
