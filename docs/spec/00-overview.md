# 00 — Overview

## What Jairs is

Jairs is a Jai-inspired systems programming language, compiled by a
hand-written, error-recovering compiler written in Rust. It is a language for
programs that care about memory layout and machine cost: there is **no garbage
collector**, **no RAII**, and **no exceptions**. Memory management is explicit
and allocator-driven, control flow is ordinary and visible, and errors are
values.

Jairs is being built as a **vertical tracer-bullet slice** first — the tiny
"Jairs-0" subset described below is driven all the way through every compiler
component (lexer, parser, CST, HIR, Sema, MIR, bytecode VM, Cranelift backend,
linker, FFI, an in-language stdlib module, LSP, tree-sitter grammar, formatter)
until `hello.jr` is a native binary with IDE support. Only then is the language
thickened, one feature wave at a time (`PLAN.md` §2).

> **Status: pre-alpha. Nothing works yet.** This specification describes the
> Jairs-0 design; the implementation is in progress. Where a feature is
> unimplemented, this spec says so.

## Design values

| Value | What it means in Jairs |
|---|---|
| **No GC** | Memory is managed explicitly; there is no tracing collector and no reference counting baked into the language. |
| **No RAII** | Objects have no implicit destructors. Cleanup is explicit (`defer` arrives in wave W2). |
| **No exceptions** | Errors are ordinary values, returned and handled explicitly. The starting model is Jai's multiple-return-values plus `#must` (ADR-0008). |
| **Explicit allocators** | Allocation goes through an explicit allocator, carried by the implicit `context` (a hidden trailing parameter, ADR-0001). The context arrives in wave W3. |
| **Compile-time execution** | Ordinary code can run at compile time in a bytecode VM via `#run`; the VM and the native backend execute the *same* MIR, so comptime and runtime cannot silently disagree. |
| **Fast compilation** | Cranelift is the first backend (≈10× faster to compile than LLVM); the parser is hand-written for speed and diagnostic quality; analysis is incremental via salsa (ADR-0007). |
| **Overflow is an error** | Integer overflow always traps — never wraps silently, never invokes undefined behaviour, never differs between debug and release (ADR-0002). Explicit wrapping operators exist for the code that needs modular arithmetic. |

## Reading this specification

Every feature below is shown with a runnable example drawn from
[`tests/corpus/valid/`](../../tests/corpus/valid/), cited by filename. Those
corpus files are the ground truth for syntax. Load-bearing decisions link to
their [ADR](../adr/README.md). Anything not yet implemented names the wave
(`PLAN.md` §2.1) that adds it.

## The Jairs-0 subset boundary

Jairs-0 is deliberately tiny. The following table states exactly what **is** and
**is not** in the language today. Everything absent arrives in a named wave.

### In Jairs-0 today

| Feature | Notes | Corpus |
|---|---|---|
| `s64` integer type | The workhorse integer. | `022-integer-literals.jr` |
| `bool` type, `true` / `false` | | `005-decl-typed.jr`, `014-comparison-logical.jr` |
| `string` type | `{data: *u8, count: s64}`, not NUL-terminated (ADR-0004). | `021-string-literals.jr` |
| `*T` pointer type | Prefix `*` address-of; postfix `.*` dereference (ADR-0011). | `015-pointers.jr` |
| `struct { … }` (one level) | Fields are `name: T;`. Structs are constants (ADR-0012). | `008-struct.jr` |
| Procedures, single return | `name :: (params) -> T { … }`; return type omitted for none. Procedures are constants (ADR-0012). | `004-proc-params-return.jr` |
| Declarations: `::`, `:=`, `: T [= value]` | Constant, inferred, and typed forms (chapter 02). | `007-constants.jr`, `006-decl-inferred.jr`, `005-decl-typed.jr` |
| `---` explicit non-initialisation | Suppresses default zeroing. | `005-decl-typed.jr` |
| `if` / `else` / `else if` | Braces required; parentheses around the condition are not. A single unbraced statement is allowed. | `010-if-else.jr` |
| `while` | | `011-while.jr` |
| `break` / `continue` | | `011-while.jr` |
| `return` | | `004-proc-params-return.jr` |
| Blocks and block scope | `{ … }` introduces a scope; shadowing is allowed. | `023-block-scope.jr` |
| Arithmetic `+ - * / %` (trapping) | Overflow traps (ADR-0002). Unary `-`. `*` binds tighter than `+`. | `012-arithmetic.jr` |
| Wrapping arithmetic `+% -% *%` | Modular; needed for hashes/PRNGs/checksums (ADR-0002). | `013-wrapping-ops.jr` |
| Comparison `== != < <= > >=` | | `014-comparison-logical.jr` |
| Logical `&& || !` | `&&` / `||` short-circuit. | `014-comparison-logical.jr` |
| Assignment `=` and compound `+= -= *= /= %=` `+%= -%= *%=` | Compound arithmetic traps like its binary form. | `016-assignment.jr` |
| Field access `a.b`, nested `a.b.c` | Auto-dereferences through pointers. | `009-field-access.jr` |
| Calls `f(a, b)`, nested | A discarded call is a statement. | `017-call.jr` |
| `#import "Name";` | One module, one file. | `018-import.jr` |
| `#foreign` / `#system_library` | FFI to libc; foreign procs are `#c_call` (ADR-0001). | `019-foreign.jr` |
| One trivial `#run` | Comptime call, top-level statement or constant initialiser. | `020-run-directive.jr` |
| Integer literals: decimal, `0x`, `0b`, `0o`, `_` separators | | `022-integer-literals.jr` |
| String literals + escapes | `\n \r \t \0 \\ \" \uXXXX` (chapter 01). | `021-string-literals.jr` |
| Line and block comments; block comments **nest** | | `002-comments.jr` |

