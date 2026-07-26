# 02 — Declarations

A Jairs program is a sequence of declarations. This chapter covers the three
declaration forms, the fact that procedures and struct types are just constants
(ADR-0012), procedure and struct declarations, the pointer type, explicit
non-initialisation, and why declaration order does not matter. It covers exactly
the Jairs-0 subset; everything absent names its wave.

## Declaration order does not matter

Semantic analysis is **lazy and on-demand** (ADR-0007), so a declaration may
refer to a name declared later in the file. Constants may refer to earlier *or
later* constants; a procedure may call a procedure defined below it
(`007-constants.jr`, `020-run-directive.jr`):

```jr
LIMIT :: MAX_ENTITIES;      // refers to a constant declared below
DERIVED :: MAX_ENTITIES + 1;

MAX_ENTITIES :: 4096;
```

There is no forward-declaration requirement and no "declare before use" rule.

## The three declaration forms

Every binding in Jairs is one of three forms, distinguished purely by the
punctuation between the name and the right-hand side.

| Form | Meaning | CST node |
|---|---|---|
| `name :: value` | **Compile-time constant.** The value must be evaluable at compile time. | `CONST_DECL` |
| `name := value` | **Inferred variable.** The type is inferred from the initialiser. | `VAR_DECL` |
| `name: T` / `name: T = value` | **Typed variable.** Explicit type, optionally initialised. | `VAR_DECL` |

### Constants — `::`

`::` binds a compile-time constant; the right-hand side must be computable at
compile time (`007-constants.jr`):

```jr
MAX_ENTITIES :: 4096;
GREETING     :: "hello";
DEBUG        :: false;

LIMIT   :: MAX_ENTITIES;
DERIVED :: MAX_ENTITIES + 1;
```

A constant's value may be produced by a compile-time `#run`
(`020-run-directive.jr`); the folded result is interned as a compile-time value
indistinguishable from a literal:

```jr
COMPUTED :: #run add(2, 3);
```

### Inferred variables — `:=`

`:=` declares a variable and infers its type from the initialiser
(`006-decl-inferred.jr`). Inference propagates through calls:

```jr
main :: () {
    count := 10;          // s64
    flag := false;        // bool
    label := "inferred";  // string
    doubled := twice(count);  // inferred from twice's return type
}

twice :: (n: s64) -> s64 {
    return n + n;
}
```

### Typed variables — `: T [= value]`

A typed declaration gives the type explicitly, with an optional initialiser
(`005-decl-typed.jr`):

```jr
main :: () {
    a: s64 = 7;        // explicit type, explicit value
    b: s64;            // explicit type, default-initialised to the zero value
    c: s64 = ---;      // explicitly uninitialised (see below)
    d: bool = true;
    e: string = "text";
    f: *s64 = *a;
    g: u8 = 255;

    b = a;
}
```

A typed declaration with no initialiser (`b: s64;`) is **default-initialised to
the type's zero value**. To opt out of that zeroing, use `---` (below).

## Explicit non-initialisation — `---`

`---` on the right of a typed declaration means "do **not** initialise this"
(`005-decl-typed.jr`). The compiler will not zero the storage. Reading such a
variable before it is assigned is an error — but that use-before-assignment check
is a **wave W3** deliverable, so in Jairs-0 the `---` form parses and suppresses
zeroing without yet diagnosing a premature read.

```jr
c: s64 = ---;
```

`---` is a single token (`UNINIT`, chapter 01), distinct from one or three
minuses.

## Procedures and structs are constants

There is **no** `proc` or `func` or `struct` *declaration* keyword. A procedure
declaration and a struct declaration are both the constant form `name :: value`,
where the value happens to be a procedure or a struct type (ADR-0012). The
grammar has one declaration rule; the shape of the right-hand side determines
what was declared.

### Disambiguating a procedure from a parenthesised expression

Because procedures and expressions are both just right-hand sides of `::`, a `(`
immediately after `::` is ambiguous. Both of these are legal:

```jr
add     :: (a: s64, b: s64) -> s64 { return a + b; }   // a procedure
GROUPED :: (1 + 2) * 3;                                 // an expression
```

The rule: **scan to the matching `)` and look at the token after it.** The
right-hand side is a procedure if and only if that token is `->`, `{`, or the
`#foreign` directive. Nothing else may follow a parameter list, and none of
those may follow an expression, so the rule is exact rather than heuristic.

