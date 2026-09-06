---
title: "os() and per-OS declarations"
description: "`os()` as a compile-time value the language folds rather than an #if, a per-OS constant chosen through #run, and a per-OS declaration generated through #insert #run."
sidebar:
  order: 47
---

Jairs has no `#if`. Before ADR-0180 the compiler had no notion of an operating system at all — no constant, no build setting a library could read — and `modules/Time` carried a comment about a clock id being macOS-only that it could not guard. `os()` closes that gap as a compile-time **value** rather than as conditional compilation: choosing between two things becomes choosing between two ordinary values, which the language already knows how to do.

## os() folds to an enum member

```jr
os_code :: () -> s64 {
    if os() == Operating_System.MACOS {
        return 1;
    }
    if os() == Operating_System.LINUX {
        return 2;
    }
    return 4;
}

// A `switch` over the enum, which is exhaustiveness-checked (ADR-0067). Its answer must agree with
// `os_code`'s chain of `if`s — two spellings of one decision, so a compiler that folded `os()` to two
// different values in two places would be caught.
os_code_switched :: () -> s64 {
    switch os() {
        case .MACOS;
            return 1;
        case .LINUX;
            return 2;
        case .WINDOWS;
            return 4;
    }
    return 0;
}
```

`os()` in a body compares against a member of `Basic.Operating_System`, the same enum `switch` sees, so a `switch os() { … }` is checked for exhaustiveness — "this program does not handle Windows" is a compile error here, not a wrong branch chosen silently at run time.

## A per-OS value at file scope, through #run

```jr
HERE :: os();
VIA_RUN :: #run os_code();
```

`os()` reads directly into a file-scope constant with no `#run` needed, and the established idiom for anything that needs a *computation* per OS is a procedure a `#run` evaluates at file scope — the same shape `modules/Random`'s `GOLDEN :: #run golden_seed();` already used before `os()` existed to feed it.

## The library's own use: a per-OS clock id

```jr
/// `CLOCK_MONOTONIC`, selected for the target (ADR-0181 §1).
monotonic_clock_id :: () -> s64 {
    if os() == Operating_System.MACOS {
        return 6;
    }
    if os() == Operating_System.LINUX {
        return 1;
    }
    return 0;
}

CLOCK_MONOTONIC :: #run monotonic_clock_id();
```

`modules/Time`'s clock id used to be a bare `6` — macOS's number — with a comment naming the problem and no answer. `os()` is deliberately not `#if`: the number is chosen by an ordinary procedure a `#run` evaluates, so no item-level conditional compilation was needed at all. Windows gets `0`, `CLOCK_REALTIME`'s value, because Windows has no `clock_gettime` and there is no correct number to name — a plausible-looking constant for a call that cannot happen would be the worse answer (ADR-0181).

## Generating a whole declaration: #insert #run

```jr
// The library declaration, chosen per OS at compile time. This is the whole per-OS surface of the
// module: every binding below names `gl` and does not care which of the three it turned out to be.
#insert #run gl_library();
```

```jr
gl_library_for :: (target: Operating_System) -> string {
    if target == Operating_System.MACOS {
        return "gl :: #framework \"OpenGL\";";
    }
    if target == Operating_System.LINUX {
        return "gl :: #system_library \"GL\";";
    }
    return "gl :: #system_library \"opengl32\";";
}

gl_library :: () -> string {
    return gl_library_for(os());
}
```

`modules/GL` needs a different **library** per OS, not just a different value — macOS ships OpenGL as a framework, so it needs `-framework OpenGL` where Linux and Windows need an ordinary `-lGL`/`-lopengl32`. A computed operand cannot be evaluated before `#system_library` needs a bare literal, which is a real cycle; generating the declaration's *text* at compile time and inserting it as a statement side-steps it, because by the time the compiler resolves the library, the operand already is a literal. This needed two compiler changes: `#insert` becoming a file-scope directive able to produce declarations, not only statements (ADR-0184), and `jr-link` gaining a second link-argument form for `-framework` (ADR-0183).

`gl_library_for` takes the OS as a parameter rather than reading `os()` itself, and that split is deliberate rather than stylistic: a generator that reads `os()` internally has exactly one executable path per machine, so two of its three branches would be text no test could ever run. Parameterised, all three strings are checked on whatever host runs the test, and `gl_library()` — which really drives the `#insert` — is asserted to agree with `gl_library_for(os())` for that host.

See also [Book I — The Jairs Language](/language/introduction/).