### Not in Jairs-0 — and the wave that adds each

| Absent feature | Added in |
|---|---|
| Full numeric tower (`s8`–`s32`, `u16`–`u64`, `float32`/`float64`), `cast()`, `xx` autocast, operator overloading | **W1** |
| `enum`, `enum_flags`, `union` | **W1** |
| Arrays `[N]T`, views `[]T`, dynamic arrays `[..]T` | **W1** |
| Bitwise operators `& | ^ ~ << >>` | **W1** |
| Float *literals* usable (they lex today but the parser rejects them) | **W1** |
| `for` (with `it` / `it_index`, `for <`), labelled `break`/`continue`, `defer`, `using`, multiple return values, named/default args, `#scope_*` | **W2** |
| The `context` value, allocators, temporary storage, the bounds-check build config, panics/traps with backtraces | **W3** |
| Full `#run` (arbitrary code), aggressive const folding, RTTI (`Type` values, `type_info()`, `Any`), `#insert`, `#code`, the `Code` type | **W4** |
| Polymorphs `$T` / `$$T`, `#modify`, `#bake_arguments`, `#expand` macros, instantiation caching | **W5** |
| Workspaces, the compiler message loop, `#run build()` build scripts, plugin hooks, `@note` attributes | **W6** |
| The in-Jairs standard library beyond the slice's `Basic` | **W7** |
| LLVM backend, `#soa`, SIMD, `#align`/`#place`, parallel Sema/codegen | **W8** |
| Full LSP surface (completion, rename, inlay hints, …), richer DWARF | **W9** |
| Graphics, GPU, UI, audio — all as libraries in Jairs | **W10** |

> **A note on `u8`.** The corpus uses `u8` in type position — `*u8` inside the
> `string` layout and in the foreign `write` signature (`019-foreign.jr`,
> `021-string-literals.jr`), and `g: u8 = 255;` in `005-decl-typed.jr`. `u8` is
> the one member of the wider numeric tower that Jairs-0 must recognise as a
> type name, because the string ABI and the libc FFI boundary are expressed in
> terms of it. The rest of the numeric tower (and general `u8` arithmetic) is a
> wave W1 concern; in Jairs-0, `u8` exists so that `*u8` and byte-sized FFI
> arguments can be spelled. See chapter 02 and the reporting note in the ADR
> index if this boundary needs sharpening.

## The whole slice at a glance

The Jairs-0 program that the entire compiler is built to handle end-to-end
(`tests/corpus/valid/024-hello.jr`):

```jr
#import "Basic";

Point :: struct {
    x: s64;
    y: s64;
}

MESSAGE  :: "hello from Jairs\n";
COMPUTED :: #run add(2, 3);

add :: (a: s64, b: s64) -> s64 {
    return a + b;
}

main :: () {
    p: Point;
    p.x = 4;
    p.y = COMPUTED;

    sum := add(p.x, p.y);
    if sum > 5 {
        print(MESSAGE);
    }

    i := 0;
    while i < 3 {
        i = i + 1;
    }

    ptr := *sum;
    print_int(ptr.*);
    print("\n");
}
```

`print` and `print_int` come from `modules/Basic`, written in Jairs itself,
which reaches libc `write` through `#foreign` (`PLAN.md` §1.2). That is the
proof that "the standard library is written in Jairs" — the bottom of the
stdlib is a syscall, so FFI and the string ABI cannot be deferred past the
slice.
