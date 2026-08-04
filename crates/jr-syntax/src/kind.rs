//! The syntax vocabulary: every token and every node the Jairs grammar can
//! produce.
//!
//! `rowan` requires a single flat `u16` kind space covering both tokens and
//! nodes, so they live in one enum here. The ordering is deliberate:
//!
//! ```text
//! trivia | literals | identifiers | keywords | punctuation | markers | nodes
//! ```
//!
//! Keeping tokens before nodes means [`SyntaxKind::is_token`] is a single
//! comparison.
//!
//! # Reserved kinds
//!
//! Several tokens are lexed but not yet accepted by the parser -- bitwise
//! operators, `for`, `defer`, `using`, and so on. They are recognised here so
//! that using one produces "bitwise operators arrive in wave W1" rather than
//! "unexpected character", and so that a later wave adding them is not a
//! breaking change for anyone who used the word as an identifier.

/// A token or node kind.
///
/// See the [module documentation](self) for the layout of the kind space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
#[allow(
    non_camel_case_types,
    reason = "SCREAMING_CASE matches rowan convention"
)]
pub enum SyntaxKind {
    // ---- trivia ----------------------------------------------------------
    /// Spaces, tabs, newlines.
    WHITESPACE,
    /// `// ...` to end of line.
    LINE_COMMENT,
    /// `/* ... */`, which nests.
    BLOCK_COMMENT,
    /// `/// ...` — documentation for the declaration that follows (ADR-0027).
    ///
    /// Four or more slashes are a [`LINE_COMMENT`](Self::LINE_COMMENT), following
    /// Rust: a row of slashes is a visual rule, not documentation.
    DOC_COMMENT,
    /// `//! ...` — documentation for the enclosing module (ADR-0027).
    MODULE_DOC_COMMENT,

    // ---- literals --------------------------------------------------------
    /// `42`, `0xdead_beef`, `0b1010`, `0o755`.
    INT_LITERAL,
    /// `1.5`, `1.0e9`. Reserved: floats arrive in wave W1.
    FLOAT_LITERAL,
    /// `"text"`.
    STRING_LITERAL,

    // ---- identifiers and directives --------------------------------------
    /// A bare identifier.
    IDENT,
    /// `#import`, `#run`, `#foreign`, ... lexed as one token including the
    /// `#`. The parser interprets the text, so adding a directive never
    /// requires a lexer change.
    DIRECTIVE,

    // ---- keywords (accepted in Jairs-0) ----------------------------------
    /// `struct`
    STRUCT_KW,
    /// `if`
    IF_KW,
    /// `else`
    ELSE_KW,
    /// `while`
    WHILE_KW,
    /// `return`
    RETURN_KW,
    /// `break`
    BREAK_KW,
    /// `continue`
    CONTINUE_KW,
    /// `true`
    TRUE_KW,
    /// `false`
    FALSE_KW,

