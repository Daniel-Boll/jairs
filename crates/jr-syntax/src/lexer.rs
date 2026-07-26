//! The hand-written lexer.
//!
//! Design notes, all of which are pinned by files in `tests/corpus`:
//!
//! * **Trivia is preserved.** Whitespace and comments become real tokens so
//!   the CST is lossless and the formatter can round-trip.
//! * **Lexing never fails.** Every byte of input ends up inside exactly one
//!   token, so offsets always reconstruct the source. Problems are reported as
//!   diagnostics alongside a best-guess token.
//! * **Block comments nest**, unlike C. Commenting out a region containing a
//!   comment does the obvious thing. An unterminated comment is reported at the
//!   *outermost* `/*`, because that is the one the user needs to find.
//! * **An unterminated string stops at the end of the line.** Running to
//!   end-of-file would swallow the rest of the program and turn one typo into a
//!   cascade of nonsense errors.

use crate::kind::SyntaxKind::{self, *};
use jr_base::{FileId, Span, TextRange, TextSize};
use jr_diag::{Diagnostic, Diagnostics};

/// A lexed token: its kind and its byte range in the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    /// What kind of token this is.
    pub kind: SyntaxKind,
    /// The byte range this token covers.
    pub range: TextRange,
}

/// The result of lexing a file.
#[derive(Debug)]
pub struct LexOutput {
    /// Every token, in source order, including trivia. Concatenating the text
    /// of these ranges reproduces the input exactly.
    pub tokens: Vec<Token>,
    /// Problems found while lexing.
    pub diagnostics: Diagnostics,
}

/// Lexes `text`, attributing spans to `file`.
///
/// Never fails: see the [module documentation](self).
#[must_use]
pub fn lex(text: &str, file: FileId) -> LexOutput {
    let mut lexer = Lexer {
        text,
        pos: 0,
        file,
        tokens: Vec::new(),
        diagnostics: Diagnostics::new(),
    };
    lexer.run();
    LexOutput {
        tokens: lexer.tokens,
        diagnostics: lexer.diagnostics,
    }
}

/// Operators, longest first. Order matters: `-%=` must be tried before `-%`,
/// which must be tried before `-`, or `a -%= b` mis-lexes.
///
/// The three-character entries are the ones that make this non-obvious:
/// `---` (uninitialised) must beat `-` twice, and the wrapping compound
/// assignments must beat their two-character prefixes.
const OPERATORS: &[(&str, SyntaxKind)] = &[
    ("+%=", PLUS_PERCENT_EQ),
    ("-%=", MINUS_PERCENT_EQ),
    ("*%=", STAR_PERCENT_EQ),
    ("---", UNINIT),
    ("+%", PLUS_PERCENT),
    ("-%", MINUS_PERCENT),
    ("*%", STAR_PERCENT),
    ("::", COLON_COLON),
    (":=", COLON_EQ),
    ("->", ARROW),
    (".*", DOT_STAR),
    ("..", DOT_DOT),
    ("+=", PLUS_EQ),
    ("-=", MINUS_EQ),
    ("*=", STAR_EQ),
    ("/=", SLASH_EQ),
    ("%=", PERCENT_EQ),
    ("==", EQ_EQ),
    ("!=", BANG_EQ),
    ("<=", LT_EQ),
    (">=", GT_EQ),
    ("&&", AMP_AMP),
    ("||", PIPE_PIPE),
    ("<<", SHL),
    (">>", SHR),
    ("(", L_PAREN),
    (")", R_PAREN),
    ("{", L_BRACE),
    ("}", R_BRACE),
    ("[", L_BRACK),
    ("]", R_BRACK),
    (",", COMMA),
    (";", SEMICOLON),
    (":", COLON),
    (".", DOT),
    ("+", PLUS),
    ("-", MINUS),
    ("*", STAR),
    ("/", SLASH),
    ("%", PERCENT),
    ("=", EQ),
    ("<", LT),
    (">", GT),
    ("!", BANG),
    ("&", AMP),
    ("|", PIPE),
    ("^", CARET),
    ("~", TILDE),
    ("@", AT),
];

