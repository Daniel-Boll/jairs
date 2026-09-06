---
title: Metaprogramming
description: Macros, instantiation predicates, argument baking, and generating code from note metadata.
sidebar:
  order: 17
---

This chapter collects the features that let a program shape its own code: `#expand` macros,
the `#modify` predicate that constrains an instantiation, `#bake_arguments` specialisation,
and the `@note`-driven code-generation loop that Jai calls its superpower. All of it builds on
[compile-time execution](/language/compile-time-execution/).

## #expand macros

A procedure marked `#expand` is a **macro**: a call **splices its body into the caller's
scope** rather than calling it. That means the body sees, and can modify, the caller's locals
— deliberately unhygienic, matching Jai:

```jr
add_to_total :: (x: s64) #expand {
    total = total + x;         // `total` is the CALLER's local
}

double :: (x: s64) -> s64 #expand {
    return x * 2;              // in expression position, `return` assigns the result
}

main :: () {
    total := 0;
    add_to_total(10);          // splices `total = total + 10;` here
    add_to_total(6);

    a := double(21);           // a == 42, via a generated result local
    c := double(4) + double(5);// two splices in one expression coexist
}
```

Each argument is bound **once** (via a generated `name := arg;` prelude), so a side-effecting
argument isn't re-evaluated per use. The compiled MIR contains no calls at all — every macro
body is inlined at its site.

Refused by design: an **early `return`** in a macro (only a tail `return` is allowed, meaning
"the result"), a **void** macro in expression position, and a **cross-file** macro call.

## #modify — constrain an instantiation

A `#modify` block is a compile-time **predicate over an instantiation**: it runs while the
template is being instantiated, and a `false` **refuses** that instantiation. It lets a
template state its requirements in code instead of in a comment:

```jr
only_s64 :: (x: $T) -> T #modify {
    return type_info(T).id == type_info(s64).id;   // accept only T = s64
} {
    return x;
}
```

Instantiating `only_s64` with anything but `s64` is rejected, with the rejection pointing at
the guarded procedure. (A predicate that fails to *run* is deliberately not treated as a
rejection.) Comparing types uses the `id` idiom from [Reflection](/language/reflection/),
since `type_info(T).id == type_info(s64).id` is how you ask "is `T` an `s64`".

## #bake_arguments — specialise by fixing arguments

`#bake_arguments` produces a new procedure from an existing one with some arguments **fixed**:

```jr
add :: (a: s64, b: s64) -> s64 { return a + b; }

add_five :: #bake_arguments add(a = 5);   // a real procedure: b -> a + 5
```

`add_five` lowers to an actual procedure — a clone of `add` with the baked parameters dropped
and their literal values substituted, which is the same machinery `$N` instantiation uses. The
operand is a *call* so the named-argument spelling is the natural one.

## @note metadata

A declaration can carry **notes** — metadata for a metaprogram to read, distinct from
directives (which instruct the compiler):

```jr
old_way :: (x: s64) -> s64 @deprecated { return x; }
checked :: (x: s64) -> s64 @requires "a positive x" { return x; }
tracked :: (x: s64) -> s64 @deprecated @internal @since "0.2" { return x; }
```

A note is `@name` or `@name "payload"`. It affects **no code** — the program's compiled output
is identical with or without notes. Notes interleave freely with directives (`@hot #no_abc` or
`#no_abc @hot`), and compose with macros and polymorphism.

### Reading and querying notes

A metaprogram reads notes at **compile time**, folded during checking with no VM needed:

```jr
has_note(checked, "requires")            // bool
note_value(checked, "requires")          // "a positive x"

noted_count("serialise")                 // how many declarations carry @serialise
noted_name("serialise", 0)               // name them, in declaration order
```

The first argument to `has_note`/`note_value` is the **declaration itself**, not its name as
text — so a misspelling is an unresolved-name error rather than a silent `false`. An absent
note is not an error: `has_note` answers `false`, `note_value` answers `""`. `noted_count` /
`noted_name` walk declarations in **declaration order** (the one order you can predict from the
source), and an out-of-range index answers `""` so an unrolled loop's tail stays quiet.