    // ---- keywords (reserved for later waves) -----------------------------
    /// `enum` — real syntax as of ADR-0041.
    ///
    /// Still inside the "reserved for later waves" block because
    /// [`SyntaxKind::is_reserved_keyword`]'s range test depends on the contiguous span, and
    /// renumbering the kind space to move one keyword out would churn every `u16` in it for
    /// no behavioural gain. The parser is what decides whether a keyword is refused.
    ENUM_KW,
    /// `union` — reserved, wave W1.
    UNION_KW,
    /// `for` — reserved, wave W2.
    FOR_KW,
    /// `defer` — reserved, wave W2.
    DEFER_KW,
    /// `using` — reserved, wave W2.
    USING_KW,
    /// `cast` — real syntax as of ADR-0037, not a reserved word.
    ///
    /// This comment said "reserved, wave W1" for three waves after `cast` landed. See
    /// [`SyntaxKind::ENUM_KW`] for why it stays in this block regardless.
    CAST_KW,
    /// `xx` (autocast) — reserved, wave W1.
    XX_KW,
    /// `null` — real syntax as of ADR-0060, a context-typed pointer literal.
    ///
    /// It stays in this block, like `cast` and `enum` before it, rather than moving out beside
    /// `FLAGS_KW`: it *was* reserved, so [`SyntaxKind::is_reserved_keyword`]'s range still ends here.
    /// The predicate now means "in the historical reserved block" rather than "refused" — every
    /// keyword in the block is implemented, and `null` was the last. The parser has no `NULL_KW`
    /// refusal arm any more; it parses as a `LITERAL_EXPR`.
    NULL_KW,
    /// `enum_flags` — real syntax as of ADR-0043.
    ///
    /// Placed **after** `NULL_KW` deliberately, which puts it *outside*
    /// [`SyntaxKind::is_reserved_keyword`]'s range: it was never reserved, so adding it into
    /// the reserved block would mean immediately having to remember to exclude it — the trap
    /// `cast` and `enum` both walked into from the other side.
    FLAGS_KW,
    /// `context` — real syntax as of ADR-0057.
    ///
    /// Placed **after** `NULL_KW` like [`SyntaxKind::FLAGS_KW`] and [`SyntaxKind::OPERATOR_KW`],
    /// which puts it *outside* [`SyntaxKind::is_reserved_keyword`]'s range: it was never reserved, so
    /// adding it to that block would mean immediately having to remember to exclude it — the trap
    /// `cast`, `enum`, `union`, `xx`, `for`, `defer` and `using` each walked into from the other side.
    CONTEXT_KW,
    /// `operator` — real syntax as of ADR-0048.
    ///
    /// Placed **after** `NULL_KW`, like `FLAGS_KW`, which puts it *outside*
    /// [`SyntaxKind::is_reserved_keyword`]'s range: it was never reserved, so adding it to the
    /// reserved block would mean immediately having to remember to exclude it — the trap `cast`,
    /// `enum`, `union` and `xx` each walked into from the other side.
    OPERATOR_KW,
    /// `push_context` — real syntax as of ADR-0063.
    ///
    /// Placed **after** `NULL_KW`, like `CONTEXT_KW` and `OPERATOR_KW`, which puts it *outside*
    /// [`SyntaxKind::is_reserved_keyword`]'s range: it was never reserved (no wave ever emitted a
    /// "arrives later" refusal for it), so adding it to that block would mean immediately having to
    /// remember to exclude it — the trap `cast`, `enum`, `union` and `xx` each walked into.
    PUSH_CONTEXT_KW,
    /// `switch` — real syntax as of ADR-0067.
    ///
    /// Placed **after** `NULL_KW`, like `CONTEXT_KW` and `PUSH_CONTEXT_KW`, which puts it *outside*
    /// [`SyntaxKind::is_reserved_keyword`]'s range: it was never reserved — no wave ever emitted an
    /// "arrives later" refusal for it — so adding it to that block would mean immediately having to
    /// remember to exclude it.
    SWITCH_KW,
    /// `case` — real syntax as of ADR-0067, and only meaningful inside a `switch`.
    ///
    /// Lexed as its own keyword rather than matched as an identifier, so that `case` opening an arm is a
    /// *token* the grammar can key on. Matching text would make a variable called `case` silently start
    /// an arm — the trap `context` walked into from the other side (ADR-0057).
    CASE_KW,
    /// `variant` — real syntax as of ADR-0068, the tagged aggregate form.
    ///
    /// Its own keyword rather than an attribute on `union`, because the two differ in *semantics* —
    /// a variant carries a tag, costs a check per read and is bigger — and ADR-0045 §1 instructed
    /// exactly this, "the way `enum_flags` is different from `enum`".
    VARIANT_KW,

    // ---- delimiters ------------------------------------------------------
    /// `(`
    L_PAREN,
    /// `)`
    R_PAREN,
    /// `{`
    L_BRACE,
    /// `}`
    R_BRACE,
    /// `[`
    L_BRACK,
    /// `]`
    R_BRACK,

    // ---- structural punctuation ------------------------------------------
    /// `,`
    COMMA,
    /// `;`
    SEMICOLON,
    /// `:`
    COLON,
    /// `::`
    COLON_COLON,
    /// `:=`
    COLON_EQ,
    /// `->`
    ARROW,
    /// `.`
    DOT,
    /// `.*` — postfix dereference.
    DOT_STAR,
    /// `..` — reserved, wave W1 (`[..]T`).
    DOT_DOT,
    /// `---` — explicitly uninitialised.
    UNINIT,

    // ---- arithmetic ------------------------------------------------------
    /// `+`
    PLUS,
    /// `-`
    MINUS,
    /// `*` — multiplication, pointer type, and address-of.
    STAR,
    /// `/`
    SLASH,
    /// `%`
    PERCENT,
    /// `+%` — wrapping add.
    PLUS_PERCENT,
    /// `-%` — wrapping subtract.
    MINUS_PERCENT,
    /// `*%` — wrapping multiply.
    STAR_PERCENT,

    // ---- assignment ------------------------------------------------------
    /// `=`
    EQ,
    /// `+=`
    PLUS_EQ,
    /// `-=`
    MINUS_EQ,
    /// `*=`
    STAR_EQ,
    /// `/=`
    SLASH_EQ,
    /// `%=`
    PERCENT_EQ,
    /// `+%=`
    PLUS_PERCENT_EQ,
    /// `-%=`
    MINUS_PERCENT_EQ,
    /// `*%=`
    STAR_PERCENT_EQ,
    /// `&=` (ADR-0042 §6)
    AMP_EQ,
    /// `|=`
    PIPE_EQ,
    /// `^=`
    CARET_EQ,
    /// `<<=`
    SHL_EQ,
    /// `>>=`
    SHR_EQ,

