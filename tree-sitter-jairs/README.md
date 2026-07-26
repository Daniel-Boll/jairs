# tree-sitter-jairs

Tree-sitter grammar for the [Jairs](https://github.com/dboll/jairs) programming language.

> **IMPORTANT — Editor use only.**
> This grammar exists for syntax highlighting, code folding, and structural
> navigation in editors (Neovim, Zed, GitHub). It is **not** the compiler's
> parser. The authoritative parser lives in `crates/jr-syntax`. The two parsers
> are held in sync by the `corpus-drift` CI job.

## Source of truth

The files in `../tests/corpus/valid/` are simultaneously:
1. Spec examples referenced by `docs/spec/`
2. Rust compiler parser tests (`jr-syntax`)
3. Tree-sitter grammar tests (this package)

**Every grammar change requires a corpus file.** See `../tests/corpus/README.md`.

## Setup

Node.js 18+ and npm are required. The tree-sitter CLI is a dev dependency.

```sh
cd tree-sitter-jairs
npm install
```

## Regenerate the parser

```sh
npm run generate
# or
npx tree-sitter generate
```

The generated `src/parser.c` and `src/tree_sitter/` are excluded from version
control (see root `.gitignore`). The hand-written `src/scanner.c` **is**
committed — it implements nested block comment lexing, which tree-sitter's
regex engine cannot express.

## Run the grammar's own tests

```sh
npm test
# or
npx tree-sitter test
```

## Run the drift gate (the actual acceptance test)

```sh
npm run parse-corpus
# or
npx tree-sitter parse --quiet ../tests/corpus/valid/*.jr
```

This command exits non-zero if any `ERROR` node appears in any valid corpus
file. The CI `corpus-drift` job runs this automatically on every push.

## Negative control

Invalid corpus files should produce errors:

```sh
npx tree-sitter parse --quiet ../tests/corpus/invalid/*.jr
# Expected: non-zero exit, ERROR or MISSING nodes in output
```

## External scanner

`src/scanner.c` is a hand-written C scanner required for **nested block
comments**. Jairs block comments nest (`/* outer /* inner */ still outer */`),
which is not expressible with tree-sitter's built-in regex tokens. The scanner
tracks comment depth and handles unterminated comments gracefully (consuming
them as a `block_comment` token so the rest of the parse can continue).

## Node names vs compiler SyntaxKind

| tree-sitter node | compiler `SyntaxKind` | Notes |
|---|---|---|
| `source_file` | `SOURCE_FILE` | |
| `const_decl` | `CONST_DECL` | |
| `var_decl` | `VAR_DECL` | |
| `import_decl` | `IMPORT_DECL` | |
| `run_decl` | `RUN_DECL` | |
| `name` | `NAME` | |
| `proc` | `PROC` | |
| `param_list` | `PARAM_LIST` | |
| `param` | `PARAM` | |
| `ret_type` | `RET_TYPE` | |
| `foreign_attr` | `FOREIGN_ATTR` | |
| `name_type` | `NAME_TYPE` | |
| `pointer_type` | `POINTER_TYPE` | |
| `struct_type` | `STRUCT_TYPE` | |
| `field_list` | `FIELD_LIST` | |
| `field` | `FIELD` | |
| `block` | `BLOCK` | |
| `decl_stmt` | `DECL_STMT` | |
| `expr_stmt` | `EXPR_STMT` | |
| `assign_stmt` | `ASSIGN_STMT` | |
| `if_stmt` | `IF_STMT` | |
| `while_stmt` | `WHILE_STMT` | |
| `return_stmt` | `RETURN_STMT` | |
| `break_stmt` | `BREAK_STMT` | |
| `continue_stmt` | `CONTINUE_STMT` | |
| `literal_expr` | `LITERAL_EXPR` | |
| `name_expr` | `NAME_EXPR` | |
| `binary_expr` | `BINARY_EXPR` | |
| `unary_expr` | `UNARY_EXPR` | |
| `paren_expr` | `PAREN_EXPR` | |
| `call_expr` | `CALL_EXPR` | |
| `arg_list` | `ARG_LIST` | |
| `field_expr` | `FIELD_EXPR` | |
| `deref_expr` | `DEREF_EXPR` | |
| `uninit_expr` | `UNINIT_EXPR` | |
| `run_expr` | `RUN_EXPR` | Also used for `#system_library "c"` |
| `directive_expr` | `DIRECTIVE_EXPR` | Bare directive, no argument |
| `identifier` | `IDENT` | |
| `directive` | `DIRECTIVE` | |
| `integer_literal` | `INT_LITERAL` | |
| `float_literal` | `FLOAT_LITERAL` | |
| `string_literal` | `STRING_LITERAL` | |
| `true` | `TRUE_KW` | |
| `false` | `FALSE_KW` | |
| `line_comment` | `LINE_COMMENT` | |
| `block_comment` | `BLOCK_COMMENT` | |

### Deliberate divergences

- **`run_expr` also handles `#system_library "c"`**: In the compiler, this is
  a `DIRECTIVE_EXPR` with an optional string argument. In the tree-sitter
  grammar, since all directives lex identically, `#system_library "c"` parses
  as a `run_expr` where the inner expression is a `literal_expr` (string). This
  is correct for editor purposes — the highlight query treats all directives
  uniformly.

- **`else_branch` field**: The compiler uses `ELSE_BRANCH` as a separate node.
  In tree-sitter, the else branch is a field on `if_stmt` pointing directly to
  either another `if_stmt` or a `block`/`_single_stmt`.

- **Reserved keywords**: The compiler lexes `enum`, `union`, `for`, `defer`,
  `using`, `cast`, `xx`, `null` as keyword tokens. In tree-sitter, they lex as
  `identifier` tokens and are highlighted as reserved keywords via a query
  pattern match. This avoids grammar conflicts while still giving editors the
  correct highlight class.
