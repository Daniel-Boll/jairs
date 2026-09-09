# ADR-0225: Publicly evidenced `String` parity has a non-overload layer

- **Status:** Accepted
- **Date:** 2026-09-09
- **Deciders:** dboll
- **Amends:** ADR-0197 §6–7 and ADR-0198 §1.

## Context

ADR-0197 and ADR-0198 described `modules/String` as covering Jai's useful surface after comparing
against vendored source. The later executable Way-to-Jai audit exposed a narrower and more useful
question: can the public spellings demonstrated by the pinned guide be written here?

The answer is not yet. The guide directly uses `slice`, `copy_string`, `begins_with`,
`find_index_from_left`, `find_index_from_right`, `string_to_int`, `string_to_float` and
`parse_int`, while this module either has a differently named primitive or no operation at all.
It also names `compare_strings`, `replace_chars` and `is_any` in public examples.

The guide is public usage evidence, not a canonical dump of Jai's closed standard-library source.
This ADR therefore claims only the spellings and behavior the pinned revision demonstrates. It
does not infer unevidenced procedures, and it does not pretend Jairs already has the general
procedure overloading needed by several exact Jai call families.

The decider approved:

- a borrowed, strict `slice(s, start, count)`;
- an owned `copy_string(s)`;
- compatibility wrappers where one existing implementation already owns the algorithm;
- conversion spellings that match the guide without deleting remainder-aware parsing;
- deferring exact overload families to the general-overloading wave.

## Decision

### 1. `slice` is a borrowed view with strict bounds

`slice(s, start, count) -> string` returns `borrow(s.data + start, count)`. It allocates nothing,
preserves pointer identity into `s`, and is valid only while `s`'s bytes remain valid.

The accepted range is:

```text
start >= 0
count >= 0
start <= s.count
count <= s.count - start
```

The subtraction form is deliberate: checking `start + count <= s.count` would let the bounds
predicate itself overflow before it could report an out-of-range slice. A failed check uses the
source-located `assert` from ADR-0224 with one stable message.

`substring` remains distinct: it clamps, allocates, and returns an owned copy. Making it an alias
for `slice` would reverse both its failure and ownership contracts.

Rejected alternatives:

- **Clamp like `substring`.** The guide describes strings as bounds-checked views, and silently
  shortening a requested slice hides the caller's bad extent.
- **Allocate the result.** A slice is representable by the existing `{data, count}` pair, and an
  allocation would introduce a free obligation the call does not advertise.
- **Use `start + count` in the assertion.** An overflowing bounds check is not a bounds diagnostic.

### 2. `copy_string` is the named owned copy

`copy_string(s) -> string` delegates to the allocating `substring(s, 0, s.count)` path. The result
is independent mutable storage and is released with `free_string`.

The wrapper is useful even though its implementation is one line. `to_string(*u8)` and `slice`
borrow; `copy_string` is the explicit operation that turns either view into owned storage. Its
absence had already left a dangling documentation link in `to_string`.

### 3. Compatibility spellings delegate rather than reimplement

The following public procedures are wrappers over the existing primitive shown:

| Public spelling | Existing implementation |
|---|---|
| `begins_with(s, prefix)` | `starts_with` |
| `compare_strings(a, b)` | `compare` |
| `find_index_from_left(s, substring)` | `find` |
| `find_index_from_right(s, substring)` | `find_from_right` |
| `replace_chars(s, chars, replacement)` | `replace_chars_in_place` |
| `is_any(c, chars)` | `contains_byte(chars, c)` |

Both names remain exported where the repository already teaches the Jairs spelling. These are
compatibility aliases, not duplicated algorithms; one implementation continues to decide each
answer.

### 4. Conversion aliases preserve the richer parsers

The guide demonstrates:

```text
string_to_int(string) -> (s64, bool)
string_to_float(string) -> (float64, bool)
parse_int(*string) -> (s64, bool)
```

`string_to_int` delegates to `to_integer` and discards its borrowed remainder.

ADR-0198 assigned the name `string_to_float` to a three-result prefix parser. That makes the
guide's two-binding call fail exact result arity. This ADR amends the name, not the capability:

- the existing three-result parser becomes `to_float`, symmetric with `to_integer`;
- `string_to_float` delegates to it and returns only value and success.

`parse_int` calls `to_integer`, replaces the pointed-to string with the remainder on success, and
returns value and success. It therefore consumes the parsed prefix; parsing an all-digit string
leaves an empty borrowed view, matching the guide's observed use. Failure leaves the input
unchanged.

Rejected alternatives:

- **Keep the three-result `string_to_float` and call it compatible.** Exact multi-result arity makes
  the demonstrated two-binding program fail.
- **Delete the remainder-aware float parser.** It is the useful sibling of `to_integer` and already
  has corpus coverage for partial numbers and incomplete exponents.
- **Infer a `parse_float` signature.** The guide names it but does not demonstrate its call or exact
  contract, so public evidence is insufficient.

### 5. Exact overload families wait for general overloading

This wave does not fake overloads with more ad-hoc names. The queued general-overloading wave owns:

- `contains`, `find`, `find_index_from_left` and `split_from_left` over both `string` and `u8`;
- `split` over both separator forms;
- `to_string(*u8)`, `to_string(*u8, count)` and the view form;
- variadic/defaulted `join`;
- the `String.to_upper` family beside `Basic.to_upper(u8)`.

The existing explicit spellings remain usable until then: `contains_byte`, `find_byte`,
`find_byte_from_right`, `split_by_byte` and `borrow`.

Path helpers remain in `File_Utilities`. That is an intentional placement divergence recorded by
ADR-0197, not a missing `String` algorithm.

### 6. Ownership documentation is part of the surface

The module header must describe its present allocating and borrowing halves rather than claiming
nothing allocates. Public comments must distinguish:

- borrowed: `borrow`, `slice`, trims, split halves, parse remainders and `to_string`;
- owned: `copy_string`, `concat`, `substring`, case copies, `join` and `replace`;
- transferred: `adopt`.

`to_c_string` returns `*u8`, so its caller releases the pointer with
`context.allocator_free(pointer)` or first wraps it with `adopt(pointer, count)` and then calls
`free_string`. Telling a caller to pass a pointer directly to `free_string(string)` is not an
actionable contract.

## Consequences

- Guide-shaped non-overload String examples compile without replacing the existing Jairs-oriented
  primitives.
- A slice has one explicit lifetime/ownership contract and one strict failure contract.
- Remainder-aware float parsing survives under a name consistent with `to_integer`.
- Public wrappers remain shallow; algorithm fixes still happen in one place.
- The remaining source-compatibility gaps are accurately attributed to general overloading or to
  deliberate module placement.
- Successful behavior is covered by a corpus program; invalid slices are compared between the VM
  and native engine because their observable result is a trap.
- This is a module-only wave. Gate 7 is required only if implementation exposes a compiler, MIR or
  backend defect.