    // ---- comparison ------------------------------------------------------
    /// `==`
    EQ_EQ,
    /// `!=`
    BANG_EQ,
    /// `<`
    LT,
    /// `<=`
    LT_EQ,
    /// `>`
    GT,
    /// `>=`
    GT_EQ,

    // ---- logical ---------------------------------------------------------
    /// `&&`
    AMP_AMP,
    /// `||`
    PIPE_PIPE,
    /// `!`
    BANG,

    // ---- bitwise (reserved, wave W1) -------------------------------------
    /// `&` — reserved.
    AMP,
    /// `|` — reserved.
    PIPE,
    /// `^` — reserved.
    CARET,
    /// `~` — reserved.
    TILDE,
    /// `<<` — reserved.
    SHL,
    /// `>>` — reserved.
    SHR,
    /// `@` — reserved, wave W6 (declaration notes).
    AT,

    // ---- markers ---------------------------------------------------------
    /// A character the lexer does not recognise.
    UNKNOWN,
    /// Virtual end-of-input token. Never present in the tree.
    EOF,

    // ======================================================================
    // Nodes. Everything at or after `SOURCE_FILE` is an interior node.
    // ======================================================================
    /// The root of every parse.
    SOURCE_FILE,

    // ---- declarations ----------------------------------------------------
    /// `name :: value` — a compile-time constant. Procedures and structs are
    /// constants whose value is a `PROC` or `STRUCT_TYPE`, exactly as in Jai.
    CONST_DECL,
    /// `operator + :: (a: T, b: T) -> T { … }` (ADR-0048 §1).
    ///
    /// Its own kind rather than a `CONST_DECL` whose `NAME` holds an operator token: every
    /// consumer of `CONST_DECL` expects an `IDENT` there, and a shared kind would make each of
    /// them ask. The *value* is an ordinary `PROC`, because an overload is an ordinary procedure
    /// whose name happens to be an operator.
    OPERATOR_DECL,
    /// `name := value`, `name: T`, or `name: T = value`.
    VAR_DECL,
    /// `#import "Basic";`
    IMPORT_DECL,
    /// A top-level `#run expr;` executed for its side effects.
    RUN_DECL,
    /// The name being bound by a declaration.
    NAME,

    // ---- procedures ------------------------------------------------------
    /// A procedure: signature plus optional body.
    PROC,
    /// `(a: s64, b: s64)`
    PARAM_LIST,
    /// `a: s64`
    PARAM,
    /// `-> s64`
    RET_TYPE,
    /// `#foreign libc "write"`
    FOREIGN_ATTR,

