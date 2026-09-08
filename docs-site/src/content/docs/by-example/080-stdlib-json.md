---
title: "JSON: a flat node store"
description: "A parser and inspector over a growable array of nodes addressed by index rather than pointer, with no serialiser and no #scope_module."
sidebar:
  order: 80
---

`JSON` parses text into one flat `[..]Json_Node` array and hands back **indices**, not pointers, into it
(ADR-0156). A JSON value is naturally recursive — an array holds values, an object holds named values —
and a recursive type needs indirection; this module chooses an `s64` index over a pointer for three
reasons: freeing the whole document is one call rather than a recursive walk, a handle is copyable and
comparable with no ownership question attached, and a flat array is what a parser produces and a walk
consumes in the same order.

## The node store

```jr
/// The absent index — no first child, no next sibling, no such member.
///
/// `-1` rather than `0`, because `0` is the root's own index in every non-empty document and a sentinel
/// that collides with a real value is the bug this constant exists to prevent.
NONE :: -1;

/// What one node is.
Json_Kind :: enum {
    /// `null`.
    NULL_;
    /// `true`.
    TRUE_;
    /// `false`.
    FALSE_;
    /// A number. `number` holds it; `integer` too when `is_integer` is true.
    NUMBER;
    /// A string. `text` holds it, unescaped, and the document owns it.
    STRING;
    /// An array. `first` is its first element, or `NONE`.
    ARRAY;
    /// An object. `first` is its first member, or `NONE`; each member's `key` is set.
    OBJECT;
}

/// One value in the store.
Json_Node :: struct {
    /// Which kind this is.
    kind: Json_Kind;
    /// The number, when `kind` is `NUMBER`.
    number: float64;
    /// The exact integer, when `kind` is `NUMBER` and `is_integer` is true.
    integer: s64;
    /// Whether the source number had no fraction and no exponent, so `integer` is exact.
    is_integer: bool;
    /// The string, when `kind` is `STRING`. Owned by the document.
    text: string;
    /// This member's name, when the parent is an `OBJECT`. Owned by the document.
    key: string;
    /// The first child, when `kind` is `ARRAY` or `OBJECT`; `NONE` when empty.
    first: s64;
    /// The next sibling under the same parent; `NONE` when last.
    next: s64;
}

/// A parsed document: the node store, plus the root's index.
Json_Document :: struct {
    /// Every node, in the order the parse produced them.
    nodes: [..]Json_Node;
    /// The root value's index, or `NONE` when the parse failed.
    root: s64;
    /// The byte offset where the parse stopped, when it failed. `-1` on success.
    error_at: s64;
}

/// Parses `text` into a document.
///
/// `#must`: the result carries a success flag beside the document, and ignoring it has to be *written*
/// (ADR-0151). A parse of untrusted input is the case that marker exists for — a caller who ignores the
/// flag reads `root == NONE` as an empty document and gets silently wrong answers.
parse :: (text: string) -> (Json_Document, bool) #must { ... }

/// Releases every node and every string the document owns.
///
/// Safe on a failed parse and safe to call twice: the array is emptied, so the second call walks nothing.
free_document :: (doc: *Json_Document) { ... }
```

`Json_Node` is a struct with every field present rather than a `variant`, which is a deliberate reversal
of the module's own preference for tagged unions elsewhere in the library: a variant would refuse a read
of the wrong case, which is exactly right — but the *parser* fills fields in an order the tag does not
yet know, and a node is the same size either way once a `string` and a `float64` are in it. So `kind` is
advisory and the accessors below do the checking a variant would have done, returning a stated default
rather than trapping — a caller inspecting untrusted JSON is asking a question, not asserting a fact.

## Walking the tree

```jr
/// The kind of node at, or `NULL_` when `at` is not a node in this document.
kind_of :: (doc: *Json_Document, at: s64) -> Json_Kind { ... }

/// The number at at, or 0.0.
number_of :: (doc: *Json_Document, at: s64) -> float64 { ... }

/// The exact integer at at, or 0.
///
/// Separate from `number_of` because going through `float64` loses precision above 2^53, and a caller
/// indexing or counting wants the exact value. `is_integer_at` asks whether this is meaningful.
integer_of :: (doc: *Json_Document, at: s64) -> s64 { ... }

/// Whether at is an integral number.
is_integer_at :: (doc: *Json_Document, at: s64) -> bool { ... }

/// The string at at, or the empty string when it is not a string.
///
/// **Borrowed, not copied**: the document owns it and `free_document` releases it, so a caller who needs
/// it to outlive the document must `concat` or `substring` it.
string_of :: (doc: *Json_Document, at: s64) -> string { ... }

/// Whether at is true.
is_true :: (doc: *Json_Document, at: s64) -> bool { ... }

/// How many elements the array at has, or 0 when it is not an array.
array_count :: (doc: *Json_Document, at: s64) -> s64 { ... }

/// Element index of the array at, or NONE.
array_at :: (doc: *Json_Document, at: s64, index: s64) -> s64 { ... }

/// How many members the object at has.
member_count :: (doc: *Json_Document, at: s64) -> s64 { ... }

/// Member index in source order, or NONE.
member_at :: (doc: *Json_Document, at: s64, index: s64) -> s64 { ... }

