---
title: "Hello, imports & foreign"
description: The slice's exit-criterion program, importing a module, binding a foreign C symbol, and running code at compile time with `#run`.
sidebar:
  order: 17
---

This page ties the fundamentals together: a complete "hello" program, module imports, foreign
function bindings, and compile-time execution with `#run`.

## Hello, Jairs

```jr
#import "Basic";

Point :: struct {
    x: s64;
    y: s64;
}

MESSAGE :: "hello from Jairs\n";
COMPUTED :: #run add(2, 3);

add :: (a: s64, b: s64) -> s64 {
    return a + b;
}

main :: () {
    p: Point;
    p.x = 4;
    p.y = COMPUTED;

    sum := add(p.x, p.y);
    if sum > 5 {
        print(MESSAGE);
    }

    i := 0;
    while i < 3 {
        i = i + 1;
    }

    ptr := *sum;
    if ptr.* == 9 {
        print_line("arithmetic and pointers agree");
    }
}
```

This is the language's own exit-criterion program for the Jairs-0 slice: it must run in the
bytecode VM via `jr run`, compile to a native arm64 binary via `jr build`, produce **identical**
output either way, and receive hover, goto-definition, and diagnostics in the language server.
That is why it exercises so much at once — a struct, a constant, a compile-time computation, a
call, `if`, `while`, and pointers.

One honest limitation is called out in the file: there is no integer printing here. `print_int`
cannot be written in the Jairs-0 subset, because turning a digit into a byte needs an `s64`-to-`u8`
conversion, and `cast` is reserved until the wave labelled W1. The program prints only fixed
strings, through `print` and `print_line`, both provided by the imported `Basic` module.

## Importing a module

```jr
#import "Basic";

main :: () {
    print("imported\n");
}
```

`#import "Basic"` brings a module's declarations into scope. `Basic` is the standard prelude
module that supplies `print` and `print_line`; without the import, those names would not
resolve.

## Foreign bindings

```jr
// A foreign library binding. `#system_library` resolves through the
// platform's dynamic loader rather than a bundled archive.
libc :: #system_library "c";

// A foreign procedure has no body; it is declared and terminated with a
// semicolon. `#c_call` opts out of the implicit context parameter
// (ADR-0001) -- foreign procedures are always `#c_call` implicitly.
write :: (fd: s64, buf: *u8, count: s64) -> s64 #foreign libc "write";

// The Jairs name and the symbol name may differ.
os_exit :: (status: s64) #foreign libc "exit";
```

To call into C, you first name a library. `#system_library "c"` resolves through the platform's
dynamic loader (rather than a bundled archive), binding the constant `libc` to it.

A foreign procedure then has **no body**: it is declared with its signature, a `#foreign`
directive naming the library and the C symbol, and a terminating semicolon. Two details:

- The Jairs name and the C symbol name may differ — `os_exit` here binds the C `"exit"`.
- Foreign procedures are always `#c_call` implicitly, which opts out of Jairs' implicit context
  parameter (ADR-0001).

## Running code at compile time with `#run`

```jr
// `#run` evaluates an arbitrary expression at compile time in the bytecode
// VM. The result is interned as a compile-time value, indistinguishable
// from a literal.
COMPUTED :: #run add(2, 3);

add :: (a: s64, b: s64) -> s64 {
    return a + b;
}

// `#run` may also appear as a top-level statement, in which case it is
// executed for its side effects during compilation.
#run report();

report :: () {
}

main :: () {
    // Constant-folded before codegen ever sees it.
    total := COMPUTED + 1;
}
```

`#run` evaluates an expression at compile time in the same bytecode VM used by `jr run`. The
result is interned as a compile-time value, indistinguishable from a literal — so `COMPUTED`
above is exactly as if you had written the number `add(2, 3)` produces, and `total := COMPUTED +
1` is constant-folded before code generation ever sees it. `#run` may also stand as a top-level
statement, in which case it runs for its side effects during compilation.

See also [Book I — The Jairs Language](/language/introduction/).