    // ---- types -----------------------------------------------------------
    /// A type named by an identifier, e.g. `s64` or `Point`.
    NAME_TYPE,
    /// `*T`
    POINTER_TYPE,
    /// `[N]T` — a fixed-size array (ADR-0039 §3).
    ///
    /// The length is a child expression rather than a token, so that `[COUNT]u8`
    /// parses the same way `[20]u8` does. Sema is where a non-constant length is
    /// refused, because "is this expression a compile-time constant" is a semantic
    /// question the parser cannot answer.
    ARRAY_TYPE,
    /// `[]T` — a view (ADR-0044 §1).
    ///
    /// A separate kind from `ARRAY_TYPE` rather than one with an absent length child, because
    /// `TypeRef::Array`'s `len: None` already means "the length was not a usable literal"
    /// (ADR-0039 §3a) — so a shared node would make a view indistinguishable from that error.
    VIEW_TYPE,
    /// `(T, T) -> T` — a procedure-pointer type (ADR-0059 §3).
    ///
    /// Holds a `PROC_TYPE_PARAMS` node then the return type. Distinct from a `RESULT_LIST`, which is
    /// also `(…)`: the `->` is what tells them apart, and the parser commits only after it has seen
    /// whether an arrow follows the closing `)`. A results list has no return type, so sharing the
    /// node would make a consumer ask which of the two it really is.
    PROC_TYPE,
    /// The parameter-type list of a `PROC_TYPE`, holding zero or more type nodes.
    ///
    /// Its own node so the return type — the last type child of a `PROC_TYPE` — is not mistaken for
    /// a parameter, which a flat list of type children would allow.
    PROC_TYPE_PARAMS,
    /// `struct { ... }`
    STRUCT_TYPE,
    /// `union { ... }` (ADR-0045).
    ///
    /// Its own kind rather than a `STRUCT_TYPE` with a flag, mirroring `Item::UnionType`: the
    /// two differ in *layout*, and every consumer that computes an offset must branch on it.
    /// It shares `FIELD_LIST`/`FIELD`, because a union's fields *are* a struct's fields.
    UNION_TYPE,
    /// `variant { … }` (ADR-0068 §1) — a tagged aggregate.
    ///
    /// Structurally identical to `UNION_TYPE`; the difference is entirely in what the tag makes the
    /// compiler emit, so the node kinds differ only so that lowering can tell them apart.
    VARIANT_TYPE,
    /// `enum { RED; GREEN; }` (ADR-0041).
    ///
    /// A *type*, like `STRUCT_TYPE`, because ADR-0012 makes `Colour :: enum { … }` an
    /// instance of the one `name :: value` constant form rather than a declaration of its
    /// own.
    ENUM_TYPE,
    /// The member list of an `enum`.
    MEMBER_LIST,
    /// A `#c_call` attribute on a procedure, opting out of the implicit context (ADR-0057 §3).
    ///
    /// Its own node rather than a bare token so `jr-fmt` finds it by kind and lowering reads it the
    /// way it reads `FOREIGN_ATTR` — every directive lexes as one `DIRECTIVE` token, so the node is
    /// what distinguishes them.
    C_CALL_ATTR,
    /// A `#no_abc` attribute on a procedure, suppressing its bounds checks (ADR-0058 §3).
    ///
    /// Its own node beside `C_CALL_ATTR` rather than one shared `ATTR` kind carrying the directive
    /// text. Two kinds means a consumer that handles one and forgets the other is a *missing arm*
    /// rather than a string comparison that silently falls through — and `jr-fmt` has lost a
    /// construct in seven of the last eight waves by exactly that route.
    ///
    /// **On the procedure, not on the index.** ADR-0003 said the opt-out would be per-index;
    /// ADR-0058 §3 amends that, because a per-index flag has to reach `Projection::Index` through
    /// every one of the eleven passes and back ends that match on a projection, and a flag some of
    /// them ignore is this project's first named failure mode.
    NO_ABC_ATTR,
    /// The `context` keyword used as an expression (ADR-0057 §1).
    ///
    /// Its own kind rather than a `NAME_EXPR`, because `context` is a keyword and not a name — a
    /// consumer reading names must not find it, or `context.allocator` would look like a field access
    /// on a variable somebody declared.
    CONTEXT_EXPR,
    /// A `#scope_module` or `#scope_export` visibility marker (ADR-0054 §1).
    ///
    /// A node rather than a bare token so that `jr-fmt` can find it by kind and lowering can read
    /// *which* directive it is from the token inside — the same shape `IMPORT_DECL` uses, and for
    /// the same reason: every directive lexes as one `DIRECTIVE` token.
    SCOPE_DECL,
    /// A named argument at a call site: the `b = 2` of `f(1, b = 2)` (ADR-0053 §1).
    ///
    /// Its own node rather than an `ASSIGN_STMT`-shaped expression, because the two mean different
    /// things and sharing a kind would make `f(a = 1)` indistinguishable from an assignment used as
    /// an argument — which Jairs does not have.
    NAMED_ARG,
    /// A parenthesised result list after `->`: `(s64, bool)` (ADR-0052 §1).
    ///
    /// Its own node rather than a `PAREN_EXPR` in type position, because the two are different
    /// things that happen to share brackets — and a consumer finding a `RESULT_LIST` by kind
    /// knows it has several types without inspecting what is inside.
    RESULT_LIST,
    /// A destructuring target list: the `q, ok` of `q, ok := f();` (ADR-0052 §2).
    ///
    /// Holds a `NAME` per target, and an `UNDERSCORE` token for each discarded position — which is
    /// why a discard needs no `NAME`: it is a *hole*, recognised positionally, and never becomes a
    /// name anything can resolve (ADR-0052 §3).
    TARGET_LIST,
    /// One enum member: `RED;` or `NOT_FOUND :: 404;`.
    ///
    /// Its own kind rather than reusing `FIELD`: a field has a *type* and a member has an
    /// optional *value*, so sharing the node would mean every consumer asking which one it
    /// really is.
    MEMBER,
    /// `{ x: s64; }`
    FIELD_LIST,
    /// `x: s64;`
    FIELD,

