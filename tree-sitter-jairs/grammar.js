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
    // `name :` — a loop label (ADR-0049 §2) or an ordinary typed declaration? Only the following
    // token tells them apart, so this is a genuine GLR conflict rather than a precedence question:
    // a `prec` silently broke every `n: s64` declaration in the corpus, which gate 6 caught.
    [$.loop_label, $.name],
    // `-> (s64)` — a one-element results list (ADR-0052 §1) or a void-returning procedure pointer
    // (ADR-0062 §1)? Both are now valid readings of the same tokens, and *nothing* after them
    // distinguishes the two, so this is a genuine ambiguity rather than a look-ahead question. A
    // declared conflict lets GLR carry both and settle it; a `prec` would silently pick one, which
    // is the trap `loop_label` and `scope_decl` each walked into. The compiler's parser resolves it
    // the same way it always did — `ret_type` looks for the arrow after the `)`.
    [$.result_list, $.proc_type_params],
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
        $.scope_decl,
        $.run_decl,
        $._decl,
      ),

    // `#scope_module` / `#scope_export` — a bare directive with no argument and no `;`, marking a
    // *position* in the file rather than declaring anything (ADR-0054 §1). Matched on its **text**
    // rather than as a `$.directive` with a precedence: a `prec(3)` made *every* bare directive a
    // scope marker and stranded `#run`'s expression, because every directive lexes as one token.
    scope_decl: (_$) =>
      field("directive", choice("#scope_module", "#scope_export")),

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
    _decl: ($) => choice($.const_decl, $.operator_decl, $.var_decl),

    // operator + :: (a: T, b: T) -> T { … } (ADR-0048 §1).
    //
    // Its own rule rather than a `const_decl` whose name is an operator: `name` is an
    // `identifier` and an operator is not one, so sharing would require loosening `name` for
    // every declaration. The *value* is an ordinary `proc`, because an overload is an ordinary
    // procedure whose name happens to be an operator.
    //
    // Every operator token the compiler's parser accepts is listed, including the ones sema then
    // refuses (ADR-0048 §2): the grammar's job is to produce the tree so a highlighter can colour
    // `operator +%` while the compiler explains why it is not allowed.
    operator_decl: ($) =>
      seq(
        "operator",
        field(
          "op",
          choice(
            "+", "-", "*", "/", "%",
            "==", "!=", "<", "<=", ">", ">=",
            "+%", "-%", "*%",
            "&", "|", "^", "~", "<<", ">>",
            "&&", "||", "!",
          ),
        ),
        "::",
        field("value", $.proc),
      ),

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
          // `using q: Point;` promotes (ADR-0050 §1). Only the typed form takes it: promotion
          // needs the field list, so the compiler refuses `using q := f()` (E0128).
          optional("using"),
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
        $.union_type,
        $.variant_type,
        $.enum_type,
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
        // `#c_call` (ADR-0057 §3) and `#no_abc` (ADR-0058 §3), in either order — a `repeat`, not two
        // `optional`s, because the compiler accepts both orderings and a fixed order would make one
        // spelling an ERROR node while the other parsed.
        repeat(field("attr", $._proc_attr)),
        choice(
          field("body", $.block),
          seq(field("foreign", $.foreign_attr), ";"),
        ),
      ),

    _proc_attr: ($) => choice($.c_call_attr, $.no_abc_attr),

    // #c_call (ADR-0057 §3)
    c_call_attr: (_$) => field("directive", "#c_call"),

    // #no_abc (ADR-0058 §3)
    no_abc_attr: (_$) => field("directive", "#no_abc"),

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
      seq(
        // `using p: Point` promotes the type's fields (ADR-0050 §1). Only the typed form takes it.
        optional("using"),
        $.identifier,
        ":",
        field("type", $._type),
        // `= 10` — a literal default (ADR-0053 §2). Any expression parses; sema refuses a
        // non-literal, with a message saying why.
        optional(seq("=", field("default", $._expr))),
      ),

    // `-> T`, `-> (T, U)` (a results list, ADR-0052 §1), or `-> (T) -> U` (a proc-pointer return,
    // ADR-0059 §3). The last two both begin `(`; the `->` after the `)` decides, and GLR explores
    // both until it appears.
    ret_type: ($) =>
      seq("->", field("type", choice($.result_list, $._type))),

    // `(T, U, …)` after `->` (ADR-0052 §1). A one-element list interns to the element itself, so
    // `-> (T)` and `-> T` are the same type — normalised in `jr-pool`, not refused here.
    result_list: ($) =>
      seq("(", $._type, repeat(seq(",", $._type)), ")"),

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
    _type: ($) =>
      choice(
        $.pointer_type,
        $.array_type,
        $.view_type,
        $.name_type,
        $.struct_type,
        $.union_type,
        $.variant_type,
        $.enum_type,
        $.proc_type,
      ),

    // (T, T) -> T — a procedure-pointer type (ADR-0059 §3).
    //
    // In return position this collides with `result_list`: both begin `(`, and only the `->` after
    // the closing `)` tells them apart. tree-sitter's GLR resolves it on its own (a declared
    // conflict was tried and `generate` reported it unnecessary). The parameter list is its own node
    // so the return type is not mistaken for a last parameter.
    proc_type: ($) =>
      seq(
        field("params", $.proc_type_params),
        // The arrow is **optional** (ADR-0062 §1): `(s64)` is a procedure pointer returning `void`,
        // the way a declaration omits it. This widens the return-position ambiguity with
        // `result_list` — `(s64)` alone is now a valid *type* as well as a one-element results list
        // — which GLR still resolves, because `ret_type` offers both and only the following token
        // distinguishes them.
        optional(seq("->", field("return", $._type))),
      ),

    proc_type_params: ($) =>
      seq("(", optional(seq($._type, repeat(seq(",", $._type)))), ")"),

    // *T
    pointer_type: ($) => seq("*", field("inner", $._type)),

    // [N]T — a fixed-size array (ADR-0039 §3).
    //
    // The length is an *expression*, matching the compiler's parser: `[COUNT]u8` must parse
    // so that sema can be the thing that refuses it, rather than the grammar deciding a
    // semantic question. `[..]T` is deliberately absent here as well, because the compiler
    // refuses it (E0124) and a grammar that accepted it would highlight a type the compiler
    // rejects.
    array_type: ($) =>
      seq("[", field("length", $._expr), "]", field("element", $._type)),

    // []T — a view (ADR-0044 §1).
    //
    // Its own rule rather than an `array_type` with an optional length, which would make the
    // two indistinguishable in a query and would let a malformed `[]` inside an array parse
    // as a view.
    view_type: ($) => seq("[", "]", field("element", $._type)),

    // s64, bool, Point, ... — no field() wrapper, just the identifier
    name_type: ($) => $.identifier,

    // enum { RED; GREEN :: 5; } (ADR-0041).
    //
    // A *type*, like `struct_type`, because ADR-0012 makes `Colour :: enum { … }` an instance
    // of the one `name :: value` constant form. `member_list` and `member` are their own
    // nodes rather than reusing `field_list`/`field`: a field has a type and a member has an
    // optional value, so sharing would make a highlight query unable to tell them apart.
    // `enum` or `enum_flags` (ADR-0043 §1). One node kind rather than two, matching the
    // compiler's parser: the two forms differ only in numbering and permitted operators, and a
    // second node kind would make every consumer handle both.
    enum_type: ($) =>
      seq(
        field("kind", choice("enum", "enum_flags")),
        field("members", $.member_list),
      ),

    // { RED; GREEN :: 5; }
    member_list: ($) => seq("{", repeat($.member), "}"),

    // RED;  or  NOT_FOUND :: 404;
    member: ($) =>
      seq(
        field("name", $.identifier),
        optional(seq("::", field("value", $._expr))),
        ";",
      ),

    // struct { ... }
    struct_type: ($) =>
      seq(
        "struct",
        field("fields", $.field_list),
      ),

    // union { ... } (ADR-0045).
    //
    // Its own rule rather than a `struct_type` with an alternated keyword, mirroring
    // `Item::UnionType` and `UNION_TYPE`: the two differ in *layout*, so a query — or a reader
    // — must be able to tell them apart without inspecting a token. It shares `field_list`,
    // because a union's fields are a struct's fields.
    union_type: ($) =>
      seq(
        "union",
        field("fields", $.field_list),
      ),

    // variant { … } (ADR-0068 §1) — a union's shape plus a tag. The tag is layout, not syntax, so
    // this differs from `union_type` only in the keyword and the node name.
    variant_type: ($) =>
      seq(
        "variant",
        field("fields", $.field_list),
      ),

    field_list: ($) => seq("{", repeat($.field), "}"),

    // struct field: `name : type ;`, or `using name : type ;` to embed (ADR-0050 §1).
    field: ($) =>
      seq(optional("using"), $.identifier, ":", field("type", $._type), ";"),

    // -----------------------------------------------------------------------
    // Statements
    // -----------------------------------------------------------------------
    block: ($) => seq("{", repeat($._stmt), "}"),

    _stmt: ($) =>
      choice(
        $.block,
        $.if_stmt,
        $.while_stmt,
        $.for_stmt,
        $.defer_stmt,
        $.push_context_stmt,
        $.code_stmt,
        $.switch_stmt,
        $.return_stmt,
        $.break_stmt,
        $.continue_stmt,
        $.decl_stmt,
        $.destructuring_stmt,
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
        $.for_stmt,
        $.defer_stmt,
        $.return_stmt,
        $.break_stmt,
        $.continue_stmt,
        $.destructuring_stmt,
        $.assign_stmt,
        $.expr_stmt,
      ),

    // `for x: iter { }`, `for x, i: iter { }`, `for < x: iter { }` (ADR-0049 §1). An optional label
    // (`outer: for …`) precedes it. The iterable is an expression, including a `range_expr`.
    for_stmt: ($) =>
      seq(
        optional(field("label", $.loop_label)),
        "for",
        optional("<"),
        field("value", $.identifier),
        optional(seq(",", field("index", $.identifier))),
        ":",
        field("iter", $._expr),
        field("body", $.block),
      ),

    // `outer:` before a loop (ADR-0049 §2). `name :` collides with an ordinary typed declaration —
    // only the following token tells them apart — so the `[$.loop_label, $.name]` conflict resolves
    // it, and a `prec` was tried and silently broke every `n: s64` declaration.
    loop_label: ($) => seq(field("label", $.identifier), ":"),

    // `defer stmt;` or `defer { }` (ADR-0049 §3).
    defer_stmt: ($) => seq("defer", field("body", $._single_stmt)),

    // switch e { case v; … else; … } (ADR-0067). An arm's body is a run of statements ending at the
    // next `case`, the next `else`, or the closing brace — the same statement-list shape a block has,
    // so no new body kind enters the grammar.
    switch_stmt: ($) =>
      seq(
        "switch",
        field("value", $._expr),
        "{",
        repeat($.switch_arm),
        "}",
      ),

    switch_arm: ($) =>
      seq(
        choice(
          seq("case", field("value", $._expr)),
          "else",
        ),
        ";",
        repeat($._stmt),
      ),

    // push_context { … } (ADR-0063) — a block with its own copy of the context. The body is a
    // braced block only, never a braceless single statement: the parser requires the braces so a
    // context swap has a visible scope.
    push_context_stmt: ($) =>
      seq("push_context", field("body", $.block)),

    // #code { … } (ADR-0080 §1) — unquoted source spliced into the enclosing scope. Its own rule rather
    // than a directive, because it takes a braced body: the directive rules take an optional string or
    // expression operand, which a `{` is neither. The body is an ordinary block, so the grammar needs no
    // special lexing — the splice is a compiler concern, not a syntactic one.
    code_stmt: ($) => seq("#code", field("body", $.block)),

    return_stmt: ($) =>
      seq(
        "return",
        optional(seq(field("value", $._expr), repeat(seq(",", field("value", $._expr))))),
        ";",
      ),

    // `break;`, `break outer;` (ADR-0049 §2), and likewise for continue.
    break_stmt: ($) => seq("break", optional(field("label", $.identifier)), ";"),

    continue_stmt: ($) => seq("continue", optional(field("label", $.identifier)), ";"),

    // `q, ok := f();` and `q, ok = f();` (ADR-0052 §2). A `_` is an ordinary identifier in Jairs, so
    // a discard needs no separate rule — lowering recognises the text.
    destructuring_stmt: ($) =>
      seq(
        field("targets", $.target_list),
        choice(":=", "="),
        field("value", $._expr),
        ";",
      ),

    target_list: ($) =>
      seq($.identifier, repeat1(seq(",", $.identifier))),

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
        // ADR-0042 §6.
        "&=",
        "|=",
        "^=",
        "<<=",
        ">>=",
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
    //    7. postfix .field  .*  (args)  [index]
    // -----------------------------------------------------------------------
    _expr: ($) =>
      choice(
        $.binary_expr,
        $.unary_expr,
        $.deref_expr,
        $.field_expr,
        $.index_expr,
        $.slice_expr,
        $.call_expr,
        $.paren_expr,
        $.literal_expr,
        $.name_expr,
        $.context_expr,
        $.range_expr,
        $.uninit_expr,
        $.cast_expr,
        $.autocast_expr,
        $.member_expr,
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
        // Precedence 4: |   (ADR-0042 §1 — bitwise binds *tighter* than comparison, unlike C)
        prec.left(
          4,
          seq(field("lhs", $._expr), field("op", "|"), field("rhs", $._expr)),
        ),
        // Precedence 5: ^
        prec.left(
          5,
          seq(field("lhs", $._expr), field("op", "^"), field("rhs", $._expr)),
        ),
        // Precedence 6: &
        prec.left(
          6,
          seq(field("lhs", $._expr), field("op", "&"), field("rhs", $._expr)),
        ),
        // Precedence 7: + - +% -%
        prec.left(
          7,
          seq(
            field("lhs", $._expr),
            field("op", choice("+", "-", "+%", "-%")),
            field("rhs", $._expr),
          ),
        ),
        // Precedence 8: << >>   (between + and *, following Go and Rust rather than C)
        prec.left(
          8,
          seq(
            field("lhs", $._expr),
            field("op", choice("<<", ">>")),
            field("rhs", $._expr),
          ),
        ),
        // Precedence 9: * / % *%
        prec.left(
          9,
          seq(
            field("lhs", $._expr),
            field("op", choice("*", "/", "%", "*%")),
            field("rhs", $._expr),
          ),
        ),
      ),

    // Prefix unary: - ! * (address-of) ~ (bitwise complement, ADR-0042 §4)
    unary_expr: ($) =>
      prec.right(
        6,
        seq(
          field("op", choice("-", "!", "*", "~")),
          field("operand", $._expr),
        ),
      ),

    // Postfix dereference: expr.*
    deref_expr: ($) =>
      prec.left(
        7,
        seq(field("operand", $._expr), ".*"),
      ),

    // Indexing: expr[index] (ADR-0039 §5).
    //
    // Precedence 7, the same as `.field`, `.*` and a call, so `a[i].x` and `a.b[i]` chain
    // left-to-right the way the compiler's postfix loop builds them.
    index_expr: ($) =>
      prec.left(
        7,
        seq(field("operand", $._expr), "[", field("index", $._expr), "]"),
      ),

    // Slicing: expr[] — a view over the whole of `expr` (ADR-0044 §2).
    //
    // Same precedence as `index_expr`, so `buf[].count` chains. A separate rule rather than an
    // optional index, for the reason `view_type` is separate from `array_type`.
    slice_expr: ($) =>
      prec.left(7, seq(field("operand", $._expr), "[", "]")),

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
            choice($.named_arg, $._expr),
            repeat(seq(",", choice($.named_arg, $._expr))),
            optional(","),
          ),
        ),
        ")",
      ),

    // `b = 2` at a call site (ADR-0053 §1). Its own node rather than an assignment-shaped
    // expression, because the two mean different things and a consumer must be able to tell them
    // apart.
    named_arg: ($) => seq(field("name", $.identifier), "=", field("value", $._expr)),

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
        $.null,
      ),

    // Name reference
    name_expr: ($) => $.identifier,

    // `0..n` — a range, reachable only as a `for`'s iterable (ADR-0049 §1). The `..` exists
    // nowhere else, which is what keeps it from colliding with `[..]T`. Low precedence so its
    // operands bind first.
    range_expr: ($) => prec.left(-2, seq(field("start", $._expr), "..", field("end", $._expr))),

    // `context` — the implicit context, a keyword rather than a name (ADR-0057 §1). Its own node so
    // a consumer reading names does not find it, or `context.allocator` would look like a field
    // access on a variable somebody declared. Carries a token-level precedence to beat `identifier`.
    context_expr: (_$) => prec(1, "context"),

    // --- (explicitly uninitialised)
    uninit_expr: (_$) => "---",

    // cast(T, x) — a conversion to an explicitly named type (ADR-0037).
    //
    // Listed *before* `call_expr` can match, and keyed on the literal `cast` token, because
    // otherwise `cast(u8, 65)` parses as a perfectly ordinary call whose function happens to
    // be named `cast` — which is what it did before this rule existed. The compiler's parser
    // produced a `CAST_EXPR` and tree-sitter produced a `call_expr`, and the corpus-drift
    // gate stayed green because a wrong *shape* is not an ERROR node. Highlighting was the
    // visible symptom: the `u8` was coloured as a value rather than a type.
    //
    // Precedence 8, above `call_expr`'s 7, so the keyword wins the ambiguity.
    cast_expr: ($) =>
      prec(
        8,
        seq(
          "cast",
          "(",
          field("type", $._type),
          ",",
          field("value", $._expr),
          ")",
        ),
      ),

    // xx expr — autocast, whose target type comes from the context (ADR-0046 §2).
    //
    // Prefix at precedence 10, the same level as the other prefix operators, so `xx n + 1` is
    // `(xx n) + 1` — matching the compiler's parser, which parses the operand with the *unary*
    // parser rather than the full expression parser.
    autocast_expr: ($) => prec(10, seq("xx", field("operand", $._expr))),

    // .RED — an enum member named without its type (ADR-0046 §3).
    //
    // Unambiguous against `field_expr`, which requires an expression *before* the `.`, and
    // against a float literal, whose fractional part needs a digit after the `.`.
    // Precedence **1**, below `field_expr`'s 7, and that ordering is load-bearing: at 10 the
    // parser preferred this rule for the `.x` in `dots[1].x`, splitting one field access into an
    // expression followed by a bare member and reporting a missing `;`. A bare member is only
    // ever reached where no expression precedes the `.`, so it must lose every ambiguity with
    // `field_expr` rather than win one — which mirrors the compiler's parser, where `.` reaches
    // this form only in *prefix* position.
    member_expr: ($) => prec(1, seq(".", field("member", $.identifier))),

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

    // Float literals: 1.5, 1.0e9, 1.5E+10, and **1e9** — an exponent with no fractional
    // part (ADR-0040).
    //
    // The `.`-less form was missing, so `x := 1e9;` produced an ERROR node here while the
    // compiler's lexer accepted it: `crates/jr-syntax/src/lexer.rs` promotes to
    // `FLOAT_LITERAL` on *either* a fractional part or an exponent, independently. Found by
    // parsing a float corpus file rather than by the drift gate, which sees an ERROR only
    // once a file containing one is in the corpus.
    //
    // Two alternatives rather than an optional `.`-group followed by an optional exponent:
    // that spelling would also match a bare `1`, taking `integer_literal`'s place.
    float_literal: (_$) =>
      token(
        choice(
          // A fractional part, optionally followed by an exponent.
          seq(
            /[0-9][0-9_]*/,
            ".",
            /[0-9][0-9_]*/,
            optional(seq(/[eE]/, optional(/[+-]/), /[0-9][0-9_]*/)),
          ),
          // An exponent with no fractional part.
          seq(/[0-9][0-9_]*/, /[eE]/, optional(/[+-]/), /[0-9][0-9_]*/),
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
    // `null` — a context-typed pointer literal (ADR-0060 §1). A keyword the lexer already produces;
    // here it joins the literal choice so `p: *u8 = null` parses as a `literal_expr` rather than an
    // error, and a highlighter colours it as a constant.
    null: (_$) => "null",

    // Line comment: // to end of line
    line_comment: (_$) => token(seq("//", /.*/)),

    // block_comment is handled by the external scanner (nested /* ... */)
  },
});