/// The borrowed name of member node at.
key_of :: (doc: *Json_Document, at: s64) -> string { ... }

/// First member named name, or NONE.
member :: (doc: *Json_Document, at: s64, name: string) -> s64 { ... }

/// Whether the object at has member name.
has_member :: (doc: *Json_Document, at: s64, name: string) -> bool { ... }
```

```jr
#import "Basic";
#import "JSON";
#import "String";

/// The allocate half of an allocator: a `#foreign` procedure cannot fill a procedure-pointer field
/// directly (E0256), so a Jairs wrapper does.
libc_alloc :: (n: s64) -> *u8 {
    return malloc(n);
}

/// The release half.
libc_free :: (p: *u8) {
    free(p);
}

main :: () {
    context.allocator = libc_alloc;
    context.allocator_free = libc_free;
    total := 0;

    shape, shape_ok := parse("{\"a\": 1, \"b\": [10, 20, 30], \"c\": \"hi\", \"d\": true, \"e\": null}");
    if shape_ok {
        if member_count(*shape, shape.root) == 5 {
            total = total + 1;
        }
        if integer_of(*shape, member(*shape, shape.root, "a")) == 1 {
            total = total + 2;
        }
        b := member(*shape, shape.root, "b");
        if array_count(*shape, b) == 3 {
            total = total + 4;
        }
        if integer_of(*shape, array_at(*shape, b, 2)) == 30 {
            total = total + 8;
        }
        if equal(string_of(*shape, member(*shape, shape.root, "c")), "hi") {
            total = total + 16;
        }
        // `is_true` of `null` must be false, not "not false" — three kinds rather than a boolean with a
        // flag is what makes that unambiguous.
        if is_true(*shape, member(*shape, shape.root, "d")) {
            if !is_true(*shape, member(*shape, shape.root, "e")) {
                total = total + 32;
            }
        }
    }
    free_document(*shape);

    exit(total);
}
```

This is the shape section of `tests/corpus/valid/127-json.jr`, trimmed and re-verified to exit **63** —
one bit for the member count, the integer read, the array length and its element, the string, and the
`true`/`null` distinction. The full corpus file goes on to check every escape sequence, UTF-8 width,
surrogate pair, and the five inputs a spec-following parser must refuse (a trailing comma, a leading
zero, trailing content, a lone surrogate, `1.`), and exits `148` — its own eleven further bits, modulo
251 because an exit status is a byte.

`member` walks the sibling chain in source order and stops at the first match, so `member_count` is
`O(n)` rather than the `O(1)` a hash map would give — the module's own docs name that trade and reject a
`Map(s64, s64)` per object explicitly, because that concrete instantiation cannot key on a string at all,
and a linear scan over the three-to-five keys a real object usually has is not where a game's JSON
loading time goes. A duplicate key is kept, both copies, in source order; `member` finds the first, and a
caller who cares about the second walks `next` themselves — JSON itself does not say what a duplicate
means, so this module does not invent a rule.

`integer_of` exists beside `number_of` because `9007199254740993` — 2^53 + 1 — has no exact `float64`
representation, so a parser that stored only the float would silently hand back
`9007199254740992`. `is_integer_at` is true only when the source number had no fraction and no exponent,
which is what makes `integer_of`'s answer exact rather than a rounded guess.

## What is absent, and why

`JSON` has <span class="jairs-status absent">absent</span> serialisation. Writing a document back out
needs to render a `float64` as the shortest decimal that round-trips — a correct `dtoa` — and an
approximate one would emit numbers this module could not read back, which is worse than emitting none.
`String`'s allocating half is ready for the rest of it; the missing piece is one function, and it is
named rather than half-built.

There is <span class="jairs-status absent">absent</span> streaming or incremental parsing: the whole text
is already in memory, since a Jairs `string` is data and a count, and a streaming parser would answer a
different question with its own state machine. There is also no depth limit — nesting depth is stack
depth, recorded here rather than left to look like an oversight.

**There is no `#scope_module` anywhere in this file**, so all 47 declarations are public, including the
28 parsing helpers under the module's own `// Internals` and `// Parsing` banner comments —
`skip_space`, `parse_value`, `parse_array`, `parse_object`, `parse_string`, `parse_number`,
`hex_digit`, `write_utf8`, and the rest. Nothing enforces the banner comment; a program can call
`skip_space` directly, and it will work exactly as the parser's own use of it does, because it is the
same function. Treat these as **leaked**, not as API: a docs page that promised them stable would be
promising something this module's own author has not committed to. Note also that its `is_digit :: (c:
s64) -> bool` shares a name with `Basic`'s public `is_digit :: (c: u8) -> bool`, and `strtod` is declared
here (public) as well as in `String` (private) — a program importing `JSON`, `Basic` and `String`
unqualified will meet E0211 on at least one of these before it meets a games-specific collision.

Parsed strings come from `context.allocator` and are released by `String.free_string`, while the node
store itself is `malloc`ed to match `List` and `Map`. A fresh context supplies the string allocator;
installing a custom one changes the *strings* JSON parses but never its node store. That split between
the two routes is a seam this module's own docs name as unresolved rather than designed.

See also [Book I — The Jairs Language](/language/introduction/).