    // ---- statements ------------------------------------------------------
    /// `{ ... }`
    BLOCK,
    /// A declaration used as a statement.
    DECL_STMT,
    /// An expression evaluated for its effect, e.g. `zero();`
    EXPR_STMT,
    /// `lhs = rhs;` and its compound forms.
    ASSIGN_STMT,
    /// `if cond { ... } else { ... }`
    IF_STMT,
    /// The `else` arm of an `if`.
    ELSE_BRANCH,
    /// `while cond { ... }`
    WHILE_STMT,
    /// `for x: buf { … }`, `for x, i: buf { … }`, `for i: 0..n { … }` (ADR-0049 §1).
    ///
    /// The loop variable is **named**, not implicit: Jai defaults to `it`/`it_index` and Jairs
    /// requires the name, because a name introduced without being written is the invisible
    /// behaviour ADR-0014 §3 refuses.
    FOR_STMT,
    /// The `a..b` range in a `for` header (ADR-0049 §1).
    ///
    /// Its own node, and reachable **only** here — there is no `..` operator in the expression
    /// grammar and no `Range` in the pool, which is what keeps it from colliding with `[..]T`.
    RANGE_EXPR,
    /// `defer stmt;` (ADR-0049 §3).
    DEFER_STMT,
    /// `push_context { … }` (ADR-0063) — a block with its own copy of the context.
    ///
    /// Wraps a `BLOCK`. Its own node rather than a flag on `BLOCK`, so every exhaustive match over
    /// statements is forced to decide what a context scope means rather than treating it as an
    /// ordinary block that happens to swap a pointer.
    PUSH_CONTEXT_STMT,
    /// `#code { … }` (ADR-0080 §1) — unquoted source that splices into the enclosing scope.
    ///
    /// Holds a [`SyntaxKind::BLOCK`], parsed as ordinary statements so its faults are reported where they
    /// are written. Its own node rather than a `DIRECTIVE_EXPR` because it takes a **braced body**, which
    /// a directive-expression's optional string-or-operand shape cannot express — and because it is a
    /// statement, not an expression: there is no `Code` value (ADR-0080 §3).
    CODE_STMT,
    /// `switch e { case v; … else; … }` (ADR-0067) — a value match with exhaustiveness checking.
    ///
    /// Holds the scrutinee expression and one [`SyntaxKind::SWITCH_ARM`] per arm.
    SWITCH_STMT,
    /// One arm of a `switch`: `case v;` or `else;`, then the statements it runs (ADR-0067 §1).
    ///
    /// Its own node rather than a flat run of statements under `SWITCH_STMT`, because an arm has an
    /// identity — a value, a body, and a position the exhaustiveness check reports against. A flat list
    /// would make "which arm does this statement belong to" a counting exercise.
    ///
    /// The `else` arm is a `SWITCH_ARM` with no value expression, which is what distinguishes it: an
    /// absent value *is* the catch-all, so nothing needs a second node kind.
    SWITCH_ARM,
    /// `label:` before a `for` or `while` (ADR-0049 §2).
    ///
    /// A label names a *loop* and is deliberately not an expression name: it is resolved against
    /// `build.rs`'s loop stack, the only place a loop's identity exists.
    LOOP_LABEL,
    /// `return expr;`
    RETURN_STMT,
    /// `break;`
    BREAK_STMT,
    /// `continue;`
    CONTINUE_STMT,

    // ---- expressions -----------------------------------------------------
    /// An integer, string, or boolean literal.
    LITERAL_EXPR,
    /// A reference to a name.
    NAME_EXPR,
    /// `a + b`
    BINARY_EXPR,
    /// `-a`, `!a`, and prefix `*a` (address-of).
    UNARY_EXPR,
    /// `(a)`
    PAREN_EXPR,
    /// `f(a, b)`
    CALL_EXPR,
    /// `(a, b)` in a call.
    ARG_LIST,
    /// `a.b`
    FIELD_EXPR,
    /// `a[i]` (ADR-0039 §5).
    ///
    /// Postfix, at the same precedence as `.b` and `.*`, so `a[i].x` and `a.b[i]`
    /// chain the way a reader expects.
    INDEX_EXPR,
    /// `a[]` — the slice operator, producing a view over the whole of `a` (ADR-0044 §2).
    ///
    /// Postfix at the same precedence as `INDEX_EXPR`, and a *separate kind* rather than an
    /// `INDEX_EXPR` with no subscript child: the two differ in what they produce and in
    /// whether they take an address, and a consumer distinguishing them by counting children
    /// would treat a malformed `a[` as a slice.
    ///
    /// Spelled `[]` and not `[..]` because `[..]T` is already reserved for dynamic arrays, so
    /// `a[..]` and `[..]T` would be the same two tokens meaning different things in different
    /// positions.
    SLICE_EXPR,
    /// `p.*`
    DEREF_EXPR,
    /// `---`
    UNINIT_EXPR,
    /// `xx expr` — autocast, whose target type comes from the context (ADR-0046 §2).
    ///
    /// A *prefix* form rather than a call-like `xx(expr)`: it takes no type argument, so there
    /// is nothing to parenthesise. Its own kind rather than a `CAST_EXPR` with no type child,
    /// because the two differ in exactly the question ADR-0046 is about — where the target type
    /// comes from — and a shared kind would make every consumer ask.
    AUTOCAST_EXPR,
    /// `.RED` — an enum member named without its type (ADR-0046 §3).
    ///
    /// Unambiguous against a float without a lexer change: a `.` begins a fractional part only
    /// when a digit follows, so `.5` is a literal and `.RED` is this.
    MEMBER_EXPR,
    /// `cast(T, x)`
    ///
    /// Its own node rather than a `CALL_EXPR` to a name, because its first argument is a
    /// *type* and Jairs cannot pass one in a call until W4's RTTI (ADR-0037 §3). A node kind
    /// rather than a token, so `TokenSet`'s `u128` bitmask is unaffected.
    CAST_EXPR,
    /// `#run expr`
    RUN_EXPR,
    /// A directive used as an expression, e.g. `#system_library "c"`.
    DIRECTIVE_EXPR,

