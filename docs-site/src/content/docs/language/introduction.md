---
title: Introduction
description: What Jairs is, the values that shape it, and how to read this book.
sidebar:
  order: 1
---

Jairs is a Jai-inspired systems programming language, compiled by a hand-written,
error-recovering compiler written in Rust. It is a language for programs that care about
memory layout and machine cost: there is **no garbage collector**, **no RAII**, and **no
exceptions**. Memory management is explicit and allocator-driven, control flow is ordinary
and visible, and errors are values.

This book is the guided tour. It is written to be read front to back — each chapter assumes
the ones before it — and by the end you will have seen the whole language: its types, its
control flow, how it manages memory, how it runs your code at compile time, and how it lets
a program write more of itself. If you would rather learn by scanning small isolated
programs, [Book II — Jairs by Example](/by-example/) covers the same ground one feature per
page. If you want to see the language do real work, [Book III — Jairs in Practice](/in-practice/)
walks through complete programs.

## The design values

Everything in Jairs falls out of a short list of commitments.

| Value | What it means in Jairs |
| --- | --- |
| **No GC** | Memory is managed explicitly; there is no tracing collector and no reference counting baked into the language. |
| **No RAII** | Objects have no implicit destructors. Cleanup is explicit, with `defer`. |
| **No exceptions** | Errors are ordinary values, returned and handled explicitly — the model is Jai's multiple return values. |
| **Explicit allocators** | Allocation goes through an allocator carried by an implicit `context`. A callee allocates without knowing *which* allocator it got. |
| **Compile-time execution** | Ordinary code can run at compile time in a bytecode VM via `#run`. The VM and the native back end execute the *same* intermediate representation, so compile-time and run-time results cannot silently disagree. |
| **Overflow is an error** | Integer overflow always traps — never wraps silently, never differs between debug and release. Explicit `+% -% *%` operators exist for code that wants modular arithmetic. |
| **Fast compilation** | Cranelift is the first back end (far faster to compile than LLVM); the parser is hand-written for speed and diagnostic quality; analysis is incremental. |

If a feature in this book ever seems surprising, it is almost always one of these values
being applied consistently. The chapter on [operators](/language/operators-and-overloading/)
is the clearest example: the reason `1 + 1.5` is a *type error* and the reason a shift by
too many bits *traps* are the same reason — Jairs refuses to guess.

## Two engines, one language

A Jairs program can be executed two ways, and this shapes how the whole language is built:

- `jr run file.jr` executes it in a **bytecode virtual machine**. This is also the engine
  that runs your `#run` blocks at compile time.
- `jr build file.jr -o out` compiles it through **Cranelift** to a native executable.

Both engines consume the *same* mid-level IR that the compiler produces. Because of that,
the two are expected to agree — and a differential test in the compiler asserts they agree
byte for byte: same output, same exit status, even the same reported location when a program
traps. When you see an example in this book end with `exit(0)` on success, that exit status
is being compared across both engines behind the scenes.

This is why Jairs can promise that compile-time execution and run-time execution never
diverge. It is not a convention; it is a checked property.

## How to read this book

- **The examples are real.** Every program shown is drawn from, or built to match, the
  compiler's own corpus of test programs. If the book and the compiler ever disagree about
  syntax, the compiler is right.
- **Absence is stated, not hidden.** Jairs is pre-alpha and deliberately small. Where a
  feature does not exist yet, the text marks it <span class="jairs-status absent">absent</span>
  and says which development *wave* introduces it. Anything shown without such a marker
  actually runs today.
- **Traps are a feature.** Jairs would rather stop your program with a clear location than
  let it compute a wrong answer. You will see the words "traps" a lot; each time, it means a
  well-defined, located run-time failure — not undefined behaviour.

## A first program

Here is a small program that touches a surprising amount of the language: a module import, a
struct, a procedure, a constant computed at compile time, field access, an `if`, a `while`,
and Jairs' prefix-address / postfix-dereference pointer syntax.

```jr
#import "Basic";

Point :: struct {
    x: s64;
    y: s64;
}

MESSAGE  :: "hello from Jairs\n";
COMPUTED :: #run add(2, 3);          // add(2, 3) runs while compiling; COMPUTED is 5

add :: (a: s64, b: s64) -> s64 {
    return a + b;
}

main :: () {
    p: Point;                        // a struct local, zero-initialised
    p.x = 4;
    p.y = COMPUTED;

    sum := add(p.x, p.y);            // := infers the type of `sum`
    if sum > 5 {
        print(MESSAGE);
    }

    i := 0;
    while i < 3 {
        i = i + 1;
    }

    ptr := *sum;                     // prefix * takes the address of `sum`
    print_int(ptr.*);                // postfix .* reads through the pointer
    print("\n");
}
```

A few things to notice, each of which gets a full chapter later:

- **Declarations use `::`, `:=`, or `: T`.** `add`, `Point`, `MESSAGE` and `COMPUTED` are
  all *constants* — introduced with `::`. `sum := …` infers a variable's type; `p: Point;`
  gives one explicitly. There is no `let`, `const`, `fn` or `struct` keyword: a procedure
  and a struct are just constants whose value happens to be a procedure or a type.
- **`print` and `print_int` are not built in.** They come from `modules/Basic`, which is
  itself written in Jairs and reaches the operating system's `write` through the foreign
  function interface. The standard library is Jairs code, all the way down to the syscall.
- **`#run add(2, 3)` runs at compile time.** The same `add` you call at run time is executed
  by the compile-time VM to produce the constant `5`. No separate metalanguage.

Run it both ways and you will get identical output:

```sh
jr run hello.jr      # prints "hello from Jairs" then 5
jr build hello.jr -o hello && ./hello
```

## Where to go next

The next chapter, [Values and types](/language/values-and-types/), starts at the bottom: the
integers, floats, booleans, strings and pointers every Jairs program is built from, and the
rules Jairs applies to conversions between them. From there the book works upward through
declarations, procedures, control flow, aggregates, the type system, memory, and finally the
features that make Jairs *Jai*-inspired rather than merely C-like — compile-time execution,
reflection, polymorphism, and metaprogramming.
