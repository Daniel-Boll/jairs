/**
 * Tree-sitter grammar for Jairs — a Jai-inspired systems language.
 *
 * IMPORTANT: This grammar is for EDITOR USE ONLY (highlighting, folding,
 * structural navigation in Neovim/Zed/GitHub). It is NOT the compiler's
 * parser. The authoritative parser lives in crates/jr-syntax. The
 * tests/corpus/valid/ files are the shared source of truth; the CI
 * corpus-drift job enforces that both parsers agree on them.
 *
 * External scanner (src/scanner.c) is required for nested block comments
 * because tree-sitter's regex engine cannot express balanced nesting.
 *
 * Design notes on conflicts and ambiguities:
 *
 * 1. All directives lex as the same DIRECTIVE token. The grammar cannot
 *    distinguish `#import` from `#run` from `#system_library` at the token
 *    level. We resolve this by:
 *    - `import_decl`: directive + string_literal + ';'  (highest prec at item level)
 *    - `run_decl`:    directive + expr + ';'            (lower prec)
 *    - `run_expr`:    directive + expr                  (in expression position)
 *    - `directive_expr`: bare directive (no argument)   (fallback)
 *    The `#system_library "c"` case parses as `run_expr` where the inner
 *    expression is a `literal_expr` (string). This is semantically correct
 *    for the editor grammar's purposes.
 *
 * 2. `run_expr` precedence: `#run` should consume the entire following
 *    expression (like a prefix operator with very low binding power). We give
 *    it precedence -1 so that binary operators inside it reduce first.
 *
 * 3. `_body` (braces-optional if/while body): the dangling-else problem.
 *    Resolved with `prec.right` on `if_stmt` and a conflict declaration.
 *
 * 4. `uninit_expr` (`---`) is part of `_expr` so it can appear anywhere an
 *    expression is expected, including `name : T = ---;`.
 *
 * 5. `arg_list` vs `paren_expr`: `(expr)(` is ambiguous — is the first `(expr)`
 *    a paren_expr or the start of a call? We declare this as a GLR conflict.
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

module.exports = grammar({
  name: "jairs",

  // The external scanner handles nested block comments.
  externals: ($) => [$.block_comment],

  // Whitespace and line comments are extras (skipped between tokens).
  extras: ($) => [/\s/, $.line_comment, $.block_comment],

  conflicts: ($) => [
    // Dangling-else: prefer attaching else to the innermost if. `prec.right`
    // on `if_stmt` plus this pair is what resolves it; `[$._body]` on its own
    // was reported as an unnecessary conflict by `tree-sitter generate`, so it
    // is deliberately absent.
    [$.if_stmt, $._single_stmt],
    // `(expr)(` — is the first `(expr)` a paren_expr, or the callee of a call?
    [$.arg_list, $.paren_expr],
  ],

  word: ($) => $.identifier,

  rules: {
    // -----------------------------------------------------------------------
    // Root
    // -----------------------------------------------------------------------
    source_file: ($) => repeat($._item),

    // -----------------------------------------------------------------------
    // Items (top-level)
    // -----------------------------------------------------------------------
    _item: ($) =>
      choice(
        $.import_decl,
        $.run_decl,
        $._decl,
      ),

    // #import "Basic";
    // Must have higher precedence than run_decl to win when the directive is
    // followed by a string literal and a semicolon.
    import_decl: ($) =>
      prec(
        2,
        seq(field("directive", $.directive), field("path", $.string_literal), ";"),
      ),

    // #run expr;  (top-level, executed for side effects)
    run_decl: ($) =>
      prec(
        1,
        seq(field("directive", $.directive), field("expr", $._expr), ";"),
      ),

    // -----------------------------------------------------------------------
    // Declarations
    // -----------------------------------------------------------------------
    _decl: ($) => choice($.const_decl, $.var_decl),

    // name :: ConstValue
    const_decl: ($) =>
      seq(
        field("name", $.name),
        "::",
        field("value", $._const_value),
      ),

    // name := expr;
    // name : Type ;
    // name : Type = expr ;   (expr includes uninit_expr for `---`)
    var_decl: ($) =>
      choice(
        seq(
          field("name", $.name),
          ":=",
          field("value", $._expr),
          ";",
        ),
        seq(
          field("name", $.name),
          ":",
          field("type", $._type),
          optional(seq("=", field("value", $._expr))),
          ";",
        ),
      ),

    // -----------------------------------------------------------------------
    // Const values (right-hand side of ::)
    // -----------------------------------------------------------------------
    _const_value: ($) =>
      choice(
        $.struct_type,
        $.proc,
        seq($._expr, ";"),
      ),

    // -----------------------------------------------------------------------
    // Procedures
    // -----------------------------------------------------------------------
    proc: ($) =>
      seq(
        field("params", $.param_list),
        optional(field("ret_type", $.ret_type)),
        choice(
          field("body", $.block),
          seq(field("foreign", $.foreign_attr), ";"),
        ),
      ),

    param_list: ($) =>
      seq(
        "(",
        optional(
          seq(
            $.param,
            repeat(seq(",", $.param)),
            optional(","),
          ),
        ),
        ")",
      ),

    // param: name ':' type — no field() on name to keep S-expr clean
    param: ($) =>
      seq($.identifier, ":", field("type", $._type)),

    ret_type: ($) => seq("->", field("type", $._type)),

    // #foreign libc "write"
    foreign_attr: ($) =>
      seq(
        field("directive", $.directive),
        field("lib", $.identifier),
        optional(field("symbol", $.string_literal)),
      ),

    // -----------------------------------------------------------------------
    // Types
    // -----------------------------------------------------------------------
    _type: ($) => choice($.pointer_type, $.name_type, $.struct_type),

    // *T
    pointer_type: ($) => seq("*", field("inner", $._type)),

    // s64, bool, Point, ... — no field() wrapper, just the identifier
    name_type: ($) => $.identifier,

    // struct { ... }
    struct_type: ($) =>
      seq(
        "struct",
        field("fields", $.field_list),
      ),

    field_list: ($) => seq("{", repeat($.field), "}"),

    // struct field: name ':' type ';' — no field() on name
    field: ($) =>
      seq($.identifier, ":", field("type", $._type), ";"),

    // -----------------------------------------------------------------------
    // Statements
    // -----------------------------------------------------------------------
    block: ($) => seq("{", repeat($._stmt), "}"),

    _stmt: ($) =>
      choice(
        $.block,
        $.if_stmt,
        $.while_stmt,
        $.return_stmt,
        $.break_stmt,
        $.continue_stmt,
        $.decl_stmt,
        $.assign_stmt,
        $.expr_stmt,
      ),

    // A declaration used as a statement.
    decl_stmt: ($) => $._decl,

    // if cond Body (else (if_stmt | Body))?
    if_stmt: ($) =>
      prec.right(
        seq(
          "if",
          field("condition", $._expr),
          field("body", $._body),
          optional(
            seq(
              "else",
              field("else_branch", choice($.if_stmt, $._body)),
            ),
          ),
        ),
      ),

    // while cond Body
    while_stmt: ($) =>
      seq(
        "while",
        field("condition", $._expr),
        field("body", $._body),
      ),

    // Body = Block | single Stmt (braces optional per corpus 010)
    _body: ($) => choice($.block, $._single_stmt),

    // Statements that can appear without braces after if/while.
    // Excludes block (already handled) and decl_stmt (ambiguous without braces).
    _single_stmt: ($) =>
      choice(
        $.if_stmt,
        $.while_stmt,
        $.return_stmt,
        $.break_stmt,
        $.continue_stmt,
        $.assign_stmt,
        $.expr_stmt,
      ),

    return_stmt: ($) =>
      seq("return", optional(field("value", $._expr)), ";"),

    break_stmt: (_$) => seq("break", ";"),

    continue_stmt: (_$) => seq("continue", ";"),

    // lhs AssignOp rhs ;
    assign_stmt: ($) =>
      seq(
        field("lhs", $._expr),
        field("op", $._assign_op),
        field("rhs", $._expr),
        ";",
      ),

    // expr ;
    expr_stmt: ($) => seq($._expr, ";"),

    // -----------------------------------------------------------------------
    // Assignment operators
    // -----------------------------------------------------------------------
    _assign_op: (_$) =>
      choice(
        "=",
        "+=",
        "-=",
        "*=",
        "/=",
        "%=",
        "+%=",
        "-%=",
        "*%=",
      ),

    // -----------------------------------------------------------------------
    // Expressions — precedence (lowest to highest)
    //   -1. directive expressions (run_expr, directive_expr) — lowest
    //    1. ||
    //    2. &&
    //    3. == != < <= > >=
    //    4. + - +% -%
    //    5. * / % *%
    //    6. prefix - ! * (address-of)
    //    7. postfix .field  .*  (args)
    // -----------------------------------------------------------------------
    _expr: ($) =>
      choice(
        $.binary_expr,
        $.unary_expr,
        $.deref_expr,
        $.field_expr,
        $.call_expr,
        $.paren_expr,
        $.literal_expr,
        $.name_expr,
        $.uninit_expr,
        $.run_expr,
        $.directive_expr,
      ),

    // Binary expressions with explicit precedence levels.
    binary_expr: ($) =>
      choice(
        // Precedence 1: ||
        prec.left(
          1,
          seq(
            field("lhs", $._expr),
            field("op", "||"),
            field("rhs", $._expr),
          ),
        ),
        // Precedence 2: &&
        prec.left(
          2,
          seq(
            field("lhs", $._expr),
            field("op", "&&"),
            field("rhs", $._expr),
          ),
        ),
        // Precedence 3: == != < <= > >=
        prec.left(
          3,
          seq(
            field("lhs", $._expr),
            field("op", choice("==", "!=", "<", "<=", ">", ">=")),
            field("rhs", $._expr),
          ),
        ),
        // Precedence 4: + - +% -%
        prec.left(
          4,
          seq(
            field("lhs", $._expr),
            field("op", choice("+", "-", "+%", "-%")),
            field("rhs", $._expr),
          ),
        ),
        // Precedence 5: * / % *%
        prec.left(
          5,
          seq(
            field("lhs", $._expr),
            field("op", choice("*", "/", "%", "*%")),
            field("rhs", $._expr),
          ),
        ),
      ),

    // Prefix unary: - ! * (address-of)
    unary_expr: ($) =>
      prec.right(
        6,
        seq(
          field("op", choice("-", "!", "*")),
          field("operand", $._expr),
        ),
      ),

    // Postfix dereference: expr.*
    deref_expr: ($) =>
      prec.left(
        7,
        seq(field("operand", $._expr), ".*"),
      ),

    // Field access: expr.name
    field_expr: ($) =>
      prec.left(
        7,
        seq(
          field("operand", $._expr),
          ".",
          field("field", $.identifier),
        ),
      ),

    // Function call: expr(args)
    call_expr: ($) =>
      prec.left(
        7,
        seq(
          field("function", $._expr),
          field("args", $.arg_list),
        ),
      ),

    arg_list: ($) =>
      seq(
        "(",
        optional(
          seq(
            $._expr,
            repeat(seq(",", $._expr)),
            optional(","),
          ),
        ),
        ")",
      ),

    // Parenthesised expression
    paren_expr: ($) => seq("(", field("inner", $._expr), ")"),

    // Literals
    literal_expr: ($) =>
      choice(
        $.integer_literal,
        $.float_literal,
        $.string_literal,
        $.true,
        $.false,
      ),

    // Name reference
    name_expr: ($) => $.identifier,

    // --- (explicitly uninitialised)
    uninit_expr: (_$) => "---",

    // #run expr  (compile-time evaluation, used as an expression)
    // Also handles #system_library "c" — the string is parsed as a literal_expr.
    // Precedence -1: lower than all binary operators, so `#run a + b` parses
    // as `#run (a + b)` not `(#run a) + b`.
    run_expr: ($) =>
      prec.right(
        -1,
        seq(field("directive", $.directive), field("expr", $._expr)),
      ),

    // Bare directive with no argument (e.g. a future #no_abc annotation).
    // Lowest precedence.
    directive_expr: ($) =>
      prec(
        -2,
        field("directive", $.directive),
      ),

    // -----------------------------------------------------------------------
    // Terminals
    // -----------------------------------------------------------------------

    // The `name` node wraps an identifier in declaration position.
    name: ($) => $.identifier,

    identifier: (_$) => /[A-Za-z_][A-Za-z0-9_]*/,

    // Directives: # immediately followed by an identifier, lexed as one token.
    directive: (_$) => /#[A-Za-z_][A-Za-z0-9_]*/,

    // Integer literals: decimal, 0x, 0b, 0o, with _ separators.
    integer_literal: (_$) =>
      token(
        choice(
          /[0-9][0-9_]*/,
          /0[xX][0-9a-fA-F][0-9a-fA-F_]*/,
          /0[bB][01][01_]*/,
          /0[oO][0-7][0-7_]*/,
        ),
      ),

    // Float literals: 1.5, 1.0e9, 1.5E+10
    float_literal: (_$) =>
      token(
        seq(
          /[0-9][0-9_]*/,
          ".",
          /[0-9][0-9_]*/,
          optional(seq(/[eE]/, optional(/[+-]/), /[0-9][0-9_]*/)),
        ),
      ),

    // String literals: "..." with escape sequences, not multi-line.
    string_literal: (_$) =>
      token(
        seq(
          '"',
          repeat(
            choice(
              /[^"\\"\n]+/,
              /\\[nrt0\\"]/,
              /\\u[0-9a-fA-F]{4}/,
            ),
          ),
          '"',
        ),
      ),

    // Boolean literals
    true: (_$) => "true",
    false: (_$) => "false",

    // Line comment: // to end of line
    line_comment: (_$) => token(seq("//", /.*/)),

    // block_comment is handled by the external scanner (nested /* ... */)
  },
});