### Generating code for each noted declaration

The payoff: `noted_insert` emits a template **once per noted declaration**, with `#` standing
for each name:

```jr
main :: () {
    n := 0;
    // one line → a call to every @counted procedure in the file
    #insert noted_insert("counted", "n = n + #() * #();");
    exit(n);
}

alpha :: () -> s64 @counted { return 1; }
beta  :: () -> s64 @counted { return 2; }
```

This is the whole metaprogram loop for the case that matters — "find every declaration tagged
`@X` and generate code for each one." It works inside the fold (generated code must exist
*before* checking, so a run-time loop could never do this job), and it needed no new machinery:
the note query, the fold channel, and `#insert` of a computed string were all already there.

`#` is the placeholder because it is a single character that is neither valid in an identifier
nor already an operator. A note whose set is empty folds to `""`, splicing nothing — an empty
generated section, not an error.

### Reading declarations at run time

`noted_declarations` is the run-time counterpart to `noted_count`/`noted_name`: it folds, at
compile time, into a `[]Declaration` — a table the compiler emits once and a running program
can loop over like any other view:

```jr
alpha :: () -> s64 @route "GET /a" { return 1; }
beta  :: () -> s64 @route "GET /b" { return 2; }

main :: () {
    routes := noted_declarations("route");
    for r: routes {
        print("% -> %\n", r.name, r.note_value);
    }
}
```

`Declaration` has two fields — `name` and `note_value` — in **declaration order**, the same
order `noted_count`/`noted_name` use. This is the difference from everything above it in this
chapter: `#insert noted_insert(…)` generates code that exists before the program runs, while
`noted_declarations` hands the *running* program a table it can genuinely iterate, count, or
filter with an ordinary `for` and a `bool` you compute at run time.

## Build scripts

A program can name its own build artefact from inside itself, as a declared constant rather
than a call — a call's effect would depend on evaluation order, while a constant is simply a
fact about the file:

```jr
BUILD_OUTPUT :: #run choose_name();    // `jr build` writes this filename
BUILD_OPT_LEVEL :: 1;                  // and this optimisation level
```

An explicit `-o` or `-O` on the command line still wins — the operator overriding the
artefact's own preference on purpose — and `BUILD_OUTPUT`'s value is confined to the working
directory, so a compiled-from-source file can't write outside it.

That is a program naming *itself*; a real build script is a **separate program**. `jr build
build.jr` treats a file that imports `modules/Compiler` specially: the file itself is
compiled and run first, as an ordinary program in the bytecode VM, and the compilations *it*
asks for happen afterwards:

```jr
#import "Basic";
#import "Compiler";

main :: () {
    t := create_target("myprog");
    add_file(t, "src/main.jr");

    o := options(t);
    o.kind = Output_Kind.EXECUTABLE;
    o.opt_level = 1;
    set_options(t, o);

    ok := build(t);
    if !ok {
        exit(1);
    }
}
```

`modules/Compiler` gives a script `create_target`, `options`/`set_options`, `add_file`,
`add_linker_argument`, `add_build_string`, `command`/`argument_of`/`run`/`output` and a
`shell` convenience, `read_file`/`write_file`, and `build` itself. The fallible ones —
`write_file`, `build`, `run` and `shell` — are `#must`, so a script cannot silently ignore a
failed build or a failed shell command; the rest either cannot fail or return nothing there
would be to check. A target builds as an **executable**, a **static archive**, a **dynamic
library**, or an **object file** — `#program_export` marks a procedure with a C-visible
symbol, since a library that exports nothing is not one. Source
text injected through `add_build_string` is visible to the target only through
`#import "Build"`, since Jairs has no shared global scope for a generated name to land in
unqualified.

Next: [The standard library](/language/the-standard-library/).