    /// A node covering text the parser could not make sense of. Its presence
    /// is what makes the tree total: parsing never fails, it only produces
    /// `ERROR` nodes alongside diagnostics.
    ERROR,
}

impl SyntaxKind {
    /// The first node kind. Everything below this is a token.
    const FIRST_NODE: Self = Self::SOURCE_FILE;

    /// Returns `true` if this kind is a token rather than an interior node.
    #[must_use]
    pub const fn is_token(self) -> bool {
        (self as u16) < (Self::FIRST_NODE as u16)
    }

    /// Returns `true` if this kind is an interior node.
    #[must_use]
    pub const fn is_node(self) -> bool {
        !self.is_token()
    }

    /// Returns `true` for whitespace and comments.
    ///
    /// Trivia is attached to the tree rather than discarded, which is what
    /// makes the formatter and the language server possible.
    #[must_use]
    pub const fn is_trivia(self) -> bool {
        matches!(
            self,
            Self::WHITESPACE
                | Self::LINE_COMMENT
                | Self::BLOCK_COMMENT
                | Self::DOC_COMMENT
                | Self::MODULE_DOC_COMMENT
        )
    }

    /// Returns `true` for any comment, documentation or not.
    ///
    /// This exists so that a consumer which treats all comments alike — the
    /// formatter has six such sites — cannot silently drop a doc comment by
    /// matching only the two kinds that predate ADR-0027. Every one of those
    /// sites ended in a `_ => {}` arm, which is a legitimate branch for the
    /// tokens it was written for and so would not have failed to compile.
    #[must_use]
    pub const fn is_comment(self) -> bool {
        matches!(
            self,
            Self::LINE_COMMENT | Self::BLOCK_COMMENT | Self::DOC_COMMENT | Self::MODULE_DOC_COMMENT
        )
    }

    /// Returns `true` for a comment that runs to end of line.
    ///
    /// The distinction that matters to the formatter: anything emitted after one of
    /// these must start on a new line, or it is swallowed by the comment. A
    /// [`BLOCK_COMMENT`](Self::BLOCK_COMMENT) has a terminator and so does not
    /// force a break.
    #[must_use]
    pub const fn is_line_comment(self) -> bool {
        matches!(
            self,
            Self::LINE_COMMENT | Self::DOC_COMMENT | Self::MODULE_DOC_COMMENT
        )
    }

    /// Returns `true` if this kind is any keyword, reserved or not.
    #[must_use]
    pub const fn is_keyword(self) -> bool {
        (self as u16) >= (Self::STRUCT_KW as u16) && (self as u16) <= (Self::NULL_KW as u16)
    }

    /// Returns `true` for keywords the parser does not yet accept.
    ///
    /// These produce a "not yet implemented" diagnostic naming the wave that
    /// will add them, rather than a confusing syntax error.
    #[must_use]
    pub const fn is_reserved_keyword(self) -> bool {
        (self as u16) >= (Self::ENUM_KW as u16) && (self as u16) <= (Self::NULL_KW as u16)
    }

    /// Maps identifier text to its keyword kind, if it is one.
    #[must_use]
    pub fn from_keyword(text: &str) -> Option<Self> {
        Some(match text {
            "struct" => Self::STRUCT_KW,
            "if" => Self::IF_KW,
            "else" => Self::ELSE_KW,
            "while" => Self::WHILE_KW,
            "return" => Self::RETURN_KW,
            "break" => Self::BREAK_KW,
            "continue" => Self::CONTINUE_KW,
            "true" => Self::TRUE_KW,
            "false" => Self::FALSE_KW,
            "enum" => Self::ENUM_KW,
            "enum_flags" => Self::FLAGS_KW,
            "operator" => Self::OPERATOR_KW,
            "context" => Self::CONTEXT_KW,
            "push_context" => Self::PUSH_CONTEXT_KW,
            "switch" => Self::SWITCH_KW,
            "case" => Self::CASE_KW,
            "variant" => Self::VARIANT_KW,
            "union" => Self::UNION_KW,
            "for" => Self::FOR_KW,
            "defer" => Self::DEFER_KW,
            "using" => Self::USING_KW,
            "cast" => Self::CAST_KW,
            "xx" => Self::XX_KW,
            "null" => Self::NULL_KW,
            _ => return None,
        })
    }

