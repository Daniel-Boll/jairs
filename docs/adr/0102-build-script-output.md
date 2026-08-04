# ADR-0102: A build script names its own artefact — `BUILD_OUTPUT`, a declared constant the driver reads

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W6 sub-wave 5.** PLAN §2.1's claim is that a build script replaces the makefile. Sub-waves 1–4 gave a
  metaprogram data, a reader, a query and code generation — all of which act on the *program*. This is the
  first thing that acts on the **build**: a compile-time value the driver obeys.

## Context

What "a build script replaces the makefile" needs, concretely, is a way for a compile-time program to **tell
the driver** something the driver then acts on. A `#run` could already compute a value and splice code, but
nothing it computed ever reached `jr build` — so every build decision still lived outside the program, which is
exactly what a makefile is.

The smallest such decision, and the one a makefile most obviously owns, is **the name of the artefact**.

## Decision

### 1. `BUILD_OUTPUT` is a declared constant, not an intrinsic call

```jai
choose :: () -> string { return "named_by_script"; }
BUILD_OUTPUT :: #run choose();     // or simply  BUILD_OUTPUT :: "app";
```

`jr build` reads it through `declared_build_output`, which finds the item by name and asks `file_consts` for its
value — so a **computed** name works exactly as a literal does. There is no second path for the computed case,
which is what ADR-0073 bought and is worth collecting on.

**A declared constant rather than `set_build_output("app")`**, which is closer to Jai's spelling. A *call* has
to happen, so its effect depends on evaluation order and on the script being reached at all; a declared
constant is a **fact about the file**. Order-dependent configuration is the failure mode makefiles are
notorious for, and importing it into their replacement would be a strange thing to do deliberately.

**`None` is not an error** — no such constant, not a `string`, or it did not evaluate. All three mean "the
driver decides". A non-`string` is already a type error at its own declaration, and reporting it again from the
driver would say the same thing twice in a worse place.

### 2. `-o` wins

A person at a terminal is overriding on purpose, and a build script that could silently defeat `-o` would make
the flag untrustworthy. The reverse precedence would also make a script's own output name unpredictable from
reading the file — you would have to know how it was invoked.

### 3. Two rejected shapes, and what would make each right

- **A whole `Build_Options` struct returned from `#run build()`** — the most Jai-like. Reading it generically
  needs `Type_Info`'s field walking (still deferred, ADR-0078); reading it by hard-coded fields is ADR-0075
  §2's validated-declaration dance for a much larger struct. Right once there are enough options to justify a
  struct. **Two is not enough**, and building the struct first would be designing a container before knowing
  what goes in it.
- **The driver running the script as a separate program that prints a manifest** — how many build systems
  work, and it needs no language surface. But it makes the build two phases with a text protocol between them,
  which is a build-system design rather than a language feature, and this wave is about the language.

## Consequences

- **A program can decide its own artefact name**, which is the first time anything in a Jairs file has
  influenced the *build* rather than the program. Three integration tests pin it: the declared name is used,
  `-o` beats it, and a file without it still defaults to its own stem — that last one so adding the query
  cannot silently change what every existing program builds to.
- **No new diagnostic code and no new query.** `declared_build_output` is a plain function over `file_hir` and
  `file_consts`, both of which the driver already calls.
- **Asserted on the file that appears**, not on a message, because the claim is that the driver *acted* on a
  value the program computed. A test that checked stdout could pass while writing the wrong file.
- **Deferred, each with a reason**: a script *adding* a module path (wants a list-valued constant and a
  decision about append-versus-replace); a script setting `--no-bounds-check` (a **safety** setting, and
  letting a file quietly disable checks for its own build deserves its own argument, the same instinct
  ADR-0058 §3 had about `#no_abc`); plugin hooks; workspaces.
- **What this is not** is a build *system*: there is no dependency graph, no incremental rule, no way to build
  several artefacts. It is one decision moved inside the language, and the honest claim is that PLAN §2.1's
  sentence is now true of something rather than true in general.
