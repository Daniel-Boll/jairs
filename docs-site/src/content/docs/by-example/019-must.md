---
title: "#must"
description: A compile-error-on-ignored-status marker carried in a procedure's type, and the discard that names an ignored result on purpose.
sidebar:
  order: 19
---

`#must` is the half of Jairs' error model ADR-0008 reserved a slot for and never filled: a fallible operation returns a value beside a success flag, and until ADR-0151 nothing stopped a caller from reading the value and dropping the flag. `#must` on a declaration makes that a compile error for every caller, because the marker lives in the procedure's **type**, not in a side table.

## The canonical fallible shape

```jr
// `#must` is on the *declaration*, and the obligation lands on every
// caller — including callers in other files, because the marker lives in the procedure's **type**
// (ADR-0008's reserved effect-row slot, finally filled) rather than in a side table.
try_divide :: (a: s64, b: s64) -> (s64, bool) #must {
    if b == 0 {
        return 0, false;
    }
    return a / b, true;
}
```

A caller must receive both return values, `quotient, ok := try_divide(10, 2)`, and go on to look at `ok`. `#must` does not require several returns either — `only_one :: (n: s64) -> s64 #must` is a single value a caller must still not silently drop (ADR-0151).

## What a bare call cannot do

```jr
try_thing :: () -> bool #must {
    return true;
}

main :: () {
    try_thing();
}
```

is <span class="jairs-status refused">refused</span> with E0287, *"the result of `try_thing` must be received: it is `#must`"* — a statement-level call with no receiver at all is exactly the shape `#must` exists to close off. The two ways out are the ones already shown: receive it, `ok := try_thing()`, or discard it visibly with `_ = try_thing()`.

## The deliberate discard

```jr
counter := 0;
_ = bump_and_fail(*counter);
total = total + counter;
```

`_ = …` runs the call for its effect and discards the result, and it is allowed on purpose (ADR-0151 §2): an unbypassable check is one people route around with a wrapper procedure, which hides the decision instead of recording it. `_ = …` is visible in the source, greppable, and reviewable — the same trade `#no_abc` already makes for bounds checks.

## The obligation survives a procedure value

```jr
f := only_one;
total = total + f(10);
```

Because `#must` is in the procedure's *type* rather than attached to its declaration by name, taking `only_one` as a value and calling it through `f` still carries the obligation. A side table keyed on the declaration would have lost it here.

## Where the library uses it

`#must` fills five of W7's modules that were otherwise finished, because a fallible routine with no way to force the check was the one thing left. `modules/File` marks every routine `#must` except `close`:

```jr
open :: (path: string, flags: s64) -> (File, bool) #must {
```

and explains the one asymmetry in its own docs: *"`close` is **not** `#must`… A failing `close` means buffered data was lost — but this module does not buffer, so there is nothing to lose; and a caller who checks it has no recovery available, because the descriptor is gone either way."* `modules/Thread` marks both halves of the lifecycle:

```jr
spawn :: (body: (*u8) -> *u8 #c_call, argument: *u8) -> (Thread, bool) #must {
```

```jr
join :: (thread: *Thread) -> bool #must {
```

— a spawn that silently failed and was then joined would hang or read garbage, and joining twice or joining a thread that never started fails too, silently, without the marker. `modules/Image`'s `load_bmp` is `#must` because a missing or corrupt file is the routine's *expected* failure, and `modules/Input`'s `push` is `#must` for the opposite reason its own `next_event` is not: `next_event`'s `bool` means "an event was written", not a failure, so an empty queue in the ordinary draw loop needs no ceremony, while `push`'s `bool` really is a failure a caller must not drop.

See also [Book I — The Jairs Language](/language/introduction/).
