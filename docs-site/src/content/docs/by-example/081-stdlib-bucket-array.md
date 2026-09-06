---
title: "Bucket_Array: addresses that do not move"
description: A growable sequence of s64 whose element pointers stay valid across growth, built from fixed-size buckets rather than List's doubling-and-copying storage.
sidebar:
  order: 81
---

`List` doubles its storage and **copies** on growth, so every pointer into it dies at the next `push`.
That is the right trade for a sequence you iterate and the wrong one for a sequence you hold references
into — an entity that points at its parent, a UI element that remembers its child, an interner that hands
out a pointer to a stored value. `Bucket_Array` keeps a `[..]` of fixed-size **buckets** and only ever
appends a bucket, so existing buckets never move and an element's address is stable for as long as the
array lives (ADR-0155).

## An address that does not move

```jr
/// How many elements one bucket holds. A constant rather than a parameter, because the only reason to
/// vary it is a measurement this project cannot take.
BUCKET_SIZE :: 16;

/// One bucket: a pointer to its sixteen slots. A NAMED struct wrapping the pointer, kept for
/// readability even though ADR-0191 gave pointer type arguments to `size_of` and `typed`, so
/// `size_of(*s64)` and `typed(*s64, p)` both work today.
Bucket :: struct {
    /// The bucket's sixteen slots.
    slots: *s64;
}

/// A growable sequence of `s64` whose element addresses never move. `count` is the logical length;
/// the last bucket is partly filled.
Bucket_Array :: struct {
    /// One pointer per bucket. THIS may move when it grows; the buckets may not.
    spine: [..]Bucket;
    /// How many elements have been pushed.
    count: s64;
}

/// An empty bucket array, allocating nothing — nothing is allocated until the first `push`.
make :: () -> Bucket_Array { ... }

/// Appends `value` and returns a STABLE pointer to where it now lives, valid for the life of the array.
/// `null` if a bucket cannot be allocated.
push :: (b: *Bucket_Array, value: s64) -> *s64 { ... }

/// A pointer to element `index`, or `null` when it is out of range — `null` rather than a trap, because
/// this is a lookup that failed rather than a memory error.
get :: (b: *Bucket_Array, index: s64) -> *s64 { ... }

/// Element `index`, or 0 when it is out of range. The value form, for a caller that does not want the
/// pointer.
value_at :: (b: *Bucket_Array, index: s64) -> s64 { ... }

/// How many buckets are allocated — `count` rounded up to a bucket. Exposed because it is the overhead
/// a caller pays for stability.
bucket_count :: (b: *Bucket_Array) -> s64 { ... }

/// Frees every bucket and the spine. The array is empty afterwards and may be pushed to again, which is
/// what makes this safe in a `defer` beside the `make`.
free_all :: (b: *Bucket_Array) { ... }
```

```text
  spine:  [ *bucket0, *bucket1, *bucket2 ]     <- this may move on growth
  buckets: [16 slots] [16 slots] [16 slots]    <- these never move
```

`push` allocates a fresh bucket with `malloc` — not from a growable region — exactly because a bucket's
whole purpose is that its address never changes, and only a fresh, independent allocation can promise
that. The *spine* itself is a `[..]Bucket` and is allowed to move, because nothing outside this module
holds a pointer to the spine; only the buckets it points at must not move. That asymmetry — one level
that may move, one that may not — is the entire design.

An entity system built on `Bucket_Array` gets a pointer per entity that survives every later spawn: a
projectile that stores `*Enemy` from `push` keeps a valid pointer no matter how many more enemies spawn
afterwards, which is exactly the property `List` cannot offer.

## Reading through an old pointer

```jr
#import "Basic";
#import "Bucket_Array";

main :: () {
    total := 0;

    b := make();

    if bucket_count(*b) == 0 {
        total = total + 16;
    }

    // **Take a pointer to the first element**, then grow far past its bucket. Fifty elements is four
    // buckets, so three more are allocated after this pointer was handed out — and it must still be valid.
    first := push(*b, 100);

    i := 1;
    while i < 50 {
        _ = push(*b, i);
        i = i + 1;
    }

    // Read through the *old* pointer. In a `List` this would be reading freed memory.
    total = total + first.*;

    sum := 0;
    j := 1;
    while j < 50 {
        sum = sum + value_at(*b, j);
        j = j + 1;
    }
    total = total + sum;

    // 50 elements in buckets of 16 is 4 buckets — the overhead a caller pays for stability.
    total = total + bucket_count(*b);

    if get(*b, 50) == null {
        if get(*b, -1) == null {
            total = total + 8;
        }
    }

    free_all(*b);

    if bucket_count(*b) == 0 {
        exit(total);
    }
    exit(1);
}
```

The property under test is the one `List` cannot offer: a pointer taken at element zero survives three
more buckets' worth of growth, and reading through it after — `first.*` — reads the value that was
written, not freed memory. Fifty elements at sixteen per bucket is four buckets, and `bucket_count`
proves the arithmetic maps indices to slots correctly by matching that number exactly. The exit status is
**73** — this is `tests/corpus/valid/124-bucket-array.jr`, whose full checksum (`1353 & 255`) also
includes the sum `0 + 1 + … + 49 = 1225` from reading every value back through `value_at`.

## What is absent, and why

`Bucket_Array` has <span class="jairs-status absent">absent</span> removal, by design rather than by
omission. A bucket array's whole promise is that an address stays valid, and removal has to answer what
happens to the hole: compacting moves elements and breaks the promise, and a tombstone means every read
has to check one, so `get` stops being pointer arithmetic. Both are real designs; neither is decidable
without knowing what the caller wants, so this module offers append and read, which is what the
stability promise is *for*.

It is concrete `s64`, not `Bucket_Array($T)`, for the same reason `List` and `Map`'s procedures stay
concrete: an imported *template* is refused across a module boundary (E0268), so a generic bucket array
cannot cross one either. `push_bucket`, the routine that grows the spine, is also **public** even though
its doc comment describes it as an internal helper serving `push` — worth knowing before treating this
module's surface as fully considered, since its signature exposes the raw `*[..]Bucket` spine to anyone
who imports it.

A game that wants `Bucket_Array($T)` — entities of a struct type rather than `s64` — cannot have it until
cross-module template instantiation lands; today an entity system built on this module stores an index or
an encoded `s64` per entity and looks the rest up separately, or keeps its own hand-written concrete
bucket array shaped like this one for its entity struct.

See also [Book I — The Jairs Language](/language/introduction/).