struct Lexer<'a> {
    text: &'a str,
    pos: usize,
    file: FileId,
    tokens: Vec<Token>,
    diagnostics: Diagnostics,
}

impl<'a> Lexer<'a> {
    fn run(&mut self) {
        while self.pos < self.text.len() {
            let start = self.pos;
            let kind = self.next_token();
            debug_assert!(
                self.pos > start,
                "lexer failed to advance at offset {start}; this would loop forever"
            );
            self.push(kind, start);
        }
    }

    fn next_token(&mut self) -> SyntaxKind {
        let Some(c) = self.peek() else {
            // `run` guarantees we are not at end of input.
            unreachable!("next_token called at end of input");
        };

        match c {
            c if c.is_whitespace() => self.whitespace(),
            '/' if self.rest().starts_with("//") => self.line_comment(),
            '/' if self.rest().starts_with("/*") => self.block_comment(),
            '"' => self.string(),
            '#' => self.directive(),
            c if is_ident_start(c) => self.ident_or_keyword(),
            c if c.is_ascii_digit() => self.number(),
            _ => self.operator_or_unknown(),
        }
    }

    // ---- trivia ----------------------------------------------------------

    fn whitespace(&mut self) -> SyntaxKind {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
        WHITESPACE
    }

    fn line_comment(&mut self) -> SyntaxKind {
        // Consumes to end of line but NOT the newline itself, which stays
        // whitespace. This keeps the formatter's line handling uniform.
        while self.peek().is_some_and(|c| c != '\n') {
            self.bump();
        }
        LINE_COMMENT
    }

    fn block_comment(&mut self) -> SyntaxKind {
        let outermost = self.pos;
        self.pos += 2; // `/*`
        let mut depth = 1usize;

        while depth > 0 {
            if self.rest().starts_with("/*") {
                self.pos += 2;
                depth += 1;
            } else if self.rest().starts_with("*/") {
                self.pos += 2;
                depth -= 1;
            } else if self.bump().is_none() {
                // Point at the outermost `/*`: that is where the user's
                // commented-out region began, and it is what they need to fix.
                // Reporting the innermost would send them to the wrong place.
                self.error(
                    outermost,
                    outermost + 2,
                    "unterminated block comment",
                    "E0002",
                )
                .with_note(format!(
                    "{depth} unclosed `/*` {} still open at end of file",
                    if depth == 1 { "is" } else { "are" }
                ))
                .with_help("block comments nest in Jairs, so each `/*` needs its own `*/`")
                .emit(&mut self.diagnostics);
                break;
            }
        }
        BLOCK_COMMENT
    }

    // ---- literals --------------------------------------------------------

    fn string(&mut self) -> SyntaxKind {
        let start = self.pos;
        self.bump(); // opening quote

        loop {
            match self.peek() {
                None => {
                    self.unterminated_string(start);
                    break;
                }
                // Stop at a newline rather than running to end of file, so a
                // missing quote costs one error instead of derailing the parse.
                Some('\n') => {
                    self.unterminated_string(start);
                    break;
                }
                Some('"') => {
                    self.bump();
                    break;
                }
                Some('\\') => self.escape(),
                Some(_) => {
                    self.bump();
                }
            }
        }
        STRING_LITERAL
    }

    fn unterminated_string(&mut self, start: usize) {
        self.error(start, self.pos, "unterminated string literal", "E0001")
            .with_help("string literals may not span multiple lines")
            .emit(&mut self.diagnostics);
    }

