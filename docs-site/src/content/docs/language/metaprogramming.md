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

## Build scripts

A program can even name its own build artefact:

```jr
BUILD_OUTPUT :: #run choose_name();    // `jr build` writes this filename
```

It is a **declared constant** rather than a `set_output()` call, because a call's effect would
depend on evaluation order while a constant is simply a fact about the file. An explicit `-o`
on the command line still wins — that is the operator overriding on purpose — and the value is
confined to the working directory so a compiled-from-source file can't write outside it. This
is not a build *system* (no dependency graph, no incremental rules); it is the makefile's most
basic job, done in the language.

## What's still missing

The honest gap: **run-time inspection** — a *run-time* loop reading declarations as values.
Everything above happens while *checking*, so every argument must be readable then, and a
`for` variable is not. Reading declarations at run time needs a compiler-emitted static table
that both engines can read, which Jairs does not yet have. So notes can be counted, named, and
generated *for* at compile time, and cannot yet be *looped over* at run time.

Next: [The standard library](/language/the-standard-library/).