If the `(` is never closed — as while the user is still typing — the compiler
assumes a procedure, because that produces the more useful diagnostic.

Note there is no ambiguity after `:=` or `=`, where the right-hand side is
always an expression. See `025-paren-constant.jr`, which exercises both readings
side by side.

## Procedure declarations

A procedure is a constant whose value is `(params) -> ReturnType { body }`. The
parameter list is comma-separated `name: Type` pairs; the return type follows
`->`; the body is a brace-delimited block (`004-proc-params-return.jr`):

```jr
add :: (a: s64, b: s64) -> s64 {
    return a + b;
}
```

A procedure that returns nothing **omits the arrow entirely**:

```jr
discard :: (unused: s64) {
    return;
}
```

A procedure with no parameters has an empty parameter list, and an empty body is
legal (`003-proc-empty.jr`):

```jr
noop :: () {
}
```

In Jairs-0 each parameter needs its own type annotation, even when adjacent
parameters share a type; parameter grouping is a **wave W2** feature
(`004-proc-params-return.jr`). Single return values only; multiple return values
are also W2.

### Foreign procedures

A `#foreign` procedure has **no body**: it is a signature terminated with a
semicolon, bound to a symbol in a foreign library (`019-foreign.jr`). Foreign
procedures use the C calling convention and are `#c_call` implicitly, opting out
of the implicit context parameter (ADR-0001).

```jr
// A foreign library binding: a constant whose value is a #system_library
// directive expression. `#system_library` resolves through the platform's
// dynamic loader.
libc :: #system_library "c";

// A foreign procedure: signature, `#foreign <library> "<symbol>"`, semicolon.
// The Jairs name and the symbol name may differ.
write :: (fd: s64, buf: *u8, count: s64) -> s64 #foreign libc "write";

os_exit :: (status: s64) #foreign libc "exit";
```

The `#foreign` attribute names the library constant (`libc`) and the external
symbol string (`"write"`), which is why the Jairs-side name and the C symbol can
differ. This is the mechanism by which the Jairs-0 stdlib reaches libc `write`
(`PLAN.md` §1.2), and it is why FFI and the string ABI are in the slice rather
than a later wave.

## Struct declarations

A struct is a constant whose value is `struct { fields }`. Each field is
`name: Type;`. Fields of aggregate type are allowed, so structs compose
(`008-struct.jr`):

```jr
Point :: struct {
    x: s64;
    y: s64;
}

// An empty struct is legal and occupies zero bytes.
Marker :: struct {
}

Entity :: struct {
    position: Point;   // a field of struct type
    health: s64;
    alive: bool;
}
```

Jairs-0 structs are "one level" in the sense of no polymorphic parameters, no
`using`, and no methods — just named, typed fields (`PLAN.md` §1.1). Those
features arrive in later waves (`using` is W2; polymorphic structs follow the
polymorphism wave W5).

## The pointer type

`*T` is the type "pointer to `T`", and it nests: `**T` is a pointer to a pointer
to `T` (`005-decl-typed.jr`, `015-pointers.jr`). The `*` in a pointer *type* is
the same `STAR` token used for prefix address-of and for multiplication; the
parser disambiguates by position (ADR-0011, chapter 01).

```jr
main :: () {
    value := 42;

    p: *s64 = *value;    // *s64 is a pointer type; *value takes an address
    copied := p.*;       // .* dereferences (postfix)
    p.* = 43;

    ppp: **s64 = *p;     // pointer to pointer
    round_trip := ppp.*.*;
}
```

Address-of (prefix `*`) and dereference (postfix `.*`) are expression forms
covered with the rest of the expression grammar; they appear here only so the
pointer *type* `*T` is complete. Field access through a pointer
auto-dereferences (`015-pointers.jr`):

```jr
origin: Point;
pp := *origin;
pp.x = 1;      // no explicit deref needed to reach a field through a pointer
```

## Fields and names summary

- A **field** is `name: Type;` inside a `struct { … }` (`FIELD` in a
  `FIELD_LIST`).
- A **parameter** is `name: Type` inside a procedure's `(…)` (`PARAM` in a
  `PARAM_LIST`).
- The bound **name** of any declaration is a `NAME` node; the declaration itself
  is `CONST_DECL` (`::`) or `VAR_DECL` (`:=` and `: T`), with `IMPORT_DECL` and
  `RUN_DECL` for the two directive declaration forms (`#import "…";` and a
  top-level `#run …;`).