    fn escape(&mut self) {
        let start = self.pos;
        self.bump(); // backslash

        let Some(c) = self.peek() else {
            // Handled by the caller as an unterminated string.
            return;
        };

        match c {
            'n' | 'r' | 't' | '0' | '\\' | '"' => {
                self.bump();
            }
            'u' => {
                self.bump();
                let digits_start = self.pos;
                let mut digits = 0;
                while digits < 4 && self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                    self.bump();
                    digits += 1;
                }
                if digits != 4 {
                    self.error(
                        start,
                        self.pos.max(digits_start),
                        "invalid unicode escape",
                        "E0003",
                    )
                    .with_help("a unicode escape is `\\u` followed by exactly four hex digits")
                    .emit(&mut self.diagnostics);
                }
            }
            // A newline directly after a backslash: do not consume it, or the
            // string would swallow the next line.
            '\n' => {}
            other => {
                self.bump();
                self.error(
                    start,
                    self.pos,
                    format!("unknown escape `\\{other}`"),
                    "E0003",
                )
                .with_help("valid escapes are `\\n` `\\r` `\\t` `\\0` `\\\\` `\\\"` and `\\uXXXX`")
                .emit(&mut self.diagnostics);
            }
        }
    }

    fn number(&mut self) -> SyntaxKind {
        let start = self.pos;
        let mut kind = INT_LITERAL;

        let radix_name = if self.rest().starts_with("0x") || self.rest().starts_with("0X") {
            self.pos += 2;
            Some(("hexadecimal", 16u32))
        } else if self.rest().starts_with("0b") || self.rest().starts_with("0B") {
            self.pos += 2;
            Some(("binary", 2))
        } else if self.rest().starts_with("0o") || self.rest().starts_with("0O") {
            self.pos += 2;
            Some(("octal", 8))
        } else {
            None
        };

        if let Some((name, radix)) = radix_name {
            let digits = self.digits(radix);
            if digits == 0 {
                self.error(
                    start,
                    self.pos,
                    format!("{name} literal has no digits"),
                    "E0004",
                )
                .emit(&mut self.diagnostics);
            }
        } else {
            self.digits(10);

            // A `.` only begins a fractional part when a digit follows, so
            // `1..2` lexes as `1` `..` `2` and `x.*` is unaffected.
            if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
                kind = FLOAT_LITERAL;
                self.bump();
                self.digits(10);
            }

            if matches!(self.peek(), Some('e' | 'E'))
                && (self.peek_at(1).is_some_and(|c| c.is_ascii_digit())
                    || (matches!(self.peek_at(1), Some('+' | '-'))
                        && self.peek_at(2).is_some_and(|c| c.is_ascii_digit())))
            {
                kind = FLOAT_LITERAL;
                self.bump(); // e
                if matches!(self.peek(), Some('+' | '-')) {
                    self.bump();
                }
                self.digits(10);
            }
        }

        // `123abc` is a typo, not a number followed by a name. Consuming the
        // suffix into the token gives one clear error instead of two confusing
        // ones.
        if self.peek().is_some_and(is_ident_continue) {
            let suffix_start = self.pos;
            while self.peek().is_some_and(is_ident_continue) {
                self.bump();
            }
            let suffix = &self.text[suffix_start..self.pos];

            // A lone trailing `e` is a half-written exponent, not a suffix.
            // Saying "invalid suffix" here would send the user looking for a
            // feature Jairs does not have instead of at their typo.
            if radix_name.is_none() && matches!(suffix, "e" | "E") {
                self.error(start, self.pos, "exponent has no digits", "E0004")
                    .with_help("an exponent needs at least one digit, as in `1e9` or `1.5e-3`")
                    .emit(&mut self.diagnostics);
            } else {
                self.error(
                    suffix_start,
                    self.pos,
                    "invalid suffix on numeric literal",
                    "E0004",
                )
                .with_help(
                    "Jairs has no literal suffixes; write the type on the declaration instead",
                )
                .emit(&mut self.diagnostics);
            }
        }

        kind
    }

    /// Consumes digits of `radix` plus `_` separators. Returns the digit count,
    /// not counting separators.
    fn digits(&mut self, radix: u32) -> usize {
        let mut count = 0;
        while let Some(c) = self.peek() {
            if c == '_' {
                self.bump();
            } else if c.is_digit(radix) {
                self.bump();
                count += 1;
            } else {
                break;
            }
        }
        count
    }

    // ---- names and directives --------------------------------------------

    fn ident_or_keyword(&mut self) -> SyntaxKind {
        let start = self.pos;
        while self.peek().is_some_and(is_ident_continue) {
            self.bump();
        }
        SyntaxKind::from_keyword(&self.text[start..self.pos]).unwrap_or(IDENT)
    }

    fn directive(&mut self) -> SyntaxKind {
        let start = self.pos;
        self.bump(); // `#`

        if !self.peek().is_some_and(is_ident_start) {
            self.error(
                start,
                self.pos,
                "expected a directive name after `#`",
                "E0006",
            )
            .with_help("directives look like `#import`, `#run`, or `#foreign`")
            .emit(&mut self.diagnostics);
            return UNKNOWN;
        }

        while self.peek().is_some_and(is_ident_continue) {
            self.bump();
        }
        DIRECTIVE
    }

    // ---- operators -------------------------------------------------------

    fn operator_or_unknown(&mut self) -> SyntaxKind {
        let rest = self.rest();
        for &(text, kind) in OPERATORS {
            if rest.starts_with(text) {
                self.pos += text.len();
                return kind;
            }
        }

        let start = self.pos;
        let c = self.bump().unwrap_or_default();
        self.error(
            start,
            self.pos,
            format!("unexpected character `{c}`"),
            "E0005",
        )
        .emit(&mut self.diagnostics);
        UNKNOWN
    }

    // ---- plumbing --------------------------------------------------------

    fn rest(&self) -> &'a str {
        &self.text[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.rest().chars().nth(n)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn push(&mut self, kind: SyntaxKind, start: usize) {
        self.tokens.push(Token {
            kind,
            range: TextRange::new(
                TextSize::new(u32::try_from(start).expect("source file exceeds 4 GiB")),
                TextSize::new(u32::try_from(self.pos).expect("source file exceeds 4 GiB")),
            ),
        });
    }

    fn error(
        &self,
        start: usize,
        end: usize,
        message: impl Into<String>,
        code: &'static str,
    ) -> Diagnostic {
        let span = Span::from_offsets(
            self.file,
            u32::try_from(start).unwrap_or(u32::MAX),
            u32::try_from(end).unwrap_or(u32::MAX),
        );
        Diagnostic::error(span, message).with_code(code)
    }
}

