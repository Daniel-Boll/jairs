---
title: "#insert"
description: Splicing statements parsed from a string — literal text, or a string computed at compile time — into the enclosing scope.
sidebar:
  order: 51
---

`#insert "…"` takes a string of Jairs source, parses it into statements, and lowers them **where
the directive is written** (ADR-0072). The model is one sentence: an insert is textual
substitution that happens *after* parsing rather than before. Its statements go into the
enclosing scope — not a nested one — so a local an insert declares is visible afterwards, and a
name from the surrounding body is visible inside.

## A literal operand

```jr
#import "Basic";

main :: () {
    n := 0;

    // One statement, declaring a local that the *next* real line reads. If an insert were a
    // scope, this would be an unresolved name.
    #insert "five := 2 + 3;";
    if five == 5 {
        n = n + 1;
    }

    // Several statements in one insert, the second reading the first's local.
    #insert "a := 10; b := a + 1;";
    if b == 11 {
        n = n + 2;
    }

    // An empty insert: legal, and inserts nothing.
    #insert "";

    // A name from one insert, read by another — inserts are not scoped against each other.
    #insert "c := b + 1;";
    if c == 12 {
        n = n + 4;
    }

    // Control flow inside inserted text, including a loop.
    #insert "total := 0; for i: 0..4 { total = total + i; } if total == 6 { n = n + 8; }";

    // An escape sequence, decoded by the same path every string literal takes — so this inserts
    // a `string` and reads its `.count`, rather than six literal characters.
    #insert "greeting := \"hi\"; if greeting.count == 2 { n = n + 16; }";

    // Nested: the inner insert's text is itself an `#insert`.
    #insert "#insert \"nested := 32;\";";
    if nested == 32 {
        n = n + 32;
    }

    // A `defer` in inserted code, which runs when `main` is left rather than when the insert ends.
    #insert "defer exit(n);";

    n = n + 1;
}
```

### Why an insert is not a block

The HIR gained a dedicated `Stmt::Insert` rather than reusing `Stmt::Block`, because a block
would have been wrong in two separate ways, each pinned by this file:

- A block is a **defer scope**, so a `defer` inside a block-based insert would run at the insert's
  end. Here the deferred `exit(n)` runs when `main` is left. The `n = n + 1` on the last line
  happens *after* the `defer` was written and is still counted, so the program exits `64` (the six
  earlier assertions sum to 63, plus one). A scoped insert would have exited `63` at the `defer`
  line instead — so the expected value distinguishes the two designs.
- A block pushes a **name scope**, so a local an insert declared would be invisible on the next
  line. Here `five`, `a`, `b`, `c` and `nested` are all declared inside inserts and read by real
  code afterwards.

Names also cross *between* inserts: `#insert "c := b + 1;"` reads `b`, declared by an earlier
insert. Inserts are not scoped against each other.

### Escapes and nesting

Because the operand is an ordinary string literal, escape sequences go through the same decoder
every string takes: `\"hi\"` inserts a genuine `string`, not the six surrounding characters. A
literal insert can even contain another `#insert`, and no depth bound is needed — escaping
*doubles* the text at every level, so a literal insert is bounded by the file it is written in.

## A computed operand

The operand need not be a literal. It may be any compile-time string — a named constant, or the
result of a `#run` (ADR-0073). This is the point at which sema and the VM become mutually
recursive: the operand must be evaluated in the bytecode VM *before* lowering can finish. That
cycle is broken by an acyclic pre-pass that evaluates insert operands, rather than by fixed-point
recovery.

```jr
#import "Basic";

// A constant whose value is Jairs source. The signature phase knows its *type* is `string`; its
// *value* is what the operand pre-pass evaluates.
CODE :: "n = n + 10;";

// An empty computed operand: evaluates to the empty string and inserts nothing.
EMPTY :: "";

// A computed operand whose text itself contains a literal `#insert`.
NESTED :: "#insert \"n = n + 32;\";";

// A procedure run at compile time to produce source text — the `#run` operand form.
build :: () -> string {
    return "n = n + 16;";
}

main :: () {
    n := 0;

    // A named-constant operand: `n = n + 10;` is spliced here, writing the enclosing `n`.
    #insert CODE;

    // An empty computed operand: inserts nothing, changes nothing.
    #insert EMPTY;

    // A `#run` operand: `build()` runs in the comptime VM, and its returned string is spliced.
    #insert #run build();

    // A nested computed operand: `NESTED`'s text is a literal `#insert`, which expands in turn.
    #insert NESTED;

    // 10 + 16 + 32 = 58.
    exit(n);
}
```

Everything the literal form guarantees still holds — the spliced statements land in the enclosing
scope and can read and write outer locals like `n`. The computed form only adds one step first:
evaluate the operand to a string, then substitute. An **empty** computed operand is worth its own
case: it is the one that distinguishes "evaluated, and it was empty" from "not yet evaluated",
since both leave the insert with no statements. Clearing the operand on expansion is what tells
them apart.

### What is refused

- A **cross-file** computed `#insert` (one whose text would change the item tree of another file)
  is refused, since `#insert` at file scope would alter the item tree (ADR-0072 §5).
- A **non-string** operand is a type error, caught in the type-error corpus.
- Nesting past 16 levels is a distinct refusal, but a computed operand cannot be written deeply
  enough to reach it by hand, so that bound has its own unit test rather than a corpus file.

The `exit` value (`58` here) makes the result observable so the two engines can be asserted to
agree byte-for-byte: if any insert had its own scope its write to `n` would not reach the `exit`,
and if the empty insert were treated as *pending* rather than evaluated-empty, the body would be
refused.
