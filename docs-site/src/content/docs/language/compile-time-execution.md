---
title: Compile-time execution
description: "Running ordinary Jairs code while compiling — #run, #insert, and #code."
sidebar:
  order: 14
---

This is where Jairs stops being a tidier C. Ordinary Jairs code can run **while the program
is being compiled**, in a bytecode virtual machine, and it can produce values *and generate
source* that the rest of the compilation then sees. There is no separate macro language: the
metaprogram is Jairs.

The keystone property, from [the introduction](/language/introduction/#two-engines-one-language):
the compile-time VM and the native back end execute the **same** intermediate representation.
So a value computed at compile time and the same computation at run time cannot disagree.

## #run — run code while compiling

`#run expr` evaluates `expr` in the compile-time VM and substitutes the result:

```jr
add :: (a: s64, b: s64) -> s64 { return a + b; }

COMPUTED :: #run add(2, 3);        // COMPUTED is the constant 5

main :: () {
    n := #run add(10, 20);         // #run works inside a body too
}
```

`#run` can call local **and imported** procedures, do arithmetic around a call, loop inside
the callee, and nest. It is bounded by a **step budget** (ten million instructions), after
which a non-terminating `#run` reports an error rather than hanging the compiler — important
because the same evaluator runs when the language server merely *opens* a file.

A `#run` may return a **struct or an array**, not just a scalar — it is interned as its
element *values* (not a raw byte image, so it is target-independent), and each engine
materialises it correctly. It may even return a struct holding a **string**.

Still <span class="jairs-status absent">absent</span> in `#run`: reading *another file's*
constant, a `#foreign` call, and using an operator overload or a default/named argument —
each because const-evaluation runs before the phase that would resolve them.

## #insert — splice generated source

`#insert` takes a **string of Jairs source**, parses it, and lowers the statements **right
where the directive is written** — same scope, so a local the inserted code declares is
visible on the next line:

```jr
#insert "n := 2 + 3;";     // as if you had typed `n := 2 + 3;` here
use(n);                    // n is in scope
```

The operand can be *computed*: a constant, or a `#run` whose result is the source text.

```jr
CODE :: "x := 40 + 2;";
#insert CODE;              // splice a named constant's text

#insert #run build_snippet();   // splice text produced at compile time
```

This is the point where the checker and the VM become mutually recursive — lowering can't
finish until the operand is evaluated, and the evaluator runs on lowered code — and Jairs
breaks the cycle with an acyclic pre-pass rather than by guessing. The operand is an ordinary
expression, so `#insert undefined;` is an unresolved-name error and a non-string operand is a
type error, each reported at the operand's own location. Nesting works; expansion past 16
levels is refused.

## #code — the same thing, unquoted

`#code { … }` is `#insert` of source you write **without quotes**:

```jr
#code {
    total := 0;
    total = total + 7;
}
```

The body is parsed where it is written, so there is no string to escape and no quoting to get
wrong. It is deliberately **sugar** over `#insert` — it adds no capability, only ergonomics.

There is intentionally **no `Code` *value*** — a first-class quoted syntax tree. Jairs
*declines* it rather than deferring it: a quoted tree is worth representing only once something
can inspect or transform one, and a value that can merely be spliced is what a `string`
already is.

## Where this leads

Compile-time execution is the foundation the next two chapters build on.
[Reflection](/language/reflection/) lets compile-time code ask about *types*
(`type_info(T)`), and [Metaprogramming](/language/metaprogramming/) turns `#insert` into a
loop that generates code for a whole set of declarations — the "find every function tagged
`@X` and emit a call to each" pattern that Jai calls its superpower.

Next: [Reflection](/language/reflection/).
