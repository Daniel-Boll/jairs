# 01 — Lexical structure

This chapter describes how Jairs source text is turned into tokens. It is
verified against the implemented lexer, `crates/jr-syntax/src/lexer.rs`, and the
token vocabulary in `crates/jr-syntax/src/kind.rs`. Where this chapter and that
code disagree, the code is authoritative and this chapter has a bug.

Two invariants govern the whole lexer:

1. **Lexing never fails.** Every byte of input ends up inside exactly one token,
   so token ranges tile the source with no gaps or overlaps and concatenating
   them reproduces the input exactly. Problems are reported as diagnostics
   *alongside* a best-guess token, never by aborting.
2. **Trivia is preserved.** Whitespace and comments become real tokens, so the
   concrete syntax tree is lossless and `jr fmt` can round-trip.

## Source encoding

Source is UTF-8. Only ASCII is meaningful *outside* comments and string
literals: identifiers are ASCII (see below), and any other non-ASCII byte in
code position is an unrecognised character (diagnostic **E0005**), consumed one
whole UTF-8 character at a time so the lexer never slices mid-character. Inside
comments and string literals, arbitrary Unicode is fine — `"café 中文 🦀"` is a
valid string literal.

## Whitespace

Whitespace is any run of Unicode whitespace characters (spaces, tabs, newlines).
It is lexed into a single `WHITESPACE` trivia token. A CR-LF sequence is
whitespace, so Windows line endings work (`a\r\nb` is two identifiers separated
by whitespace).

## Comments

Jairs has two comment forms plus two documentation forms, all four trivia
(`002-comments.jr`, `026-doc-comments.jr`):

```jr
//! Documentation for the enclosing module.

// A line comment.

/* A block comment. */

/*
    Block comments /* nest */, unlike C. The lexer tracks depth so that
    commenting out a region containing a comment does the obvious thing.
*/

/// Documentation for the declaration that follows.
answer :: 42;

//// Four or more slashes are a rule, not documentation.

// Trailing comment with no newline after it.
```

- **Line comment** — `//` to the end of the line. The terminating newline is
  **not** part of the comment; it stays whitespace. A line comment at end of
  file with no trailing newline is fine.
- **Block comment** — `/* … */`, and **block comments nest**. Each `/*` increases
  depth and each `*/` decreases it; the comment ends only when depth returns to
  zero. This is the property that makes commenting out a region that already
  contains a comment behave as expected. An unterminated block comment is
  reported at the **outermost** `/*` (diagnostic **E0002**), because that is the
  one the user needs to find, and the diagnostic notes how many `/*` are still
  open.
- **Doc comment** — `///` to the end of the line, documenting the declaration
  that follows it.
- **Module doc comment** — `//!` to the end of the line, documenting the file.

### Documentation comments

Added by [ADR-0027](../adr/0027-doc-comments.md). Three rules, and the first is the
one that keeps this a lexical matter rather than a grammatical one:

1. **A doc comment is trivia.** The parser never sees one, so no grammar rule can
   require or forbid one, and adding these forms cannot change what parses. `///`
   before a declaration and `///` in the middle of a procedure body are the same
   token to the parser.
2. **`////` is not documentation.** Four or more slashes lex as an ordinary line
   comment, following Rust, because a row of slashes is a visual rule. `//!!` *is*
   module documentation, whose text happens to begin with `!` — also as in Rust,
   because there is no corresponding convention of writing a rule out of `!`.
3. **A doc comment that precedes no declaration is silently ignored.** No
   diagnostic. Attachment happens above the lexer, in `jr_db::file_docs`, which
   also decides that a blank line or an intervening `//` comment breaks a doc
   block — so what a reader sees as separated is separated.

Attachment is therefore *not* a lexical property, and nothing in this chapter
promises a `///` will document anything. The lexer's whole contribution is to say
which comments are documentation and which are asides.

## Identifiers

