---
title: Dynamic arrays
description: "`[..]T`'s compiler-known {data, count, capacity} layout, and modules/List's push, pop, get, set and elements over it."
sidebar:
  order: 29
---

`[..]T` is a growable array with a compiler-known layout — `{data: *T, count: s64, capacity: s64}` — but the compiler gives it no operations of its own (ADR-0136). Growth is a library's job: `modules/List` converted from a hand-rolled `List :: struct($T)` onto the native type and supplies `push`, `pop`, `get`, `set`, `clear`, `elements` and `free_data` over a `*[..]s64` (ADR-0140).

## A zero-initialised list

```jr
// A zero-initialised native dynamic array: no allocation, empty, safe to free.
xs: [..]s64;
if is_empty(*xs) && xs.count == 0 && xs.capacity == 0 && xs.data == null {
    n = n + 1;
}
```

A declared `[..]s64` with no initialiser has `.data` null and both counts zero — no allocation has happened, and it is safe to hand straight to `free_data`.

## Growing it: push, pop, get, set

```jr
/// Appends `v`, answering `false` when the allocation it needed failed.
///
/// Amortised `O(1)`: the capacity doubles, so `n` pushes copy `O(n)` elements in total.
push :: (a: *[..]s64, v: s64) -> bool {
    if a.count >= a.capacity {
        if !grow(a) {
            return false;
        }
    }
    (a.data + a.count).* = v;
    a.count = a.count + 1;
    return true;
}
```

```jr
/// The element at `index`, plus whether the index was in the **used** range.
get :: (a: *[..]s64, index: s64) -> (s64, bool) {
    if index < 0 {
        return 0, false;
    }
    if index >= a.count {
        return 0, false;
    }
    return (a.data + index).*, true;
}
```

`push` past the first capacity — four elements — reallocates and doubles, so eight pushes into a fresh list trigger exactly one reallocation and every earlier element survives the copy. `pop` and `set` both bound on `.count`, not `.capacity`: the slots between them hold whatever the allocator handed back, genuinely undefined rather than merely unused, so `set(a, a.count, v)` answers `false` instead of silently extending the list.

## elements(): a view over the used prefix

```jr
elements :: (a: *[..]s64) -> []s64 {
    return view(a.data, a.count);
}
```

`view` is a language intrinsic — no import needed — that turns a pointer and a count into a `[]s64`. `elements(*xs)` hands the list's used prefix straight to anything that already takes a view, `sort_ints(elements(*xs))` among them, because a view is a window onto the same storage rather than a copy. That view is invalidated by anything that reallocates: a `push` that grows moves the storage, and `free_data` invalidates it outright — nothing enforces this, since there is no borrow checker.

## What indexing directly cannot do

```jr
xs: [..]s64;
v := xs[0];
```

is <span class="jairs-status refused">refused</span>:

```
error[E0234]: cannot index a value of type `[..]s64`
  = only a fixed-size array `[N]T` and a view `[]T` can be indexed
  = dynamic arrays `[..]T` arrive in a later wave
```

A `[..]T` carries no `[]` operator of its own — read it through `view(data, count)`, or through `elements()` if it is one `modules/List` already owns, and index the view instead.

## Freeing: free_data

```jr
/// Releases the storage and empties the list.
///
/// **This is the routine a caller must not forget.** There are no destructors, so nothing else will free the
/// memory. Safe to call on a list that never grew (`data` is null and `free` of null is a no-op) and safe to
/// call twice, because it resets `data` — a second call frees nothing rather than freeing again.
free_data :: (a: *[..]s64) {
    if a.data != null {
        free(untyped(a.data));
    }
    a.data = null;
    a.count = 0;
    a.capacity = 0;
}
```

## Growth allocates through malloc, not context.allocator

`List`'s `grow` calls `Basic.malloc` and `free` directly rather than going through `context.allocator` — the convention `String`, `File_Utilities` and `JSON` follow for their own allocating routines. That is a deliberate module choice, not an oversight: `List` and `Map` both allocate through libc's own `malloc`/`free` rather than the context's pair, matching `List`'s own module docs, which name `String` allocating through the context and `List`/`Map` allocating with `malloc` as a real seam in the library rather than an inconsistency to hide. A program that mixes `List` with an installed arena allocator therefore still frees a list through `free_data`, not through `context.allocator_free` — the two allocation stories in this library have not (yet) been unified.

See also [Book I — The Jairs Language](/language/introduction/).
