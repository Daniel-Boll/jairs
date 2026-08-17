---
title: Build scripts (BUILD_OUTPUT)
description: A compile-time constant that names the build artefact — the first time anything in a Jairs file influences the build rather than the program.
sidebar:
  order: 66
---

`BUILD_OUTPUT` is a compile-time constant that names the build artefact (ADR-0102). It is the first time
anything in a Jairs file has influenced the **build** rather than the program. The driver reads it, so a
declared name — even a *computed* one — decides what `jr build` calls the executable.

```jr
#import "Basic";

/// The script: an ordinary compile-time procedure whose result names the artefact. Nothing marks it as
/// special — it is `#run` at the constant's declaration that makes it a build script, which means any
/// procedure can be one.
choose_name :: () -> string {
    return "note_driven_app";
}

/// The declaration the driver reads. A `#run` here, so the name is *computed*; `BUILD_OUTPUT :: "app";` is the
/// same feature with the computation elided.
BUILD_OUTPUT :: #run choose_name();

main :: () {
    // The program is entirely unaffected by the constant above: it is compile-time data for the driver, so it
    // contributes no code and this exits on its own arithmetic.
    n := 8;
    exit(n + 1);
}
```

## A declared fact, not a call

`BUILD_OUTPUT :: #run choose_name();` is read through the driver's constant-evaluation path, so a
**computed** name works exactly as a literal does — `BUILD_OUTPUT :: "app";` is the same feature with the
computation elided. There is no second path for the computed case.

It is a declared *constant* rather than a `set_build_output("app")` call, and that is deliberate. A call
has to *happen*, so its effect would depend on evaluation order and on the script being reached at all; a
declared constant is a **fact about the file**. Order-dependent configuration is precisely the failure
mode makefiles are notorious for, and importing it into their replacement would be strange to do on
purpose. The name is screaming-case because it *is* a constant, and one the compiler knows by name: a
lowercase `build_output` would read like an ordinary local and give no hint the driver is watching it.

## `-o` still wins

A person at a terminal passing `-o` is overriding deliberately, and a script that could silently defeat
the flag would make the flag untrustworthy — and would make the artefact's name unpredictable from
reading the file alone. So the command-line flag beats the declared constant.

## What the program can and cannot observe

The *program* is unaffected: `BUILD_OUTPUT` type-checks, formats, appears in no MIR, and changes nothing
about what `main` does — the example exits on its own arithmetic (9). That is the only property a corpus
file can pin, because the driver's behaviour is not something a program can observe from inside itself.
The driver behaviours — that the declared name is used, that `-o` beats it, and that a file *without* the
constant still defaults to its own file stem — are pinned by separate driver integration tests.

## What this is not

This is **not** a build *system*. There is no dependency graph, no incremental rule, and no way to build
several artefacts. One build decision — the name of the output — has moved inside the language. The
honest claim is that a build script *can* replace one makefile responsibility, not that it replaces the
makefile in general.
