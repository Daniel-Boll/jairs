---
title: "@note metadata"
description: Metadata a declaration carries for a metaprogram to read — not an instruction to the compiler.
sidebar:
  order: 63
---

A `@note` is metadata a declaration carries for a *metaprogram* to read (ADR-0098). It is `@name` or
`@name "payload"` on a declaration — `@deprecated`, `@requires "a positive x"`. Crucially, a note is
**not** an instruction to the compiler — that is what directives like `#c_call` and `#no_abc` are —
which is why it is its own node kind and its own list on the declaration. A consumer collecting notes
must not have to filter directives out of the same list, and vice versa.

```jr
#import "Basic";

/// A bare note.
old_way :: (x: s64) -> s64 @deprecated {
    return x;
}

/// A note with a payload — the same form, with the optional string present.
checked :: (x: s64) -> s64 @requires "a positive x" {
    return x;
}

/// Several notes on one declaration.
tracked :: (x: s64) -> s64 @deprecated @internal @since "0.2" {
    return x;
}

/// A note before a directive.
fast :: (x: s64) -> s64 @hot #no_abc {
    return x;
}

/// And after one, since the loop takes either order.
faster :: (x: s64) -> s64 #no_abc @hot {
    return x;
}

/// A note on a `#expand` macro — notes compose with macros.
doubled :: (x: s64) -> s64 @inline #expand {
    return x * 2;
}

/// A note on a polymorphic procedure — and on one with a `$T`, so it composes with instantiation too.
identity :: (x: $T) -> T @generic {
    return x;
}

main :: () {
    n := identity(7);
    exit(n);
}
```

## The two spellings are one form

`@deprecated` is a **bare** note; `@requires "a positive x"` is the same form with an optional string
payload present. Several notes stack on one declaration (`@deprecated @internal @since "0.2"`), so the
list is genuinely a list. A note sits happily beside a directive in either order (`@hot #no_abc` and
`#no_abc @hot`), the same any-order rule the directives themselves follow — and notes compose with the
rest of the language: they attach to a `#expand` macro and to a polymorphic `$T` procedure just as
readily as to an ordinary one.

## A note affects no code

This is the property worth pinning: a note is read by a *metaprogram*, not by the compiler, so it
changes nothing about the program's behaviour. The MIR of the program above is exactly what it would be
with the notes deleted — `main` does ordinary work and exits 7.

That is why notes came first among the metaprogramming build-script features: they are the *data* the
readers, queries and code generators operate on. Building a reader against notes that did not yet exist
would have meant designing its shape against no consumer. The pages that follow — reading a named
declaration's notes, querying a file for every declaration tagged `@X`, and generating code for each —
all build on this one form.
