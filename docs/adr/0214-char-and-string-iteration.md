# ADR-0214: `#char` and string iteration share byte semantics

- **Status:** Accepted
- **Date:** 2026-09-08
- **Deciders:** dboll
- **Amends:** ADR-0004 (strings are counted bytes), ADR-0049 §1 (`for`'s compiler-known
  sequence shapes), ADR-0016 §2 (context-typed integer literals)

## Context

The first lexer loop in the Tsoding-shaped `.ora` experiment is ordinary Jai:

```jr
for c: content {
    if c == #char "<" {
        // ...
    }
}
```

`content` is a `string`. Jairs already parses both the named form above and the nameless
`for content { it }`, but sema admits only arrays and views as sequences. The source therefore reaches
E0247 even though ADR-0004 already fixes a string's representation as `{data: *u8, count: s64}`.

`#char` already reaches the generic directive CST as a directive with a string operand. Lowering
rejects it only because it is not in the expression-directive allowlist. The remaining question is
what value it denotes. That decision must agree with iteration: a loop value and the literal compared
with it cannot use different notions of a character.

The decider chose byte iteration and an ASCII byte literal.

## Decision

### §1. A string is directly iterable as `u8` bytes

Both forms are legal:

```jr
for text {
    use(it, it_index);
}

for byte, index: text {
    use(byte, index);
}
```

The element is `u8`; the index is the zero-based `s64` byte offset. A pointer to a string follows the
same recursive auto-dereference rule arrays and views already use. Forward and reverse loops,
`it`/`it_index`, bounds checks, and the loop-local copy semantics are unchanged.

This is a fourth compiler-known iterable shape, not an implicit conversion from `string` to `[]u8`.
ADR-0044 makes those distinct types deliberately, and iteration needs no value conversion: it reads
the existing string data and count projections directly.

### §2. `#char` is one decoded ASCII byte, represented as an integer literal

`#char` takes a string literal whose decoded contents are exactly one ASCII character. Ordinary
escapes are decoded first, so these denote the same value:

```jr
#char "A"
#char "\u0041"
```

Escaped newline, NUL, quote, and backslash are equally valid. Empty, multi-character, and non-ASCII
operands are E0296. An invalid string escape keeps its existing lexical/lowering diagnostic and does
not receive a second cardinality error.

The result lowers directly to the existing untyped integer literal. In a comparison with a loop byte
it becomes `u8`; without an expected type it follows the ordinary integer-literal default. There is no
`Literal::Char`, HIR node, pool item, MIR operation, or backend representation for a value that is
already an integer.

### §3. Iteration indexes encoded bytes, not Unicode scalar values

UTF-8 text is visited one byte at a time. `it_index` is therefore always an offset usable with the
string's `.data` and `.count`, and reverse iteration reverses bytes. `#char "é"` is refused rather
than made into a scalar that could equal no one-byte iteration value.

Unicode code-point or grapheme iteration belongs in a library iterator with an explicit decoding and
invalid-input policy. It is not hidden inside the language's byte loop.

### §4. Lowering reuses the existing sequence path

Sema maps `string` to element type `u8`. MIR loads the bound through `StringCount` and the element
base through `StringData`, then uses the same induction variable, bounds check, indexed load, and
loop-local assignment as arrays and views.

All three engines already lower those projections. No backend operation changes, but the wave still
runs gate 7 because the MIR builder changes and the third engine has something to verify.

## Rejected alternatives

- **Iterate Unicode scalar values.** It needs a decoding/error policy, makes reverse iteration and
  indexing different operations, and does not match the byte predicates in `Basic`.
- **Return the first byte or pack several UTF-8 bytes from `#char`.** Both silently hide whether the
  source denotes a scalar or bytes. Refusing the ambiguity is safer.
- **Add a character type or HIR literal.** The runtime value is an integer, and another representation
  would make every evaluator and backend learn a distinction they immediately erase.
- **Convert `string` implicitly to `[]u8`.** It weakens ADR-0044's explicit boundary and changes more
  than iteration needs.
- **Wait for a user-defined iteration protocol.** The representation and element type are already
  compiler facts, while such a protocol remains a separate language design.

## Consequences

The source idiom `for c: content { if c == #char "<" { ... } }` now has one coherent type story:
`content` contributes `u8` values and `#char` contextually becomes that same type. The nameless form
does the same through `it`.

E0296 is consumed. The next free diagnostic code is E0297.

Iterating by reference (`for *c`), Unicode-aware iteration, and user-defined iterable types remain
absent. Assigning to the loop variable still changes its copy rather than the source byte.
