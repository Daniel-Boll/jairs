# ADR-0183: `jr-link` learns a second argument form — `-framework`

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **The blocker the Simp plan named wrongly**, found by probing rather than by reading. The plan ruled
  OpenGL out because a per-OS *library name* needs a computed `#system_library` operand and the query order
  forbids one. That cycle is real. It is also **not the first blocker**.

## Context

### Two commands, and they settle it

```
$ cc probe.c -o probe -lOpenGL           ld: library 'OpenGL' not found   (exit 1)
$ cc probe.c -o probe -framework OpenGL                                   (exit 0)
```

OpenGL on macOS is a **framework**, not a dylib on a search path. And `jr-link`'s entire flag vocabulary was
two lines:

```rust
for path in request.library_paths { command.arg(format!("-L{}", path.display())); }
for library in request.libraries  { command.arg(format!("-l{library}")); }
```

So **a perfect per-OS name mechanism would have emitted `-lOpenGL` and failed.** The library name was never
the first thing in the way; the missing argument form was. That is a smaller and more tractable blocker than
the plan described, and it is the sixth premise of that plan not to survive contact — recorded in PLAN §7
with the other five.

### Why this is worth its own ADR

Because the fix is not "add a flag". A link form has to travel from the *declaration* that names it to the
driver that emits it, through the pool, the MIR plan, both back ends and the build output — and if any hop
carries only a name, the driver has to guess. Guessing is the one thing that must not happen here: the two
forms are not interchangeable and neither is a fallback for the other.

## Decision

### §1 — The form is part of the library's identity, not a flag beside it

`#framework "OpenGL"` is a **new directive**, parallel to `#system_library "SDL2"`, and `LinkKind` is
interned *into* the pool's `ForeignLibraryValue`:

```rust
ForeignLibraryValue(StrId, LinkKind)   // was: ForeignLibraryValue(StrId)
```

**A modifier on `#system_library` was rejected.** A framework is a different kind of linkable thing, not a
style of naming one, and a program that wrote one form meaning the other should not silently get a search of
the wrong kind.

**Interning the kind makes `#system_library "OpenGL"` and `#framework "OpenGL"` two different values**, which
is right and is the property a test pins: if they interned equal, a program naming the framework could be
handed the library's `PoolId` and linked with the flag that does not resolve.

An enum rather than a `bool`, for the house reason — a third form (a full path, `-l:libfoo.so.1`, a Windows
`.lib`) becomes a compile error at every site that must decide what it means. The house rule earned its keep
immediately: adding the field turned **nine** crates' pattern sites into compile errors, each of which had
to be looked at.

### §2 — No inference, and no fallback

`jr-link` matches on the kind and emits `-lNAME` or `-framework NAME` — two arguments for the second, since
`-frameworkOpenGL` is not a thing `ld` accepts.

- **No inference from the name.** The compiler cannot know which a name means.
- **No `-l` fallback after a failed `-framework`**, or vice versa. That would make `#system_library "SDL2"`
  on macOS try a framework that does not exist before finding the dylib that does — a link that succeeds for
  a reason the source did not state, which is what ADR-0019 §4's "refuse rather than guess" rule exists to
  prevent.

The source says which. And after ADR-0184 the *declaration itself* is generated per OS, so no file carries a
form that is wrong on another platform — which is the same asymmetry ADR-0163 §2 drew for `-L`: a path is a
property of the machine, not of the program.

### §3 — `jr-link` stays a leaf crate

It declares its **own** `LinkKind` and `LinkLibrary`, and `jr-cli` converts. `jr-link` has *no
dependencies at all* — that is the seam ADR-0009 drew, and it is why the linker can be read and tested
without the compiler. The duplication is two variants; the alternative is a dependency on `jr-pool` for an
enum with two cases.

The conversion is exhaustive, so a third link form is a compile error at the driver rather than a silent
`-l`.

### §4 — A framework name cannot become a flag

`-framework` takes its name as a **separate argument**, so unlike `-lNAME` — where the name is concatenated
and a leading `-` is harmless — a name beginning with `-` would reach `cc` as an option of its own:
`#framework "-rpath"` would be a linker flag the source never asked for.

The existing `not_a_flag` guard prefixes `./`, which is meaningless for a framework name. So a sibling guard
**empties** it instead, failing the link with `ld: framework not found` rather than doing something. A
refusal a reader can see beats an argument they cannot.

## Consequences

- macOS frameworks are reachable: `CoreFoundation`, `OpenGL`, `Cocoa`'s C surface. **Cocoa proper is still
  out**, because every Objective-C call goes through `objc_msgSend`, which is variadic and blocked upstream
  in Cranelift (ADR-0162 §2). The link form was never that blocker either.
- `-framework` on Linux or Windows fails, correctly. Nothing guards against writing it there, because
  ADR-0184 makes the declaration per-OS generated and a hand-written one is the author's claim.
- **A full-path form is still missing**, so a library outside every `-L` path is unreachable. Recorded rather
  than left to be rediscovered; it is the same two-line shape as this.

## Verification

- **`a_framework_links_where_the_library_form_does_not`** asserts the pair, and the *negative* half is the
  one that matters: `#framework "CoreFoundation"` builds, links, and its symbol resolves at run time
  (exit 42), while `#system_library "CoreFoundation"` — the same name, the other form — **fails to link**. A
  success-only test would pass on a compiler that quietly emitted `-l` for both, if the name happened to
  resolve; this one cannot.
- **`a_per_os_library_is_chosen_by_comptime_code_and_linked`** reads `otool -L` on the built binary and
  asserts `OpenGL.framework` is recorded. The exit code alone would not prove it: that program calls no GL
  entry point, so a compiler that dropped the library would still exit 42.
- **`a_library_and_a_framework_of_one_name_are_different_values`** pins the interning in `jr-pool`, including
  that `foreign_library_kind` answers `None` for a non-library `PoolId` rather than defaulting to
  `Library` — which is precisely the wrong guess for a framework.
- All seven gates green.