An identifier starts with an ASCII letter or `_`, and continues with ASCII
letters, digits, or `_`:

- start: `_` or `A`–`Z` or `a`–`z`
- continue: `_` or `A`–`Z` or `a`–`z` or `0`–`9`

Identifiers are **ASCII only**. `héllo` is not one identifier — it lexes as the
identifier `h` followed by an unrecognised character `é` (E0005). Casing is
significant, so `If` is an identifier and not the keyword `if`, and `structure`
is an identifier and not `struct`.

## Keywords

Keyword recognition is exact-match against identifier text. Keywords split into
two tables: those the parser **accepts** in Jairs-0, and those it merely
**reserves** for a later wave. A reserved keyword is still lexed as its keyword
token (not as an identifier), so that using it produces a "not yet implemented,
arrives in wave W*n*" diagnostic rather than a confusing error — and so that a
later wave adding the feature is not a breaking change for code that used the
word as a name.

That refusal is the parser's **E0121**, and a reserved *literal* form is **E0120**
(floating-point) or **E0122** (a bitwise operator). All three sit inside the parser's
E0100–E0199 block. They previously used E0200–E0202, which belong to name resolution —
so a tool filtering on "unresolved name" saw a `for` loop.

### Accepted keywords (Jairs-0)

| Keyword | Token |
|---|---|
| `struct` | `STRUCT_KW` |
| `if` | `IF_KW` |
| `else` | `ELSE_KW` |
| `while` | `WHILE_KW` |
| `return` | `RETURN_KW` |
| `break` | `BREAK_KW` |
| `continue` | `CONTINUE_KW` |
| `true` | `TRUE_KW` |
| `false` | `FALSE_KW` |

### Reserved keywords (lexed, not yet accepted)

| Keyword | Token | Arrives in |
|---|---|---|
| `enum` | `ENUM_KW` | W1 |
| `union` | `UNION_KW` | W1 |
| `cast` | `CAST_KW` | W1 |
| `xx` | `XX_KW` | W1 |
| `null` | `NULL_KW` | W1 |
| `for` | `FOR_KW` | W2 |
| `defer` | `DEFER_KW` | W2 |
| `using` | `USING_KW` | W2 |

Type names such as `s64`, `bool`, `string`, and `u8` are **not** keywords — they
are ordinary identifiers resolved as types by name (see chapter 02).

## Integer literals

An integer literal is `INT_LITERAL`. Four radices are recognised, and `_` may be
used as a digit separator anywhere among the digits (`022-integer-literals.jr`):

| Form | Prefix | Example |
|---|---|---|
| Decimal | *(none)* | `1234567890`, `9223372036854775807` |
| Hexadecimal | `0x` / `0X` | `0xdead_beef`, `0XDEADBEEF` |
| Binary | `0b` / `0B` | `0b1010_1010` |
| Octal | `0o` / `0O` | `0o755` |

```jr
main :: () {
    decimal := 1234567890;
    zero    := 0;

    hex     := 0xdead_beef;
    binary  := 0b1010_1010;
    octal   := 0o755;

    grouped := 1_000_000;

    max := 9223372036854775807;
}
```

A radix prefix with no following digits (`0x`, `0b_`) is an error (**E0004**,
"*radix* literal has no digits") but still yields an `INT_LITERAL` token so the
parser has something to work with. A trailing identifier-like suffix such as
`123abc` is treated as one malformed literal, not a number followed by a name,
and reported once (**E0004**, "invalid suffix on numeric literal") — Jairs has no
literal suffixes.

## Float literals

Float literals **lex** as `FLOAT_LITERAL` but are **rejected by the parser until
wave W1** — floats are not part of Jairs-0. They are recognised so that a float
in Jairs-0 source produces a clear "floats arrive in W1" diagnostic rather than a
lexer error.

A decimal integer becomes a float when it is followed by either:

