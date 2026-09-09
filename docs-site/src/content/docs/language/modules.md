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

## Qualified imports

A flat import merges *nothing* when you write it under a name instead:

```jr
Window :: #import "Window";
File   :: #import "File";

main :: () {
    f, ok := File.open_read("data.txt");   // File.open_read, not Window's anything
    w := Window.create_window(640, 480, "demo");

    File.close(*f);      // File.close, not Window.close
    Window.close(*w);
}
```

`Window` and `File` both export `close`. A flat `#import` of both is unwritable — the second
import makes `close` ambiguous, and every call site would need to know which module's `close`
it meant with no way to say so. An aliased import merges nothing into your namespace at all,
so the collision never arises: `Alias.name` works in value position, and a qualified name
also works in *type* position (`f: Window.Event`), because a module that hid its types behind
the alias would not have solved the collision it exists for.

## What crosses a module boundary

Most things cross an import cleanly: **procedures, types, enum members, and the *values* of
constants** are all visible to an importer.

The main thing that does **not** cross yet is an imported struct's **fields** for `using`: you
can hold a value of an imported struct type and pass it around, but `using` on an imported
struct is refused. A *parameterised* struct now crosses cleanly too — `Array($T)` and
`Map($K, $V)` are declared once in their modules and instantiated by an importer as
`Array(s64)`, `Map(s64, s64)`, and so on. What stays concrete is the *procedures*: an
imported polymorphic procedure cannot be instantiated by its importer at all (E0268), so
`push :: (a: *Array($T), v: T)` would be uncallable from outside its own module regardless of
whether the struct it takes is generic. `Array`'s and `Map`'s procedures therefore still take
a concrete instance — `*Array(s64)`, `*Map(s64, s64)` — see [The standard
library](/language/the-standard-library/). `[..]T`'s own operations (in `List`) hit the same
wall from the other side: the *type* is structural and crosses for free, but its procedures
are concrete `[..]s64` for the identical E0268 reason.

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

`modules/Basic` uses exactly this to hide its internal helpers (`out_byte`, `out_u64`,
`format_any`, `format_into_out`, and the rest of the formatter)
while exporting `print` and its compatibility wrappers. A finer `#scope_file` is
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

### Aggregates cross the boundary too

A `#foreign` procedure can take and return a whole struct, not just scalars — as long as its
shape matches what the C ABI actually passes in registers:

```jr
Div_Result :: struct { quot: s64; rem: s64; }

ldiv :: (numer: s64, denom: s64) -> Div_Result #foreign libc "ldiv";
```

Two shapes cross: a **small integer aggregate** (every field a word, at most two words total)
and a **homogeneous float aggregate** (every field the same float type, at most four of
them — the shape a `CGRect`-style struct needs). A struct that mixes integer and float fields,
or is simply too big, is `Class::Memory` and stays refused (E0286): the two real ABIs
disagree about where a mixed struct's fields go, and guessing one would be silently wrong on
the other.

### #c_variadic — declaring a variadic C function

A C function declared with a trailing `...` needs `#c_variadic` on the Jairs side, naming
which parameters are the **fixed** ones:

```jr
printf :: (fmt: *u8, arg: s64) -> s64 #c_variadic #foreign libc "printf";
```

Declaring is legal today — a library author can bind `printf` now. **Calling** it with more
arguments than the fixed list is refused (E0289): Cranelift has no variadic calling
convention, and this project holds all three engines to one answer rather than letting
`jr build` fail where `jr run` would succeed. `creat` for a fixed-arity `open`, or
`vsnprintf` with a prepared buffer, are the way around it.

### #framework — the other link form

`#system_library` always emits `-lNAME`. On macOS, a framework — `OpenGL`, `CoreFoundation` —
is not a dylib on the search path, and needs a different linker argument:

```jr
gl :: #framework "OpenGL";
```

The two forms are not interchangeable, and neither falls back to the other: naming the wrong
one is a link failure (`ld: library 'OpenGL' not found` for `#system_library`, `ld: framework
not found` for `#framework` on a platform without one), never a silent wrong answer.

### Targeting an operating system

`os()` answers `Operating_System.MACOS`, `.LINUX` or `.WINDOWS` as a compile-time constant, and
`#insert` at file scope splices generated *declarations* — not just statements — into your
file. Together they are how a module picks a per-OS library or link form without a compiler
flag:

```jr
gl_library_declaration :: () -> string {
    if os() == Operating_System.MACOS   { return "gl :: #framework \"OpenGL\";"; }
    if os() == Operating_System.WINDOWS { return "gl :: #system_library \"opengl32\";"; }
    return "gl :: #system_library \"GL\";";
}

#insert #run gl_library_declaration();
```

The generated declaration is indistinguishable from a written one from that point on — it
resolves in any order and is visible to the rest of the file. A **computed** `#insert` at file
scope can only generate a library declaration this way; generating a constant, a procedure or
a struct needs the *literal* form (a plain string, no `#run`), because those need a signature
or a value from a compiler phase that has already run by the time a computed insert expands.

## The shape of a module

A module is just a `.jr` file with declarations. `modules/Basic/module.jr` is a normal Jairs
file; so is any module you write. A *module* has no manifest, no separate interface file, and
no build descriptor of its own — the declarations, and the `#scope_*` directives among them,
*are* the module's interface. A *project* is different: `jairs.toml`, written by `jr new` or
`jr init`, is a real project manifest — it is what lets `jr build`/`jr run` be invoked with no
arguments from inside a project directory.

Next: [Compile-time execution](/language/compile-time-execution/), where the language starts
to do things C cannot.