    /// The source text of this kind, when it is fixed.
    ///
    /// Returns `None` for kinds whose text varies (identifiers, literals,
    /// trivia) and for nodes. Used by diagnostics to say ``expected `;` ``.
    #[must_use]
    pub const fn static_text(self) -> Option<&'static str> {
        Some(match self {
            Self::STRUCT_KW => "struct",
            Self::IF_KW => "if",
            Self::ELSE_KW => "else",
            Self::WHILE_KW => "while",
            Self::RETURN_KW => "return",
            Self::BREAK_KW => "break",
            Self::CONTINUE_KW => "continue",
            Self::TRUE_KW => "true",
            Self::FALSE_KW => "false",
            Self::ENUM_KW => "enum",
            Self::FLAGS_KW => "enum_flags",
            Self::OPERATOR_KW => "operator",
            Self::CONTEXT_KW => "context",
            Self::PUSH_CONTEXT_KW => "push_context",
            Self::SWITCH_KW => "switch",
            Self::CASE_KW => "case",
            Self::VARIANT_KW => "variant",
            Self::UNION_KW => "union",
            Self::FOR_KW => "for",
            Self::DEFER_KW => "defer",
            Self::USING_KW => "using",
            Self::CAST_KW => "cast",
            Self::XX_KW => "xx",
            Self::NULL_KW => "null",
            Self::L_PAREN => "(",
            Self::R_PAREN => ")",
            Self::L_BRACE => "{",
            Self::R_BRACE => "}",
            Self::L_BRACK => "[",
            Self::R_BRACK => "]",
            Self::COMMA => ",",
            Self::SEMICOLON => ";",
            Self::COLON => ":",
            Self::COLON_COLON => "::",
            Self::COLON_EQ => ":=",
            Self::ARROW => "->",
            Self::DOT => ".",
            Self::DOT_STAR => ".*",
            Self::DOT_DOT => "..",
            Self::UNINIT => "---",
            Self::PLUS => "+",
            Self::MINUS => "-",
            Self::STAR => "*",
            Self::SLASH => "/",
            Self::PERCENT => "%",
            Self::PLUS_PERCENT => "+%",
            Self::MINUS_PERCENT => "-%",
            Self::STAR_PERCENT => "*%",
            Self::EQ => "=",
            Self::PLUS_EQ => "+=",
            Self::MINUS_EQ => "-=",
            Self::STAR_EQ => "*=",
            Self::SLASH_EQ => "/=",
            Self::PERCENT_EQ => "%=",
            Self::PLUS_PERCENT_EQ => "+%=",
            Self::MINUS_PERCENT_EQ => "-%=",
            Self::STAR_PERCENT_EQ => "*%=",
            Self::AMP_EQ => "&=",
            Self::PIPE_EQ => "|=",
            Self::CARET_EQ => "^=",
            Self::SHL_EQ => "<<=",
            Self::SHR_EQ => ">>=",
            Self::EQ_EQ => "==",
            Self::BANG_EQ => "!=",
            Self::LT => "<",
            Self::LT_EQ => "<=",
            Self::GT => ">",
            Self::GT_EQ => ">=",
            Self::AMP_AMP => "&&",
            Self::PIPE_PIPE => "||",
            Self::BANG => "!",
            Self::AMP => "&",
            Self::PIPE => "|",
            Self::CARET => "^",
            Self::TILDE => "~",
            Self::SHL => "<<",
            Self::SHR => ">>",
            Self::AT => "@",
            _ => return None,
        })
    }

    /// A human-readable description for diagnostics.
    #[must_use]
    pub fn describe(self) -> String {
        if let Some(text) = self.static_text() {
            return format!("`{text}`");
        }
        match self {
            Self::IDENT => "an identifier".to_owned(),
            Self::INT_LITERAL => "an integer literal".to_owned(),
            Self::FLOAT_LITERAL => "a float literal".to_owned(),
            Self::STRING_LITERAL => "a string literal".to_owned(),
            Self::DIRECTIVE => "a directive".to_owned(),
            Self::EOF => "end of file".to_owned(),
            other => format!("{other:?}"),
        }
    }
}

impl From<SyntaxKind> for rowan::SyntaxKind {
    fn from(kind: SyntaxKind) -> Self {
        Self(kind as u16)
    }
}

/// The `rowan` language marker for Jairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum JairsLanguage {}

impl rowan::Language for JairsLanguage {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> Self::Kind {
        assert!(
            raw.0 <= SyntaxKind::ERROR as u16,
            "raw syntax kind {} is out of range",
            raw.0
        );
        // SAFETY: `SyntaxKind` is `#[repr(u16)]` with contiguous discriminants
        // from 0 to `ERROR`, and the assertion above bounds `raw.0` to that
        // range.
        unsafe { std::mem::transmute::<u16, SyntaxKind>(raw.0) }
    }