- a fractional part: a `.` **immediately followed by a digit** (`1.5`, `0.0`); or
- an exponent: `e`/`E`, an optional `+`/`-`, and **at least one digit**
  (`1e9`, `1.5E+10`, `2.0e-3`).

The "followed by a digit" rule on the fractional part is load-bearing: it is
what keeps `1..2` lexing as `1` `..` `2` (a range, chapter/word W1) and `1.x` as
`1` `.` `x` (field access) rather than as broken floats. A half-written exponent
`1e` is reported as "exponent has no digits" (**E0004**) — the likelier intent
than "a variable named `e`".

## String literals

A string literal is `STRING_LITERAL`: `"` … `"` on a **single line**
(`021-string-literals.jr`). A string is `{data: *u8, count: s64}` and is **not**
NUL-terminated (ADR-0004).

```jr
main :: () {
    plain  := "simple";
    empty  := "";
    escapes := "tab:\there\nnewline above\r\n";
    quoted := "she said \"hello\"";
    slash  := "back\\slash";
    zero   := "embedded\0nul";
    unicode := "caf\u00e9 \u4e2d\u6587";

    n := plain.count;
    d := plain.data;
}
```

An unterminated string stops at the **end of the line**, not end of file
(diagnostic **E0001**): a missing quote costs one error instead of swallowing the
rest of the program. A backslash immediately before a newline does not consume
the newline, for the same reason.

### Escape sequences

The complete escape table. Anything else after a backslash is an unknown escape
(**E0003**).

| Escape | Meaning |
|---|---|
| `\n` | newline |
| `\r` | carriage return |
| `\t` | tab |
| `\0` | NUL |
| `\\` | backslash |
| `\"` | double quote |
| `\uXXXX` | Unicode scalar, **exactly four** hex digits |

A `\u` escape with other than four hex digits is an "invalid unicode escape"
(**E0003**). An escaped quote `\"` does not end the string. Unknown escapes are
reported but lexing continues, so one bad escape does not derail the rest of the
literal.

## Directives

A directive is a single `DIRECTIVE` token: `#` immediately followed by an
identifier-start character and then identifier-continue characters — `#import`,
`#run`, `#foreign`, `#system_library`, `#c_call`, `#no_abc`, and any future
directive. **The `#` is part of the token**, and the lexer does not interpret the
directive name; the parser does. That means adding a new directive never requires
a lexer change. A `#` not followed by an identifier start is an error (**E0006**,
"expected a directive name after `#`") and yields an `UNKNOWN` token.

## Operators and punctuation

The complete operator/punctuation table, exactly as the lexer knows it. "Status"
notes tokens that are lexed but reserved for a later wave.