/// Extension used only to keep the diagnostic-building chains above readable.
trait Emit {
    fn emit(self, sink: &mut Diagnostics);
}

impl Emit for Diagnostic {
    fn emit(self, sink: &mut Diagnostics) {
        sink.push(self);
    }
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic()
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file() -> FileId {
        FileId::from_usize(0)
    }

    /// Lexes and renders as `KIND "text"` pairs, skipping whitespace so the
    /// expectations stay readable.
    fn dump(text: &str) -> String {
        let out = lex(text, file());
        out.tokens
            .iter()
            .filter(|t| t.kind != WHITESPACE)
            .map(|t| format!("{:?} {:?}", t.kind, &text[t.range]))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn kinds(text: &str) -> Vec<SyntaxKind> {
        lex(text, file())
            .tokens
            .into_iter()
            .map(|t| t.kind)
            .filter(|k| *k != WHITESPACE)
            .collect()
    }

    fn errors(text: &str) -> Vec<String> {
        lex(text, file())
            .diagnostics
            .into_vec()
            .into_iter()
            .map(|d| d.message)
            .collect()
    }

    /// The invariant that makes the CST lossless: token ranges tile the input
    /// exactly, with no gaps and no overlaps.
    fn assert_tiles_input(text: &str) {
        let out = lex(text, file());
        let mut cursor = TextSize::new(0);
        for token in &out.tokens {
            assert_eq!(
                token.range.start(),
                cursor,
                "gap or overlap before {:?} in {text:?}",
                token.kind
            );
            cursor = token.range.end();
        }
        assert_eq!(
            usize::from(cursor),
            text.len(),
            "tokens do not cover all of {text:?}"
        );
        let rejoined: String = out.tokens.iter().map(|t| &text[t.range]).collect();
        assert_eq!(rejoined, text, "detokenising must reproduce the source");
    }

    #[test]
    fn empty_input_yields_no_tokens() {
        let out = lex("", file());
        assert!(out.tokens.is_empty());
        assert!(out.diagnostics.is_empty());
    }

    #[test]
    fn tokens_always_tile_the_input() {
        for text in [
            "",
            " ",
            "\n\n",
            "a := 1;",
            "// comment",
            "/* a /* b */ c */",
            "\"unterminated",
            "/* unterminated",
            "0x",
            "123abc",
            "héllo",
            "€",
            "a\r\nb",
            "#import \"Basic\";",
            "---",
            "p.*",
        ] {
            assert_tiles_input(text);
        }
    }

    #[test]
    fn declarations_lex() {
        assert_eq!(
            kinds("MAX :: 10;"),
            [IDENT, COLON_COLON, INT_LITERAL, SEMICOLON]
        );
        assert_eq!(kinds("a := 1;"), [IDENT, COLON_EQ, INT_LITERAL, SEMICOLON]);
        assert_eq!(
            kinds("a: s64 = 1;"),
            [IDENT, COLON, IDENT, EQ, INT_LITERAL, SEMICOLON]
        );
        assert_eq!(kinds("a: s64;"), [IDENT, COLON, IDENT, SEMICOLON]);
    }

    #[test]
    fn keywords_are_distinguished_from_identifiers() {
        assert_eq!(kinds("struct"), [STRUCT_KW]);
        assert_eq!(kinds("structure"), [IDENT]);
        assert_eq!(kinds("if_"), [IDENT]);
        assert_eq!(kinds("_if"), [IDENT]);
        assert_eq!(kinds("If"), [IDENT]);
    }

    #[test]
    fn reserved_keywords_lex_as_keywords_not_identifiers() {
        // So that `for x` reports "not yet implemented" rather than silently
        // treating `for` as a variable name that a later wave would break.
        for (text, kind) in [
            ("for", FOR_KW),
            ("defer", DEFER_KW),
            ("using", USING_KW),
            ("enum", ENUM_KW),
            ("union", UNION_KW),
            ("cast", CAST_KW),
            ("xx", XX_KW),
            ("null", NULL_KW),
        ] {
            assert_eq!(kinds(text), [kind], "{text}");
            assert!(kind.is_reserved_keyword(), "{text}");
        }
    }

    // ---- the operator longest-match table --------------------------------

    #[test]
    fn wrapping_operators_beat_their_prefixes() {
        assert_eq!(kinds("a +% b"), [IDENT, PLUS_PERCENT, IDENT]);
        assert_eq!(kinds("a -% b"), [IDENT, MINUS_PERCENT, IDENT]);
        assert_eq!(kinds("a *% b"), [IDENT, STAR_PERCENT, IDENT]);
        assert_eq!(kinds("a +%= b"), [IDENT, PLUS_PERCENT_EQ, IDENT]);
        assert_eq!(kinds("a -%= b"), [IDENT, MINUS_PERCENT_EQ, IDENT]);
        assert_eq!(kinds("a *%= b"), [IDENT, STAR_PERCENT_EQ, IDENT]);
    }

    #[test]
    fn uninit_beats_repeated_minus() {
        assert_eq!(kinds("x := ---;"), [IDENT, COLON_EQ, UNINIT, SEMICOLON]);
        // Two minuses are not an operator, so they stay separate tokens.
        assert_eq!(kinds("--"), [MINUS, MINUS]);
        assert_eq!(kinds("----"), [UNINIT, MINUS]);
    }

    #[test]
    fn arrow_beats_minus() {
        assert_eq!(kinds("-> -"), [ARROW, MINUS]);
        assert_eq!(kinds("-= -"), [MINUS_EQ, MINUS]);
    }

    #[test]
    fn compound_assignment_beats_bare_operator() {
        assert_eq!(
            kinds("+= -= *= /= %="),
            [PLUS_EQ, MINUS_EQ, STAR_EQ, SLASH_EQ, PERCENT_EQ]
        );
    }

    #[test]
    fn comparison_and_logical_operators() {
        assert_eq!(
            kinds("== != <= >= < > && || !"),
            [
                EQ_EQ, BANG_EQ, LT_EQ, GT_EQ, LT, GT, AMP_AMP, PIPE_PIPE, BANG
            ]
        );
    }

    #[test]
    fn colon_forms_are_distinguished() {
        assert_eq!(kinds(":: := :"), [COLON_COLON, COLON_EQ, COLON]);
    }

    #[test]
    fn pointer_syntax() {
        // Prefix `*` for address-of and pointer types, postfix `.*` for
        // dereference. Same STAR token; the parser disambiguates by position.
        assert_eq!(
            kinds("p: *s64 = *x;"),
            [IDENT, COLON, STAR, IDENT, EQ, STAR, IDENT, SEMICOLON]
        );
        assert_eq!(kinds("p.*"), [IDENT, DOT_STAR]);
        assert_eq!(kinds("p.*.*"), [IDENT, DOT_STAR, DOT_STAR]);
        assert_eq!(kinds("**s64"), [STAR, STAR, IDENT]);
    }

    #[test]
    fn dot_forms_are_distinguished() {
        assert_eq!(kinds("a.b"), [IDENT, DOT, IDENT]);
        assert_eq!(kinds("a.*"), [IDENT, DOT_STAR]);
        assert_eq!(kinds("1..2"), [INT_LITERAL, DOT_DOT, INT_LITERAL]);
    }

    #[test]
    fn reserved_bitwise_operators_lex() {
        assert_eq!(
            kinds("& | ^ ~ << >> @"),
            [AMP, PIPE, CARET, TILDE, SHL, SHR, AT]
        );
    }

    // ---- comments --------------------------------------------------------

    #[test]
    fn line_comment_excludes_its_newline() {
        let text = "// hi\nx";
        let out = lex(text, file());
        assert_eq!(out.tokens[0].kind, LINE_COMMENT);
        assert_eq!(&text[out.tokens[0].range], "// hi");
        assert_eq!(out.tokens[1].kind, WHITESPACE);
        assert!(out.diagnostics.is_empty());
    }

    #[test]
    fn line_comment_at_eof_without_newline() {
        assert_eq!(kinds("// trailing"), [LINE_COMMENT]);
        assert!(errors("// trailing").is_empty());
    }

    #[test]
    fn block_comments_nest() {
        let text = "/* outer /* inner */ still outer */ after";
        let out = lex(text, file());
        assert_eq!(out.tokens[0].kind, BLOCK_COMMENT);
        assert_eq!(
            &text[out.tokens[0].range], "/* outer /* inner */ still outer */",
            "a nested `*/` must not end the outer comment"
        );
        assert!(out.diagnostics.is_empty());
        assert_eq!(out.tokens.last().unwrap().kind, IDENT);
    }

    #[test]
    fn unterminated_block_comment_points_at_outermost_open() {
        let text = "x\n/* outer /* inner */";
        let out = lex(text, file());
        let diags = out.diagnostics.into_vec();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].message, "unterminated block comment");
        // The span must be the OUTER `/*` at offset 2, not the inner one.
        assert_eq!(u32::from(diags[0].primary.span.start()), 2);
        assert_eq!(u32::from(diags[0].primary.span.end()), 4);
    }

    #[test]
    fn unterminated_nested_comment_reports_depth() {
        let diags = lex("/* a /* b /* c", file()).diagnostics.into_vec();
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].notes.iter().any(|(_, n)| n.contains('3')),
            "should report 3 unclosed comments, got {:?}",
            diags[0].notes
        );
    }

    // ---- strings ---------------------------------------------------------

    #[test]
    fn plain_and_empty_strings() {
        assert_eq!(dump(r#""text""#), r#"STRING_LITERAL "\"text\"""#);
        assert_eq!(kinds(r#""""#), [STRING_LITERAL]);
        assert!(errors(r#""text""#).is_empty());
    }

    #[test]
    fn valid_escapes_are_accepted() {
        let text = r#""tab:\t nl:\n cr:\r nul:\0 slash:\\ quote:\" u:\u00e9""#;
        assert_eq!(kinds(text), [STRING_LITERAL]);
        assert!(errors(text).is_empty(), "{:?}", errors(text));
    }

    #[test]
    fn escaped_quote_does_not_end_the_string() {
        let text = r#""a\"b" x"#;
        let out = lex(text, file());
        assert_eq!(&text[out.tokens[0].range], r#""a\"b""#);
        assert!(out.diagnostics.is_empty());
    }

    #[test]
    fn unknown_escape_is_reported_but_lexing_continues() {
        let errs = errors(r#""bad \q here""#);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0], "unknown escape `\\q`");
        assert_eq!(kinds(r#""bad \q here""#), [STRING_LITERAL]);
    }

    #[test]
    fn short_unicode_escape_is_reported() {
        let errs = errors(r#""\u12""#);
        assert_eq!(errs, ["invalid unicode escape"]);
    }

    #[test]
    fn unterminated_string_stops_at_end_of_line() {
        // Corpus invalid/005: the following statement must still be lexed, so
        // one missing quote costs one error rather than eating the file.
        let text = "a := \"oops\nb := 1;";
        let out = lex(text, file());
        let diags = out.diagnostics.into_vec();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].message, "unterminated string literal");

        let after: Vec<_> = out
            .tokens
            .iter()
            .skip_while(|t| t.kind != STRING_LITERAL)
            .skip(1)
            .map(|t| t.kind)
            .filter(|k| *k != WHITESPACE)
            .collect();
        assert_eq!(
            after,
            [IDENT, COLON_EQ, INT_LITERAL, SEMICOLON],
            "the statement after an unterminated string must still lex"
        );
    }

    #[test]
    fn unterminated_string_at_eof() {
        assert_eq!(errors("\"oops"), ["unterminated string literal"]);
    }

    #[test]
    fn backslash_newline_does_not_swallow_the_next_line() {
        let text = "\"a\\\nb := 1;";
        let out = lex(text, file());
        assert_eq!(
            out.diagnostics.len(),
            1,
            "expected exactly one unterminated-string error"
        );
        assert!(
            out.tokens.iter().any(|t| t.kind == COLON_EQ),
            "the next line must still lex"
        );
    }

    // ---- numbers ---------------------------------------------------------

    #[test]
    fn integer_literal_forms() {
        for text in [
            "0",
            "1234567890",
            "0xdead_beef",
            "0XDEADBEEF",
            "0b1010_1010",
            "0o755",
            "1_000_000",
            "9223372036854775807",
        ] {
            assert_eq!(kinds(text), [INT_LITERAL], "{text}");
            assert!(errors(text).is_empty(), "{text}: {:?}", errors(text));
        }
    }

    #[test]
    fn radix_prefix_without_digits_is_reported() {
        assert_eq!(errors("0x"), ["hexadecimal literal has no digits"]);
        assert_eq!(errors("0b"), ["binary literal has no digits"]);
        assert_eq!(errors("0o"), ["octal literal has no digits"]);
        // The token is still produced, so the parser sees a literal.
        assert_eq!(kinds("0x"), [INT_LITERAL]);
    }

    #[test]
    fn underscore_separators_alone_are_not_digits() {
        assert_eq!(errors("0x_"), ["hexadecimal literal has no digits"]);
    }

    #[test]
    fn float_literals_lex_even_though_the_parser_rejects_them() {
        for text in ["1.5", "0.0", "1.0e9", "1.5E+10", "2.0e-3"] {
            assert_eq!(kinds(text), [FLOAT_LITERAL], "{text}");
        }
    }

    #[test]
    fn dot_not_followed_by_digit_is_not_a_float() {
        // `1..2` is a range (reserved) and `1 . x` is field access; neither is
        // a float, so the fractional part requires a following digit.
        assert_eq!(kinds("1..2"), [INT_LITERAL, DOT_DOT, INT_LITERAL]);
        assert_eq!(kinds("1.x"), [INT_LITERAL, DOT, IDENT]);
    }

    #[test]
    fn exponent_requires_digits() {
        // A half-written exponent is treated as one malformed number rather
        // than `1` followed by a variable named `e`. That is the more likely
        // intent, and it keeps the error count at one.
        assert_eq!(kinds("1e"), [INT_LITERAL]);
        assert_eq!(errors("1e"), ["exponent has no digits"]);
        assert_eq!(errors("1E"), ["exponent has no digits"]);

        // But a real identifier suffix is still reported as a suffix.
        assert_eq!(errors("1exyz"), ["invalid suffix on numeric literal"]);

        // A well-formed exponent is a float.
        assert_eq!(kinds("1e9"), [FLOAT_LITERAL]);
        assert!(errors("1e9").is_empty());
    }

    #[test]
    fn invalid_numeric_suffix_is_one_error_not_two_tokens() {
        assert_eq!(errors("123abc"), ["invalid suffix on numeric literal"]);
        assert_eq!(kinds("123abc"), [INT_LITERAL]);
    }

    // ---- directives ------------------------------------------------------

    #[test]
    fn directives_lex_as_one_token() {
        let text = "#import \"Basic\";";
        // Note the `r##` delimiter: the expected text contains `"#`, which
        // would close an `r#"` string early.
        assert_eq!(
            dump(text).lines().next().unwrap(),
            r##"DIRECTIVE "#import""##
        );
        assert_eq!(kinds(text), [DIRECTIVE, STRING_LITERAL, SEMICOLON]);
    }

    #[test]
    fn all_slice_directives_lex_uniformly() {
        // Adding a directive must never require a lexer change.
        for text in [
            "#run",
            "#foreign",
            "#system_library",
            "#import",
            "#c_call",
            "#no_abc",
            "#some_future_directive",
        ] {
            assert_eq!(kinds(text), [DIRECTIVE], "{text}");
        }
    }

    #[test]
    fn hash_without_a_name_is_reported() {
        assert_eq!(errors("# "), ["expected a directive name after `#`"]);
        assert_eq!(kinds("# "), [UNKNOWN]);
    }

    // ---- unknown input ---------------------------------------------------

    #[test]
    fn unexpected_characters_are_reported_individually() {
        // Corpus invalid/007: stray tokens must not stop the lexer.
        let errs = errors("$ `");
        assert_eq!(errs.len(), 2);
        assert_eq!(errs[0], "unexpected character `$`");
        assert_eq!(kinds("$ `"), [UNKNOWN, UNKNOWN]);
    }

    #[test]
    fn non_ascii_identifiers_are_rejected_one_char_at_a_time() {
        // Multi-byte characters must advance by their full UTF-8 width, or the
        // lexer would slice mid-character and panic.
        let out = lex("héllo", file());
        assert_eq!(out.tokens[0].kind, IDENT); // `h`
        assert!(out.tokens.iter().any(|t| t.kind == UNKNOWN)); // `é`
        assert_eq!(out.diagnostics.len(), 1);
    }

    #[test]
    fn unicode_inside_string_literals_is_fine() {
        let text = "\"café 中文 🦀\"";
        assert_eq!(kinds(text), [STRING_LITERAL]);
        assert!(errors(text).is_empty());
    }

    #[test]
    fn crlf_is_whitespace() {
        assert_eq!(kinds("a\r\nb"), [IDENT, IDENT]);
        assert!(errors("a\r\nb").is_empty());
    }

    // ---- a whole program -------------------------------------------------

    #[test]
    fn hello_world_lexes_without_errors() {
        let text = r#"
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
    sum := add(p.x, COMPUTED);
    if sum > 5 {
        print(MESSAGE);
    }
    ptr := *sum;
    print_int(ptr.*);
}
"#;
        let out = lex(text, file());
        assert!(
            out.diagnostics.is_empty(),
            "hello world must lex cleanly: {:?}",
            out.diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
        assert_tiles_input(text);
    }
}
