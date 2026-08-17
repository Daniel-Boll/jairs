---
title: A bracket checker with a stack
description: Use a fixed-capacity array as a stack to check that brackets are balanced.
sidebar:
  order: 2
---

A classic use of a stack: decide whether the brackets in a string — `()`, `[]`, `{}` — are
balanced and correctly nested. This program uses the
[`Array`](/language/the-standard-library/#array-and-list) module as a stack, and it shows how
Jairs' **two-value returns** stand in for exceptions.

```jr
#import "Basic";
#import "String";
#import "Array";

// The matching opener byte for a closing bracket, or -1 if `b` is not a closer.
opener_for :: (b: s64) -> s64 {
    if b == 41  return 40;    // )  matches  (
    if b == 93  return 91;    // ]  matches  [
    if b == 125 return 123;   // }  matches  {
    return -1;
}

is_opener :: (b: s64) -> bool {
    return b == 40 || b == 91 || b == 123;
}

// True if every bracket in `s` is closed in the right order.
balanced :: (s: string) -> bool {
    stack: Array(s64);         // a fixed-capacity array used as a stack

    i := 0;
    while i < s.count {
        b := byte_at(s, i);
        if is_opener(b) {
            if !push(*stack, b) {
                return false;   // nested deeper than the stack can hold
            }
        } else {
            want := opener_for(b);
            if want != -1 {
                top, ok := pop(*stack);
                if !ok || top != want {
                    return false;
                }
            }
        }
        i = i + 1;
    }
    // `stack.count == 0` rather than `is_empty(*stack)`: both String and Array export
    // an `is_empty`, so the flat import merge makes the bare name ambiguous (E0211).
    return stack.count == 0;    // nothing left open
}

report :: (s: string) {
    if balanced(s) {
        print("balanced:   ");
    } else {
        print("unbalanced: ");
    }
    print(s);
    print("\n");
}

main :: () {
    report("(a[b]{c})");
    report("([)]");
    report("(((");
}
```

Output:

```
balanced:   (a[b]{c})
unbalanced: ([)]
unbalanced: (((
```

## How it works

**The stack is just an `Array(s64)`.** A freshly declared `stack: Array(s64)` is zeroed —
its element buffer and its `count` both start empty — so there is nothing to initialise. We
push the *opener byte* onto it (`push` returns `false` if the array is full, which for this
fixed-capacity type means we nested deeper than it holds).

**Closing a bracket pops and checks.** For a byte that `opener_for` recognises as a closer,
we `pop`. `pop` returns **two values** — the popped element and a flag that is `false` when
the stack was empty. This is Jairs' error model in miniature: instead of an exception on an
empty pop, you get `(_, false)` and decide what to do. Here, an empty stack or a mismatched
opener both mean the string is unbalanced.

**Non-bracket bytes are ignored.** A letter's byte code is neither an opener nor a recognised
closer, so both `is_opener` and `opener_for` decline it and the loop moves on.

**Balanced means the stack ends empty.** If we finish the scan with anything still open, the
stack's `count` is non-zero and the string is unbalanced. We write `stack.count == 0` rather
than `is_empty(*stack)` for a reason worth seeing: *both* `String` and `Array` export a
procedure called `is_empty`, and Jairs' [flat import merge](/language/modules/#importing)
makes the bare name ambiguous — using it here is the compile error `E0211`. Reading the field
directly sidesteps the collision, and the ambiguity itself is exactly the hazard the
[unused-import warning](/language/modules/#unused-imports-are-a-warning) exists to keep in
check.

## Why two-value returns, not exceptions

Notice that nothing here *throws*. Popping an empty stack is an expected situation — the
input `([)]` reaches it legitimately — so it is a value the caller checks, not an error that
unwinds the stack. This is the whole of Jairs' error handling: see [Errors and
traps](/language/errors-and-traps/#errors-are-values). A trap is reserved for the
*unrecoverable* — and indexing this array out of range would be one, which is exactly why the
capacity-guarding `push` returns a flag rather than letting you overflow it.

## What it demonstrates

- `Array` used as a stack, with `push` / `pop` / `is_empty`.
- Two-value returns as the alternative to exceptions.
- Byte-level string scanning, as in [the word counter](/in-practice/word-count/).

Next: [memoising with a hash map](/in-practice/memoized-fib/).
