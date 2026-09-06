---
title: Qualified imports
description: "`Alias :: #import \"M\";` then `Alias.name`, merging nothing into a file's scope — the prerequisite for a program that imports two modules exporting the same name."
sidebar:
  order: 46
---

`#import "Name"` merges a module's exported declarations into a file's namespace, flat — which is why `Window` and `File` cannot both be imported bare into the same program: both export `open`. ADR-0179 added a second form: `Alias :: #import "M";`, after which the module's names are reached only as `Alias.name`, merging nothing. This is a prerequisite for a games book, not a nicety, since drawing a window and loading a level from disk in the same file needs exactly this.

## Aliased, so nothing collides

```jr
#import "Basic";
String :: #import "String";
File :: #import "File";

// A qualified type in a parameter. The argument is a local declared `File.File` in `main`.
descriptor_of :: (f: *File.File) -> s64 {
    return f.fd;
}

main :: () {
    n := 0;

    // A qualified value, in a call. `byte_at("abc", 2)` is `c`, 99.
    if String.byte_at("abc", 2) == 99 {
        n = n + 1;
    }
    // A qualified constant.
    if File.READ_WRITE == 2 {
        n = n + 4;
    }
```

`String :: #import "String";` binds the alias `String` to the module without merging any of its names into this file — so a value (`String.byte_at`), a constant (`File.READ_WRITE`), and a type (`File.File`) are all reached the same way, through the alias. A qualified type resolves against that module's own signatures rather than through the flat merge, which is what makes a type reachable at all in a module that merged nothing in.

## One type, two spellings

```jr
f: File.File;
f.fd = -1;
if descriptor_of(*f) == -1 {
    n = n + 8;
}
// The same pointer handed to the module's own procedure, whose parameter is spelled bare inside
// its own file. Two spellings, one type.
if !File.is_open(*f) {
    n = n + 16;
}
```

`File.File` here and the bare `File` inside `File`'s own module intern to the same `PoolId` — the same reason a pointer declared `File.File` in this file can be handed to `descriptor_of`, whose own parameter reads `*File.File`, and to `File.is_open`, whose parameter reads `*File` from inside the module that owns it.

## The collision it exists for

Two bare `#import`s that both export one name are refused with E0211, *"ambiguous name `blend`: provided by multiple imported modules"* — the fixture `Colors :: #import "Colors"; Palette :: #import "Palette";` above each name a `blend`, and aliasing both is what makes the file writable at all: an aliased import merges nothing, so there is nothing left to be ambiguous, and both procedures are reachable, each under the module that declares it.

## Bare when the alias would repeat the name

```jr
Window :: #import "Window";
Input :: #import "Input";
Simp :: #import "Simp";
GL :: #import "GL";
Image :: #import "Image";
Time :: #import "Time";
String :: #import "String";
// `UI` is imported **bare**, not aliased, because its type is called `UI` too: `#import "UI";` brings
// `UI`, `button`, `begin_frame` and `feed` into this file's scope, and a local `ui: UI` is the state.
#import "UI";
#import "Basic";
```

`modules/UI` names its own type `UI`, so aliasing it (`UI :: #import "UI";`) would shadow the type with the module. `modules/UI` itself aliases its own two imports, `Input :: #import "Input"; Simp :: #import "Simp";`, for the same collision reason — `Window`'s eleven unprefixed exports are exactly what a bare `Input` or `Simp` used to have to dodge, before ADR-0179 gave it an alias to hide behind instead. A program mixes both forms freely: alias where a name would collide, leave bare where the module's own name is the type you want in scope.

See also [Book I — The Jairs Language](/language/introduction/).