| Token | Kind | Status |
|---|---|---|
| `+` | `PLUS` | |
| `-` | `MINUS` | |
| `*` | `STAR` | multiply, `*T` pointer type, prefix address-of (ADR-0011) |
| `/` | `SLASH` | |
| `%` | `PERCENT` | |
| `+%` | `PLUS_PERCENT` | wrapping add |
| `-%` | `MINUS_PERCENT` | wrapping subtract |
| `*%` | `STAR_PERCENT` | wrapping multiply |
| `=` | `EQ` | |
| `+=` | `PLUS_EQ` | |
| `-=` | `MINUS_EQ` | |
| `*=` | `STAR_EQ` | |
| `/=` | `SLASH_EQ` | |
| `%=` | `PERCENT_EQ` | |
| `+%=` | `PLUS_PERCENT_EQ` | wrapping add-assign |
| `-%=` | `MINUS_PERCENT_EQ` | wrapping sub-assign |
| `*%=` | `STAR_PERCENT_EQ` | wrapping mul-assign |
| `==` | `EQ_EQ` | |
| `!=` | `BANG_EQ` | |
| `<` | `LT` | |
| `<=` | `LT_EQ` | |
| `>` | `GT` | |
| `>=` | `GT_EQ` | |
| `&&` | `AMP_AMP` | |
| `\|\|` | `PIPE_PIPE` | |
| `!` | `BANG` | |
| `&` | `AMP` | reserved, W1 (bitwise) |
| `\|` | `PIPE` | reserved, W1 (bitwise) |
| `^` | `CARET` | reserved, W1 (bitwise) |
| `~` | `TILDE` | reserved, W1 (bitwise) |
| `<<` | `SHL` | reserved, W1 (bitwise) |
| `>>` | `SHR` | reserved, W1 (bitwise) |
| `@` | `AT` | reserved, W6 (declaration notes) |
| `(` `)` | `L_PAREN` `R_PAREN` | |
| `{` `}` | `L_BRACE` `R_BRACE` | |
| `[` `]` | `L_BRACK` `R_BRACK` | reserved, W1 (arrays) |
| `,` | `COMMA` | |
| `;` | `SEMICOLON` | |
| `:` | `COLON` | |
| `::` | `COLON_COLON` | constant declaration |
| `:=` | `COLON_EQ` | inferred declaration |
| `->` | `ARROW` | return type |
| `.` | `DOT` | field access |
| `.*` | `DOT_STAR` | postfix dereference (ADR-0011) |
| `..` | `DOT_DOT` | reserved, W1 (`[..]T`) |
| `---` | `UNINIT` | explicit non-initialisation |

A character the lexer does not recognise at all is `UNKNOWN` with diagnostic
**E0005**, and lexing continues — `$ \`` produces two `UNKNOWN` tokens and two
errors, not a halt.

## The longest-match rule

Operators are matched **longest first**. The lexer tries multi-character
operators before their prefixes, so the order in the table above is not
cosmetic — it is the disambiguation rule. The cases that make this non-obvious:

- **`---` vs `-`.** `---` is the single `UNINIT` token. `----` is `---` then `-`
  (`UNINIT` `MINUS`), because `---` wins the longest match and one `-` is left.
  Two minuses `--` are **not** an operator at all — there is no `--` token — so
  they lex as `MINUS` `MINUS`.
- **`-%=` vs `-%` vs `-`.** The three-character wrapping compound assignments beat
  their two-character prefixes, which beat the bare operator: `a -%= b` is
  `MINUS_PERCENT_EQ`, `a -% b` is `MINUS_PERCENT`, `a - b` is `MINUS`. The same
  holds for `+%=`/`+%`/`+` and `*%=`/`*%`/`*`.
- **`->` vs `-`.** `-> -` is `ARROW` `MINUS`; `-= -` is `MINUS_EQ` `MINUS`.
- **`.` vs `.*` vs `..`.** `a.b` is `DOT`; `a.*` is `DOT_STAR`; `1..2` is
  `INT_LITERAL` `DOT_DOT` `INT_LITERAL`. Note that the float rule interacts here:
  a `.` only begins a fractional part when a **digit** follows, so `1..2` and
  `p.*` are never mistaken for floats.

## Lexer diagnostic codes

Every diagnostic the lexer can emit. All are recoverable: a token is still
produced so the parser can continue.

| Code | Message | Emitted when |
|---|---|---|
| **E0001** | unterminated string literal | a `"` string reaches end of line or end of file before its closing quote |
| **E0002** | unterminated block comment | a `/*` (reported at the outermost one) is never closed before end of file |
| **E0003** | invalid unicode escape · unknown escape `\x` | `\u` not followed by exactly four hex digits, or a backslash escape that is not in the escape table |
| **E0004** | *radix* literal has no digits · exponent has no digits · invalid suffix on numeric literal | a radix prefix with no digits, a half-written exponent, or an identifier-like suffix on a number |
| **E0005** | unexpected character `x` | a character (including any non-ASCII byte outside comments/strings) the lexer does not recognise |
| **E0006** | expected a directive name after `#` | a `#` not immediately followed by an identifier-start character |