    fn kind_to_raw(kind: Self::Kind) -> rowan::SyntaxKind {
        kind.into()
    }
}

/// A node in the Jairs concrete syntax tree.
pub type SyntaxNode = rowan::SyntaxNode<JairsLanguage>;
/// A token in the Jairs concrete syntax tree.
pub type SyntaxToken = rowan::SyntaxToken<JairsLanguage>;
/// Either a node or a token.
pub type SyntaxElement = rowan::SyntaxElement<JairsLanguage>;

#[cfg(test)]
mod tests {
    use super::*;
    use rowan::Language;

    #[test]
    fn token_and_node_halves_do_not_overlap() {
        assert!(SyntaxKind::WHITESPACE.is_token());
        assert!(SyntaxKind::EOF.is_token());
        assert!(SyntaxKind::SOURCE_FILE.is_node());
        assert!(SyntaxKind::ERROR.is_node());
        assert!(!SyntaxKind::SOURCE_FILE.is_token());
    }

    #[test]
    fn trivia_is_exactly_whitespace_and_comments() {
        assert!(SyntaxKind::WHITESPACE.is_trivia());
        assert!(SyntaxKind::LINE_COMMENT.is_trivia());
        assert!(SyntaxKind::BLOCK_COMMENT.is_trivia());
        // A doc comment is trivia, which is what keeps the parser out of this
        // change entirely (ADR-0027 §1).
        assert!(SyntaxKind::DOC_COMMENT.is_trivia());
        assert!(SyntaxKind::MODULE_DOC_COMMENT.is_trivia());
        assert!(!SyntaxKind::IDENT.is_trivia());
        assert!(!SyntaxKind::SEMICOLON.is_trivia());
    }

    #[test]
    fn every_comment_kind_is_a_comment() {
        assert!(SyntaxKind::LINE_COMMENT.is_comment());
        assert!(SyntaxKind::BLOCK_COMMENT.is_comment());
        assert!(SyntaxKind::DOC_COMMENT.is_comment());
        assert!(SyntaxKind::MODULE_DOC_COMMENT.is_comment());
        assert!(!SyntaxKind::WHITESPACE.is_comment());
        assert!(!SyntaxKind::IDENT.is_comment());
    }

    #[test]
    fn keyword_classification() {
        assert!(SyntaxKind::STRUCT_KW.is_keyword());
        assert!(!SyntaxKind::STRUCT_KW.is_reserved_keyword());
        assert!(SyntaxKind::FOR_KW.is_keyword());
        assert!(SyntaxKind::FOR_KW.is_reserved_keyword());
        assert!(!SyntaxKind::IDENT.is_keyword());
    }

    #[test]
    fn every_keyword_round_trips_through_its_text() {
        // Guards against a keyword added to `from_keyword` but forgotten in
        // `static_text`, which would make diagnostics print `FOO_KW`.
        for text in [
            "struct", "if", "else", "while", "return", "break", "continue", "true", "false",
            "enum", "union", "for", "defer", "using", "cast", "xx", "null",
        ] {
            let kind = SyntaxKind::from_keyword(text)
                .unwrap_or_else(|| panic!("`{text}` is not recognised as a keyword"));
            assert_eq!(
                kind.static_text(),
                Some(text),
                "`{text}` is missing from static_text"
            );
        }
    }

    #[test]
    fn non_keywords_are_not_keywords() {
        assert_eq!(SyntaxKind::from_keyword("structure"), None);
        assert_eq!(SyntaxKind::from_keyword("Struct"), None);
        assert_eq!(SyntaxKind::from_keyword("main"), None);
        assert_eq!(SyntaxKind::from_keyword(""), None);
    }

    #[test]
    fn raw_kind_round_trips() {
        for kind in [
            SyntaxKind::WHITESPACE,
            SyntaxKind::COLON_COLON,
            SyntaxKind::SOURCE_FILE,
            SyntaxKind::ERROR,
        ] {
            let raw = JairsLanguage::kind_to_raw(kind);
            assert_eq!(JairsLanguage::kind_from_raw(raw), kind);
        }
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn out_of_range_raw_kind_panics_rather_than_transmuting_garbage() {
        let _ = JairsLanguage::kind_from_raw(rowan::SyntaxKind(u16::MAX));
    }

    #[test]
    fn describe_is_useful_for_diagnostics() {
        assert_eq!(SyntaxKind::SEMICOLON.describe(), "`;`");
        assert_eq!(SyntaxKind::IDENT.describe(), "an identifier");
        assert_eq!(SyntaxKind::EOF.describe(), "end of file");
    }
}
