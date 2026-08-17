---
title: Modules
description: Imports, module visibility, and the foreign function interface.
sidebar:
  order: 13
---

A Jairs program is one or more modules. The module system is deliberately simple: **one
module is one file**, imports are flat, and cycles are allowed.

## Importing

```jr
#import "Basic";      // brings in modules/Basic
#import "Math";       // and Math

main :: () {
    print("hi\n");    // print comes from Basic
    r := sqrt(2.0);   // sqrt comes from Math
}
```

`#import "Name"` merges the named module's exported declarations into your file's namespace —
a **flat** import, so you write `print`, not `Basic.print`. Which file `"Basic"` resolves to
depends on the module search path; the language server's hover on an `#import` shows which
file it actually resolved to, precisely because the answer depends on configuration.

## What crosses a module boundary

Most things cross an import cleanly: **procedures, types, enum members, and the *values* of
constants** are all visible to an importer.

The main thing that does **not** cross yet is an imported struct's **fields**: you can hold a
value of an imported struct type and pass it around, but `using` on an imported struct is
refused, and a few field-level operations are restricted. This is why the standard library's
containers are provided as concrete `s64` instances (`Array(s64)`, `Map(s64, s64)`) rather
than as fully generic types — see [The standard library](/language/the-standard-library/).

Operator overloads **do** cross the boundary, which is what lets `Math`'s `Vector3 + Vector3`
work in your file. And an imported module's **own errors are now reported**: if a module you
import is itself broken, the diagnostic points at *its* source, rather than passing your build
only to fail cryptically inside an engine later.

## Visibility: scope directives

Declarations are **exported by default**. To keep something module-private, mark it:

```jr
#scope_module      // everything below is hidden from importers

helper :: () { … }

#scope_export      // back to exported
```

`modules/Basic` uses exactly this to hide its internal helpers (`put_byte`, `print_digits`)
while exporting `print` and `print_int`. A finer `#scope_file` is
<span class="jairs-status absent">absent</span> — indistinguishable from `#scope_module`
while a module is a single file — as is re-export.

## Unused imports are a warning

Jairs *warns* about an unused `#import` (`E0231`) — unlike Jai, which does not. The reason is
specific to the flat-merge model: an unused import silently enlarges the namespace every
identifier resolves against, and can turn a later declaration into an ambiguity error from a
module the file never actually uses. The warning is conservative: an import is flagged only
when nothing in the file uses any name it provides, in either expression or type position.

## The foreign function interface

The bottom of the standard library is the operating system, reached through `#foreign`:

```jr
libc :: #system_library "c";

write :: (fd: s64, buf: *u8, count: s64) -> s64 #foreign libc "write";
malloc :: (size: s64) -> *u8 #foreign libc "malloc";
```

A `#foreign` procedure has no body — it names a symbol in a system library. Foreign
procedures are `#c_call` (they get no `context`), and they run at **run time only**: a
`#foreign` call at compile time is refused, because a host pointer read through the VM's own
address space would be a plausible wrong value.

This is the sense in which "the standard library is written in Jairs": `print` is Jairs code
that calls `write`, which is a `#foreign` binding to the C library. There is a syscall at the
bottom, and everything above it is the language.

Floats can cross the FFI boundary too — passed in floating-point registers, as every real ABI
expects — which is how `Math` reaches libm for `sqrt`, `sin`, and friends.

## The shape of a module

A module is just a `.jr` file with declarations. `modules/Basic/module.jr` is a normal Jairs
file; so is any module you write. There is no manifest, no separate interface file, and no
build descriptor — the declarations, and the `#scope_*` directives among them, *are* the
module's interface.

Next: [Compile-time execution](/language/compile-time-execution/), where the language starts
to do things C cannot.
