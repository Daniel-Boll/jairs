# ADR-0122 — A declared `BUILD_OUTPUT` is confined to the working directory

**Status:** Accepted
**Date:** 2026-08-07
**Amends:** ADR-0102, which introduced `BUILD_OUTPUT` and did not constrain its value.

## Context

ADR-0102 let a program name its own artefact: `BUILD_OUTPUT :: #run choose_name();`. The
value is a `string` constant, so it may be **computed by arbitrary compile-time code in the
file being compiled**.

The audit at `354d900` ([`docs/assessment-2026-08-07.md`](../assessment-2026-08-07.md),
finding F4) traced that value from the declaration to the filesystem and found **nothing
checked it** at any point: `jr-db/src/build.rs` returns the interned string verbatim,
`jr-cli/src/commands/build.rs` wrapped it in `PathBuf::from`, and `jr-link` wrote to it.

Two consequences, neither subtle:

```jairs
BUILD_OUTPUT :: "../../.git/hooks/pre-commit";
```

`jr build` wrote an executable to a path **git runs on the next commit**. An absolute path
wrote anywhere the invoking user could. This turns "I compiled a file someone sent me" into
"I ran their code", and the only action required of the victim is the one the tool is for.

```jairs
BUILD_OUTPUT :: "-Wl,--version";
```

`jr-link` passes the object path as `cc`'s **first positional argument** and the output as
its `-o` value, so a leading `-` is read as a **flag** rather than a path — argument
injection into the linker.

The framing that matters: for a compiler, **the source is attacker-controlled in the
ordinary case**. Compiling code one did not write is not an unusual scenario, it is the
scenario. ADR-0102 documented *naming the artefact*; it did not document writing anywhere on
the filesystem, so this is a gap between the decision and its implementation rather than a
trade the project had made and stated.

Note what is *not* the issue: `jr-link` builds its command with separate `.arg()` calls and
never a shell, so there was no shell injection — the exposure was argument injection and an
unconstrained path.

## Decision

### 1. A declared name must stay inside the working directory

`confined_output` refuses, with a sentence naming the cause:

- an empty name, or one naming only `.`;
- an **absolute** path (a `Prefix` or `RootDir` component);
- any `..` component;
- a leading `-`;
- an interior NUL byte — Rust strings admit one and the OS boundary does not, so it is
  rejected here where the message can say so rather than surfacing as an opaque io error.

A relative **subdirectory** stays legal (`build/app`). Naming one is an ordinary thing for a
build script to do, and forbidding it would push people back to `-o` for a normal case.
Confinement is by rejecting what *leaves* the directory, not by flattening the name.

The refusal is a driver-level error with exit `2` — "accepted but could not be built" —
because that is what happened: the program checked, and the artefact it asked for is not one
this command will write.

### 2. Only a *declared* name is confined; `-o` is not

An explicit `-o` is a person at a terminal saying where they want the file. Confining it
would make the flag less useful than a shell redirection, and it is the same reasoning that
makes `-o` beat the declaration in the first place (ADR-0102 §2): the human is overriding on
purpose.

So the boundary is exactly "a value the *program* chose" versus "a value the *operator*
chose", which is also the trust boundary. `BUILD_OUTPUT` is data from the artefact under
compilation; `-o` is an instruction from the person compiling it.

### 3. `jr-link` does not trust its caller

`not_a_flag` prefixes `./` to any path it hands `cc` or `codesign` that begins with `-`.
`./-x` and `-x` name the same file, so this is behaviour-preserving for the filesystem while
removing the ambiguity for the argument parser.

This is deliberately **redundant** with §1 for a declared name, and load-bearing for an
explicit `-o`, which §2 leaves unchecked. More to the point, a linker driver should not
depend on which of its callers validated what — `jr-link` is a one-function module with no
internal dependencies, and keeping it correct in isolation is why that is worth having.

Rejected: *validating in `jr-db::declared_build_output` and reporting a diagnostic at the
declaration.* It reads better — the span would point at the constant — but it puts a
*driver* policy into a query that the LSP also calls, and `jr check` has no opinion about
where a build writes. A file whose `BUILD_OUTPUT` is unusable is still a valid program; only
`jr build` cares.

## Consequences

`jr build` on a hostile file refuses with a message naming the cause instead of writing an
executable outside the working directory. `-o` is unchanged. A declared name in a
subdirectory still works.

Test count 990 → 1001: seven unit tests on the predicate, two end-to-end refusals, and two
on `not_a_flag`. The escape test asserts the file **does not appear** as well as the exit
code, because a refusal that still wrote the artefact somewhere would pass an
exit-code-only test.

**What this does not fix.** `#system_library` names still reach `cc`'s argv as `-l{name}`,
which on GNU ld admits `-l:/path/to/file.so` — a route to linking an attacker-supplied
shared object. That is its own change, and it wants a decision about what a library name may
contain rather than a path check. The `cc` binary is still located through `PATH`. And the
audit's security scope was only partly covered — its assessor failed twice, so the VM's
memory region, `Any`/proc-pointer forgery, comptime-FFI-gate bypasses and `jr-lsp` path
handling remain unexamined, and are recorded as such in the assessment's coverage section.
