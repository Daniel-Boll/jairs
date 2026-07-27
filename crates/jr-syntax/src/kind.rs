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
    /// `enum` — reserved, wave W1.
    ENUM_KW,
    /// `union` — reserved, wave W1.
    UNION_KW,
    /// `for` — reserved, wave W2.
    FOR_KW,
    /// `defer` — reserved, wave W2.
    DEFER_KW,
    /// `using` — reserved, wave W2.
    USING_KW,
    /// `cast` — reserved, wave W1.
    CAST_KW,
    /// `xx` (autocast) — reserved, wave W1.
    XX_KW,
    /// `null` — reserved, wave W1.
    NULL_KW,

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
    /// `struct { ... }`
    STRUCT_TYPE,
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
    /// `p.*`
    DEREF_EXPR,
    /// `---`
    UNINIT_EXPR,
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
