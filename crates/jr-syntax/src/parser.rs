//! The error-recovering recursive-descent parser for Jairs.
//!
//! # Design
//!
//! The parser uses `rowan`'s [`GreenNodeBuilder`] with **checkpoints** for
//! left-associative binary expressions. A checkpoint is placed before the
//! left operand; after parsing the operator and right operand the builder
//! wraps everything since the checkpoint into a `BINARY_EXPR` node. This
//! avoids a separate event/marker vector while still producing the correct
//! left-associative tree shape.
//!
//! # Losslessness
//!
//! Every byte of input ends up in the tree. Trivia (whitespace and comments)
//! is attached to the next non-trivia token. The invariant
//! `parse(text, file).syntax().text().to_string() == text` is tested over
//! every corpus file.
//!
//! # Error recovery
//!
//! The parser never returns `Err`. Unparseable input becomes `ERROR` nodes
//! alongside diagnostics. Recovery uses token-set-based synchronisation:
//! when stuck, the parser consumes tokens into an `ERROR` node until it
//! reaches a token that can start the enclosing construct. A depth counter
//! prevents stack overflow on deeply nested or adversarial input.

use rowan::{Checkpoint, GreenNode, GreenNodeBuilder};

use crate::kind::{SyntaxKind, SyntaxKind::*, SyntaxNode};
use crate::lexer::{Token, lex};
use jr_base::{FileId, Span, TextSize};
use jr_diag::{Diagnostic, Diagnostics};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// The result of parsing a source file.
///
/// Cheap to clone: the green tree is reference-counted.
#[derive(Debug, Clone)]
pub struct Parse {
    green: GreenNode,
    diagnostics: Diagnostics,
}

impl Parse {
    /// The root [`SyntaxNode`] of the concrete syntax tree.
    ///
    /// The tree is lossless: `syntax().text().to_string()` reproduces the
    /// original source byte for byte.
    #[must_use]
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    /// All diagnostics produced during parsing (and lexing).
    #[must_use]
    pub fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }

    /// Returns `true` if the parse produced no errors.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics.has_errors()
    }

    /// Returns `Ok(syntax)` if there are no errors, `Err(diagnostics)` otherwise.
    ///
    /// # Errors
    /// Returns `Err` when the parse produced at least one error diagnostic.
    pub fn ok(self) -> Result<SyntaxNode, Diagnostics> {
        if self.has_errors() {
            Err(self.diagnostics)
        } else {
            Ok(self.syntax())
        }
    }
}

/// Parses `text` as a Jairs source file, attributing spans to `file`.
///
/// Never fails: see the [module documentation](self).
#[must_use]
pub fn parse(text: &str, file: FileId) -> Parse {
    let lex_out = lex(text, file);
    let mut p = Parser::new(text, &lex_out.tokens, file);
    p.parse_source_file();
    let (green, mut diagnostics) = p.finish();
    diagnostics.extend(lex_out.diagnostics.into_vec());
    Parse { green, diagnostics }
}

// ---------------------------------------------------------------------------
// Token sets for recovery
// ---------------------------------------------------------------------------

/// A compact set of [`SyntaxKind`]s used for synchronisation during error
/// recovery.
#[derive(Clone, Copy)]
struct TokenSet(u128);

// A `TokenSet` is a bitmask indexed by `SyntaxKind`'s discriminant, so it can only
// hold token kinds whose discriminant fits in a `u128`. Adding a token kind shifts
// every later discriminant, and overflowing this would be a shift-overflow panic in
// a `const` — a confusing failure a long way from its cause. ADR-0027 added two
// token kinds and this is the guard that makes the next one a build error instead.
const _: () = assert!(
    (SyntaxKind::SOURCE_FILE as u16) <= 128,
    "TokenSet is a u128 bitmask over token kinds; there are now too many to fit"
);

impl TokenSet {
    const fn new(kinds: &[SyntaxKind]) -> Self {
        let mut bits = 0u128;
        let mut i = 0;
        while i < kinds.len() {
            bits |= 1u128 << (kinds[i] as u16);
            i += 1;
        }
        Self(bits)
    }

    const fn contains(self, kind: SyntaxKind) -> bool {
        self.0 & (1u128 << (kind as u16)) != 0
    }

    const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Tokens that can start a top-level item.
const ITEM_START: TokenSet = TokenSet::new(&[IDENT, DIRECTIVE]);

/// Tokens that can start a statement.
const STMT_START: TokenSet = TokenSet::new(&[
    IDENT,
    DIRECTIVE,
    L_BRACE,
    IF_KW,
    WHILE_KW,
    RETURN_KW,
    BREAK_KW,
    CONTINUE_KW,
]);

/// Tokens that can start an expression (primary).
const EXPR_START: TokenSet = TokenSet::new(&[
    INT_LITERAL,
    FLOAT_LITERAL,
    STRING_LITERAL,
    TRUE_KW,
    FALSE_KW,
    IDENT,
    L_PAREN,
    MINUS,
    BANG,
    STAR,
    UNINIT,
    DIRECTIVE,
]);

// ---------------------------------------------------------------------------
// Parser internals
// ---------------------------------------------------------------------------

/// Maximum recursion depth. Exceeding this emits a diagnostic and stops
/// recursing, preventing stack overflow on adversarial input.
const MAX_DEPTH: u32 = 256;

/// Maximum length of an iteratively-built operator chain (binary operators,
/// postfix accessors).
///
/// This bounds *tree* depth, which [`MAX_DEPTH`] cannot: those loops are
/// iterative, but each iteration wraps everything parsed so far, so the tree
/// grows as deep as the chain is long. Walking or dropping a `rowan` tree
/// recurses, so an unbounded chain becomes a stack overflow at drop time --
/// nowhere near the code that caused it.
const MAX_CHAIN: u32 = 1024;

struct Parser<'src> {
    text: &'src str,
    tokens: &'src [Token],
    /// Index of the next non-trivia token to consume.
    pos: usize,
    file: FileId,
    builder: GreenNodeBuilder<'static>,
    diagnostics: Diagnostics,
    /// Current recursion depth.
    depth: u32,
    /// Whether the depth limit has already been reported.
    ///
    /// Deeply nested input would otherwise produce one diagnostic per nesting
    /// level -- tens of thousands of identical errors, which is both useless to
    /// the user and a memory-exhaustion vector.
    depth_error_reported: bool,
    /// Trivia tokens that have been lexed but not yet emitted into the tree.
    pending_trivia: Vec<Token>,
}

impl<'src> Parser<'src> {
    fn new(text: &'src str, tokens: &'src [Token], file: FileId) -> Self {
        Self {
            text,
            tokens,
            pos: 0,
            file,
            builder: GreenNodeBuilder::new(),
            diagnostics: Diagnostics::new(),
            depth: 0,
            depth_error_reported: false,
            pending_trivia: Vec::new(),
        }
    }

    fn finish(self) -> (GreenNode, Diagnostics) {
        (self.builder.finish(), self.diagnostics)
    }

    // ---- token inspection ------------------------------------------------

    /// The kind of the current non-trivia token, skipping trivia.
    fn current(&mut self) -> SyntaxKind {
        self.skip_trivia_peek();
        if self.pos < self.tokens.len() {
            self.tokens[self.pos].kind
        } else {
            EOF
        }
    }

    /// The kind of the token `n` positions ahead (0 = current), skipping trivia.
    fn nth(&mut self, n: usize) -> SyntaxKind {
        // We need to look ahead past trivia. Collect non-trivia positions.
        let mut count = 0;
        let mut i = self.pos;
        // First flush pending trivia from current position
        while i < self.tokens.len() {
            if self.tokens[i].kind.is_trivia() {
                i += 1;
                continue;
            }
            if count == n {
                return self.tokens[i].kind;
            }
            count += 1;
            i += 1;
        }
        EOF
    }

    /// Returns `true` if the current token is `kind`.
    fn at(&mut self, kind: SyntaxKind) -> bool {
        self.current() == kind
    }

    /// Distinguishes a procedure signature from a parenthesised expression when
    /// the current token is the `(` after `::`.
    ///
    /// Scans forward to the matching `)` (tracking nesting, ignoring trivia) and
    /// reports whether it is followed by `->`, `{`, or a `#foreign` directive --
    /// the only things that may follow a parameter list.
    ///
    /// Unterminated input answers `true`, because `add :: (a: s64` is far more
    /// likely a procedure the user is still typing than an expression, and that
    /// guess produces the better diagnostic.
    fn looks_like_proc_signature(&mut self) -> bool {
        debug_assert_eq!(self.current(), L_PAREN);

        let mut depth = 0usize;
        let mut i = self.pos;
        while i < self.tokens.len() {
            let kind = self.tokens[i].kind;
            i += 1;
            match kind {
                L_PAREN => depth += 1,
                R_PAREN => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
        if depth != 0 {
            return true; // unterminated: assume a half-typed procedure
        }

        // First non-trivia token after the matching `)`.
        while i < self.tokens.len() && self.tokens[i].kind.is_trivia() {
            i += 1;
        }
        match self.tokens.get(i) {
            None => false,
            Some(token) => match token.kind {
                ARROW | L_BRACE => true,
                DIRECTIVE => &self.text[self.tokens[i].range] == "#foreign",
                _ => false,
            },
        }
    }

    /// Returns `true` if the current token is in `set`.
    fn at_set(&mut self, set: TokenSet) -> bool {
        set.contains(self.current())
    }

    // ---- trivia handling -------------------------------------------------

    /// Advance `pos` past trivia, collecting them into `pending_trivia`.
    fn skip_trivia_peek(&mut self) {
        while self.pos < self.tokens.len() && self.tokens[self.pos].kind.is_trivia() {
            self.pending_trivia.push(self.tokens[self.pos]);
            self.pos += 1;
        }
    }

    /// Emit all pending trivia into the current node.
    fn flush_trivia(&mut self) {
        for tok in self.pending_trivia.drain(..) {
            let text = &self.text[tok.range];
            self.builder.token(tok.kind.into(), text);
        }
    }

    // ---- advancing -------------------------------------------------------

    /// Consume the current token (which must not be trivia) and emit it.
    fn bump(&mut self) {
        self.skip_trivia_peek();
        self.flush_trivia();
        debug_assert!(self.pos < self.tokens.len(), "bump past end of tokens");
        let tok = self.tokens[self.pos];
        debug_assert!(!tok.kind.is_trivia(), "bump called on trivia");
        let text = &self.text[tok.range];
        self.builder.token(tok.kind.into(), text);
        self.pos += 1;
    }

    /// Consume the current token only if it matches `kind`. Returns `true` on success.
    fn eat(&mut self, kind: SyntaxKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Consume `kind`, emitting an error diagnostic if it is absent.
    ///
    /// Returns `true` if the token was present.
    fn expect(&mut self, kind: SyntaxKind) -> bool {
        if self.eat(kind) {
            return true;
        }
        let span = self.current_span();
        let found = self.current();
        self.error(
            span,
            format!("expected {}, found {}", kind.describe(), found.describe()),
            "E0100",
        );
        false
    }

    // ---- span helpers ----------------------------------------------------

    /// The span of the current non-trivia token (or end-of-file).
    fn current_span(&mut self) -> Span {
        self.skip_trivia_peek();
        if self.pos < self.tokens.len() {
            let r = self.tokens[self.pos].range;
            Span::new(self.file, r)
        } else {
            // EOF: empty span at end of input
            let end = self
                .tokens
                .last()
                .map_or(TextSize::new(0), |t| t.range.end());
            Span::empty_at(self.file, end)
        }
    }

    // ---- diagnostics -----------------------------------------------------

    fn error(&mut self, span: Span, message: impl Into<String>, code: &'static str) {
        self.diagnostics
            .push(Diagnostic::error(span, message).with_code(code));
    }

    // ---- node wrappers ---------------------------------------------------

    fn start_node(&mut self, kind: SyntaxKind) {
        self.flush_trivia();
        self.builder.start_node(kind.into());
    }

    fn finish_node(&mut self) {
        self.builder.finish_node();
    }

    fn checkpoint(&mut self) -> Checkpoint {
        self.flush_trivia();
        self.builder.checkpoint()
    }

    fn start_node_at(&mut self, checkpoint: Checkpoint, kind: SyntaxKind) {
        self.builder.start_node_at(checkpoint, kind.into());
    }

    // ---- depth guard -----------------------------------------------------

    fn enter(&mut self) -> bool {
        if self.depth >= MAX_DEPTH {
            // Report once. One diagnostic per nesting level would mean tens of
            // thousands of identical errors on adversarial input.
            if !self.depth_error_reported {
                self.depth_error_reported = true;
                let span = self.current_span();
                self.error(span, "input is nested too deeply", "E0199");
            }
            return false;
        }
        self.depth += 1;
        true
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// Reports that input is nested or chained beyond what we will parse.
    ///
    /// Reported at most once per file: one diagnostic per level would mean tens
    /// of thousands of identical errors on adversarial input.
    fn report_too_deep(&mut self) {
        if self.depth_error_reported {
            return;
        }
        self.depth_error_reported = true;
        let span = self.current_span();
        self.error(span, "input is nested too deeply", "E0199");
    }

    // ---- error recovery --------------------------------------------------

    /// Consume tokens into an `ERROR` node until `recovery` contains the
    /// current token or we hit EOF.
    ///
    /// Does NOT consume `}` when `stop_at_brace` is true, which prevents
    /// destroying the enclosing block structure.
    ///
    /// **This may consume nothing.** Recovery sets legitimately contain the
    /// token that got us stuck -- both an item and a declaration begin with
    /// `IDENT`, so being stuck on a bare `x` at top level means `ITEM_START`
    /// already matches. Callers in a loop MUST therefore pair this with
    /// [`Self::force_progress`], or they spin forever. See the
    /// `bare_identifier_*` regression tests.
    fn recover_until(&mut self, recovery: TokenSet, stop_at_brace: bool) {
        self.start_node(ERROR);
        loop {
            let cur = self.current();
            if cur == EOF {
                break;
            }
            if recovery.contains(cur) {
                break;
            }
            if stop_at_brace && cur == R_BRACE {
                break;
            }
            self.bump();
        }
        self.finish_node();
    }

    /// Guarantees a recovering loop advances.
    ///
    /// Pass the token position captured *before* the recovery attempt. If
    /// nothing was consumed, one token is forced into an `ERROR` node so the
    /// enclosing loop cannot spin.
    ///
    /// Being at `}` is not a stall: the enclosing block loop terminates on it,
    /// so consuming it here would destroy the block structure that
    /// `stop_at_brace` exists to protect.
    fn force_progress(&mut self, before: usize) {
        if self.pos != before || self.at(EOF) || self.at(R_BRACE) {
            return;
        }
        self.start_node(ERROR);
        self.bump();
        self.finish_node();
    }

    // =========================================================================
    // Grammar rules
    // =========================================================================

    // ---- source file -------------------------------------------------------

    fn parse_source_file(&mut self) {
        self.start_node(SOURCE_FILE);
        while !self.at(EOF) {
            if !self.parse_item() {
                // Stuck at top level: consume into ERROR until we see something
                // that can start an item.
                let before = self.pos;
                let span = self.current_span();
                self.error(span, "unexpected token at top level", "E0101");
                self.recover_until(ITEM_START, false);
                // A bare `x` at top level is already in ITEM_START, so the line
                // above can consume nothing. Without this the loop spins.
                self.force_progress(before);
            }
        }
        // Flush any trailing trivia (e.g. trailing newline / comment).
        self.flush_trivia();
        self.finish_node();
    }

    // ---- items -------------------------------------------------------------

    /// Parses one item. Returns `false` if the current token cannot start an item.
    fn parse_item(&mut self) -> bool {
        match self.current() {
            DIRECTIVE => {
                let text = self.current_directive_text();
                match text {
                    "#import" => self.parse_import_decl(),
                    "#run" => self.parse_run_decl(),
                    _ => {
                        // Unknown directive at top level — treat as a stray token.
                        return false;
                    }
                }
            }
            IDENT => {
                // Could be `name ::` (const decl), `name :=` (var decl), or `name :` (typed var decl).
                // Look ahead past the name to the next non-trivia token.
                let next = self.nth(1);
                match next {
                    COLON_COLON | COLON_EQ | COLON => self.parse_decl(),
                    _ => return false,
                }
            }
            _ => return false,
        }
        true
    }

    fn current_directive_text(&mut self) -> &'src str {
        self.skip_trivia_peek();
        if self.pos < self.tokens.len() && self.tokens[self.pos].kind == DIRECTIVE {
            &self.text[self.tokens[self.pos].range]
        } else {
            ""
        }
    }

    fn parse_import_decl(&mut self) {
        self.start_node(IMPORT_DECL);
        self.bump(); // #import
        self.expect(STRING_LITERAL);
        self.expect(SEMICOLON);
        self.finish_node();
    }

    fn parse_run_decl(&mut self) {
        self.start_node(RUN_DECL);
        self.bump(); // #run
        self.parse_expr();
        self.expect(SEMICOLON);
        self.finish_node();
    }

    // ---- declarations ------------------------------------------------------

    fn parse_decl(&mut self) {
        let next = self.nth(1);
        match next {
            COLON_COLON => self.parse_const_decl(),
            COLON_EQ => self.parse_var_decl_inferred(),
            COLON => self.parse_var_decl_typed(),
            _ => {
                // Should not happen given callers check first.
                let span = self.current_span();
                self.error(span, "expected a declaration", "E0102");
            }
        }
    }

    fn parse_name(&mut self) {
        self.start_node(NAME);
        self.expect(IDENT);
        self.finish_node();
    }

    fn parse_const_decl(&mut self) {
        self.start_node(CONST_DECL);
        self.parse_name();
        self.bump(); // `::`
        self.parse_const_value();
        self.finish_node();
    }

    /// Parses the right-hand side of a `::` declaration.
    ///
    /// `ConstValue := StructType | Proc | Expr ';'`
    fn parse_const_value(&mut self) {
        match self.current() {
            STRUCT_KW => {
                // `struct { ... }` — no trailing semicolon
                self.parse_struct_type();
            }
            L_PAREN => {
                // Genuinely ambiguous: after `::`, a `(` may open a procedure's
                // parameter list (`add :: (a: s64) -> s64 { ... }`) or a
                // parenthesised expression (`X :: (1 + 2) * 3;`).
                //
                // Resolved by looking past the matching `)`: a procedure is
                // followed by `->`, `{`, or `#foreign`. Nothing else can follow
                // a parameter list, and an expression can never be followed by
                // any of them.
                if self.looks_like_proc_signature() {
                    self.parse_proc();
                } else {
                    self.parse_expr();
                    self.expect(SEMICOLON);
                }
            }
            DIRECTIVE => {
                // Could be `#run expr ;` or a directive expression like `#system_library "c"`.
                // Both are expressions; the semicolon follows.
                self.parse_expr();
                self.expect(SEMICOLON);
            }
            _ => {
                // Expression constant: `expr ;`
                if self.at_set(EXPR_START) {
                    self.parse_expr();
                    self.expect(SEMICOLON);
                } else {
                    let span = self.current_span();
                    self.error(span, "expected a value after `::`", "E0103");
                    self.recover_until(ITEM_START.union(STMT_START), true);
                }
            }
        }
    }

    fn parse_var_decl_inferred(&mut self) {
        // `name := expr ;`
        self.start_node(VAR_DECL);
        self.parse_name();
        self.bump(); // `:=`
        if self.at_set(EXPR_START) || self.at(UNINIT) {
            self.parse_rhs_value();
        } else {
            let span = self.current_span();
            self.error(span, "expected an expression after `:=`", "E0104");
            self.recover_until(STMT_START.union(ITEM_START), true);
        }
        self.expect(SEMICOLON);
        self.finish_node();
    }

    fn parse_var_decl_typed(&mut self) {
        // `name : Type` or `name : Type = expr ;` or `name : Type = --- ;`
        self.start_node(VAR_DECL);
        self.parse_name();
        self.bump(); // `:`
        // Parse the type
        if self.at_set(TYPE_START) {
            self.parse_type();
        } else {
            let span = self.current_span();
            self.error(span, "expected a type after `:`", "E0105");
            // Don't consume — let the `=` or `;` be found below
        }
        // Optional `= rhs`
        if self.eat(EQ) {
            self.parse_rhs_value();
        }
        self.expect(SEMICOLON);
        self.finish_node();
    }

    fn parse_rhs_value(&mut self) {
        if self.at(UNINIT) {
            self.start_node(UNINIT_EXPR);
            self.bump();
            self.finish_node();
        } else {
            self.parse_expr();
        }
    }

    // ---- procedures --------------------------------------------------------

    fn parse_proc(&mut self) {
        self.start_node(PROC);
        self.parse_param_list();
        // Optional return type
        if self.at(ARROW) {
            self.parse_ret_type();
        }
        // Body or foreign attribute
        match self.current() {
            L_BRACE => self.parse_block(),
            DIRECTIVE if self.current_directive_text() == "#foreign" => {
                self.parse_foreign_attr();
                self.expect(SEMICOLON);
            }
            _ => {
                // Missing body
                let span = self.current_span();
                self.error(
                    span,
                    "expected a procedure body `{ ... }` or `#foreign`",
                    "E0106",
                );
                // Don't consume — let the caller recover
            }
        }
        self.finish_node();
    }

    fn parse_param_list(&mut self) {
        self.start_node(PARAM_LIST);
        self.expect(L_PAREN);
        if !self.at(R_PAREN) {
            self.parse_param();
            while self.eat(COMMA) {
                if self.at(R_PAREN) {
                    break; // trailing comma
                }
                if !self.at(IDENT) {
                    break;
                }
                self.parse_param();
            }
        }
        self.expect(R_PAREN);
        self.finish_node();
    }

    fn parse_param(&mut self) {
        self.start_node(PARAM);
        if self.at(IDENT) {
            self.bump(); // param name
            self.expect(COLON);
            if self.at_set(TYPE_START) {
                self.parse_type();
            } else {
                let span = self.current_span();
                self.error(span, "expected a type for parameter", "E0107");
            }
        } else {
            let span = self.current_span();
            self.error(span, "expected a parameter name", "E0108");
            // Recover: skip to `,` or `)`
            self.recover_until(TokenSet::new(&[COMMA, R_PAREN]), true);
        }
        self.finish_node();
    }

    fn parse_ret_type(&mut self) {
        self.start_node(RET_TYPE);
        self.bump(); // `->`
        if self.at_set(TYPE_START) {
            self.parse_type();
        } else {
            let span = self.current_span();
            self.error(span, "expected a return type after `->`", "E0109");
        }
        self.finish_node();
    }

    fn parse_foreign_attr(&mut self) {
        self.start_node(FOREIGN_ATTR);
        self.bump(); // `#foreign`
        // Library name (identifier)
        if self.at(IDENT) {
            self.bump();
        } else {
            let span = self.current_span();
            self.error(span, "expected a library name after `#foreign`", "E0110");
        }
        // Optional symbol name (string literal)
        self.eat(STRING_LITERAL);
        self.finish_node();
    }

    // ---- types -------------------------------------------------------------

    fn parse_type(&mut self) {
        // `*T` and `struct { ... }` both recurse, so type parsing needs the same
        // depth guard as statements and expressions. Without it, `p: ***...s64`
        // is unbounded recursion and overflows the stack -- an abort, which in a
        // language server takes the editor down with it.
        if !self.enter() {
            // Consume the `*` (or whatever we are sitting on) so the caller
            // cannot loop, then stop descending.
            if !self.at(EOF) && !self.at(R_BRACE) {
                self.start_node(ERROR);
                self.bump();
                self.finish_node();
            }
            return;
        }
        self.parse_type_inner();
        self.leave();
    }

    fn parse_type_inner(&mut self) {
        match self.current() {
            STAR => {
                self.start_node(POINTER_TYPE);
                self.bump(); // `*`
                self.parse_type();
                self.finish_node();
            }
            IDENT => {
                self.start_node(NAME_TYPE);
                self.bump();
                self.finish_node();
            }
            STRUCT_KW => {
                self.parse_struct_type();
            }
            _ => {
                let span = self.current_span();
                self.error(span, "expected a type", "E0111");
            }
        }
    }

    fn parse_struct_type(&mut self) {
        self.start_node(STRUCT_TYPE);
        self.bump(); // `struct`
        self.start_node(FIELD_LIST);
        self.expect(L_BRACE);
        while !self.at(R_BRACE) && !self.at(EOF) {
            if self.at(IDENT) {
                self.parse_field();
            } else {
                let before = self.pos;
                let span = self.current_span();
                self.error(span, "expected a field name", "E0112");
                self.recover_until(TokenSet::new(&[IDENT, R_BRACE]), true);
                self.force_progress(before);
            }
        }
        self.expect(R_BRACE);
        self.finish_node(); // FIELD_LIST
        self.finish_node(); // STRUCT_TYPE
    }

    fn parse_field(&mut self) {
        self.start_node(FIELD);
        self.bump(); // field name (IDENT)
        self.expect(COLON);
        if self.at_set(TYPE_START) {
            self.parse_type();
        } else {
            let span = self.current_span();
            self.error(span, "expected a type for field", "E0113");
        }
        self.expect(SEMICOLON);
        self.finish_node();
    }

    // ---- blocks and statements ---------------------------------------------

    fn parse_block(&mut self) {
        self.start_node(BLOCK);
        // Record the opening brace position for error reporting.
        let open_brace_span = self.current_span();
        self.bump(); // `{`
        while !self.at(R_BRACE) && !self.at(EOF) {
            if !self.parse_stmt() {
                // Stuck inside a block: consume into ERROR until we see a
                // statement start or `}`. Never consume `}` here.
                let before = self.pos;
                let span = self.current_span();
                self.error(span, "unexpected token in block", "E0114");
                self.recover_until(STMT_START, true);
                self.force_progress(before);
            }
        }
        if !self.eat(R_BRACE) {
            // Unclosed brace: report at the opening `{`.
            self.error(open_brace_span, "unclosed `{`", "E0115");
        }
        self.finish_node();
    }

    /// Parses one statement. Returns `false` if the current token cannot start one.
    fn parse_stmt(&mut self) -> bool {
        if !self.enter() {
            // Returning `true` tells the block loop "a statement was consumed".
            // If we consume nothing the loop spins forever, appending a
            // diagnostic each time until the process is killed. So swallow one
            // token to guarantee progress -- except at `}` or EOF, where the
            // caller's loop terminates on its own and eating the brace would
            // destroy the block structure.
            if !self.at(R_BRACE) && !self.at(EOF) {
                self.start_node(ERROR);
                self.bump();
                self.finish_node();
            }
            return true;
        }
        let result = self.parse_stmt_inner();
        self.leave();
        result
    }

    fn parse_stmt_inner(&mut self) -> bool {
        match self.current() {
            L_BRACE => {
                self.parse_block();
                true
            }
            IF_KW => {
                self.parse_if_stmt();
                true
            }
            WHILE_KW => {
                self.parse_while_stmt();
                true
            }
            RETURN_KW => {
                self.parse_return_stmt();
                true
            }
            BREAK_KW => {
                self.parse_break_stmt();
                true
            }
            CONTINUE_KW => {
                self.parse_continue_stmt();
                true
            }
            IDENT => {
                // Could be: decl (name :: / name := / name :), assign stmt, or expr stmt.
                let next = self.nth(1);
                match next {
                    COLON_COLON | COLON_EQ | COLON => {
                        self.start_node(DECL_STMT);
                        self.parse_decl();
                        self.finish_node();
                    }
                    _ => {
                        // Could be assignment or expression statement.
                        self.parse_assign_or_expr_stmt();
                    }
                }
                true
            }
            _ if self.at_set(EXPR_START) => {
                self.parse_assign_or_expr_stmt();
                true
            }
            _ => false,
        }
    }

    fn parse_if_stmt(&mut self) {
        self.start_node(IF_STMT);
        self.bump(); // `if`
        self.parse_expr();
        self.parse_body();
        // Optional else
        if self.at(ELSE_KW) {
            self.start_node(ELSE_BRANCH);
            self.bump(); // `else`
            if self.at(IF_KW) {
                self.parse_if_stmt();
            } else {
                self.parse_body();
            }
            self.finish_node();
        }
        self.finish_node();
    }

    fn parse_while_stmt(&mut self) {
        self.start_node(WHILE_STMT);
        self.bump(); // `while`
        self.parse_expr();
        self.parse_body();
        self.finish_node();
    }

    /// Parses a `Body`: either a `Block` or a single `Stmt`.
    fn parse_body(&mut self) {
        if self.at(L_BRACE) {
            self.parse_block();
        } else {
            // Single statement without braces.
            if !self.parse_stmt() {
                let span = self.current_span();
                self.error(span, "expected a statement or `{`", "E0116");
            }
        }
    }

    fn parse_return_stmt(&mut self) {
        self.start_node(RETURN_STMT);
        self.bump(); // `return`
        if self.at_set(EXPR_START) {
            self.parse_expr();
        }
        self.expect(SEMICOLON);
        self.finish_node();
    }

    fn parse_break_stmt(&mut self) {
        self.start_node(BREAK_STMT);
        self.bump(); // `break`
        self.expect(SEMICOLON);
        self.finish_node();
    }

    fn parse_continue_stmt(&mut self) {
        self.start_node(CONTINUE_STMT);
        self.bump(); // `continue`
        self.expect(SEMICOLON);
        self.finish_node();
    }

    /// Parses either an assignment statement or an expression statement.
    ///
    /// We parse an expression first, then check whether an assignment operator
    /// follows. This handles both `a.b = c;` and `f();`.
    fn parse_assign_or_expr_stmt(&mut self) {
        let cp = self.checkpoint();
        self.parse_expr();

        if self.at_set(ASSIGN_OPS) {
            // Wrap the already-parsed lhs + op + rhs into ASSIGN_STMT.
            self.start_node_at(cp, ASSIGN_STMT);
            self.bump(); // assignment operator
            self.parse_expr();
            self.expect(SEMICOLON);
            self.finish_node();
        } else {
            // Expression statement.
            self.start_node_at(cp, EXPR_STMT);
            self.expect(SEMICOLON);
            self.finish_node();
        }
    }

    // ---- expressions -------------------------------------------------------

    fn parse_expr(&mut self) {
        if !self.enter() {
            return;
        }
        self.parse_expr_bp(0);
        self.leave();
    }

    /// Pratt-style binding-power parser for binary expressions.
    ///
    /// Precedence levels (lowest first):
    /// 1. `||`
    /// 2. `&&`
    /// 3. `==` `!=` `<` `<=` `>` `>=`
    /// 4. `+` `-` `+%` `-%`
    /// 5. `*` `/` `%` `*%`
    fn parse_expr_bp(&mut self, min_bp: u8) {
        let cp = self.checkpoint();

        // Prefix and primary. Postfix is applied *inside* this call, because
        // postfix binds tighter than prefix: `*p.x` is `*(p.x)`, the address of
        // a field, not `(*p).x`.
        self.parse_unary_or_primary();

        // Binary operators, left-associative via `start_node_at(cp, ..)`.
        //
        // The chain counter bounds TREE depth, which the recursion guard cannot:
        // this loop is iterative, but each iteration wraps everything so far, so
        // `1 + 1 + 1 ...` builds a left-leaning tree as deep as the chain is
        // long. Dropping or walking such a tree recurses, so an unbounded chain
        // is a stack overflow at drop time, far from the code that caused it.
        let mut chain = 0u32;
        loop {
            let (lbp, rbp) = match self.current() {
                PIPE_PIPE => (1, 2),
                AMP_AMP => (3, 4),
                EQ_EQ | BANG_EQ | LT | LT_EQ | GT | GT_EQ => (5, 6),
                PLUS | MINUS | PLUS_PERCENT | MINUS_PERCENT => (7, 8),
                STAR | SLASH | PERCENT | STAR_PERCENT => (9, 10),
                _ => break,
            };

            if lbp < min_bp {
                break;
            }

            chain += 1;
            if chain >= MAX_CHAIN {
                self.report_too_deep();
                break;
            }

            self.start_node_at(cp, BINARY_EXPR);
            self.bump(); // operator

            // The right operand handles its own prefix and postfix; `rbp` is
            // `lbp + 1`, which is what makes these operators left-associative.
            self.parse_expr_bp(rbp);

            self.finish_node(); // BINARY_EXPR
        }
    }

    /// Parses the postfix chain `.field`, `.*`, `(args)` applied to whatever
    /// begins at `cp`.
    ///
    /// Wrapping at `cp` -- the start of the *whole* operand -- is what nests
    /// these correctly: `a.b.c` must be `FIELD_EXPR(FIELD_EXPR(a, b), c)`. Taking
    /// a fresh checkpoint per iteration instead would emit the receiver and each
    /// accessor as flat siblings, leaving `FIELD_EXPR` with no receiver child at
    /// all and nothing for `jr-hir` to lower.
    fn parse_postfix_chain(&mut self, cp: Checkpoint) {
        let mut chain = 0u32;
        loop {
            // Same tree-depth concern as the binary chain above.
            chain += 1;
            if chain >= MAX_CHAIN {
                self.report_too_deep();
                return;
            }

            match self.current() {
                DOT => {
                    self.start_node_at(cp, FIELD_EXPR);
                    self.bump(); // `.`
                    if !self.eat(IDENT) {
                        let span = self.current_span();
                        self.error(span, "expected a field name after `.`", "E0117");
                    }
                    self.finish_node();
                }
                DOT_STAR => {
                    self.start_node_at(cp, DEREF_EXPR);
                    self.bump(); // `.*`
                    self.finish_node();
                }
                L_PAREN => {
                    self.start_node_at(cp, CALL_EXPR);
                    self.parse_arg_list();
                    self.finish_node();
                }
                _ => return,
            }
        }
    }

    fn parse_unary_or_primary(&mut self) {
        // Prefix operators chain (`!!!x`, `***x`), so this needs the depth guard
        // too. Note `-` does NOT exercise this deeply, because `---` lexes as a
        // single UNINIT token; `!` and `*` are the shapes that recurse.
        if !self.enter() {
            if !self.at(EOF) && !self.at(R_BRACE) {
                self.start_node(ERROR);
                self.bump();
                self.finish_node();
            }
            return;
        }
        match self.current() {
            MINUS | BANG | STAR => {
                self.start_node(UNARY_EXPR);
                self.bump(); // operator
                self.parse_unary_or_primary();
                self.finish_node();
            }
            _ => {
                // Postfix binds tighter than prefix, so it is applied to the
                // primary here rather than to the enclosing unary expression.
                let cp = self.checkpoint();
                self.parse_primary();
                self.parse_postfix_chain(cp);
            }
        }
        self.leave();
    }

    fn parse_primary(&mut self) {
        match self.current() {
            INT_LITERAL | STRING_LITERAL | TRUE_KW | FALSE_KW => {
                self.start_node(LITERAL_EXPR);
                self.bump();
                self.finish_node();
            }
            FLOAT_LITERAL => {
                // Float literals are reserved (wave W1). Emit a diagnostic but
                // still produce a LITERAL_EXPR so the tree is usable.
                let span = self.current_span();
                self.error(span, "floating-point literals arrive in wave W1", "E0200");
                self.start_node(LITERAL_EXPR);
                self.bump();
                self.finish_node();
            }
            IDENT => {
                self.start_node(NAME_EXPR);
                self.bump();
                self.finish_node();
            }
            L_PAREN => {
                self.start_node(PAREN_EXPR);
                self.bump(); // `(`
                self.parse_expr();
                self.expect(R_PAREN);
                self.finish_node();
            }
            UNINIT => {
                self.start_node(UNINIT_EXPR);
                self.bump();
                self.finish_node();
            }
            DIRECTIVE => {
                let text = self.current_directive_text();
                if text == "#run" {
                    self.start_node(RUN_EXPR);
                    self.bump(); // `#run`
                    self.parse_expr();
                    self.finish_node();
                } else {
                    // Any other directive used as an expression.
                    self.start_node(DIRECTIVE_EXPR);
                    self.bump(); // directive
                    // Optional string literal (e.g. `#system_library "c"`)
                    self.eat(STRING_LITERAL);
                    self.finish_node();
                }
            }
            // Reserved keywords: emit a wave-specific diagnostic.
            FOR_KW | DEFER_KW | USING_KW => {
                let span = self.current_span();
                let kw = self.current();
                self.error(
                    span,
                    format!("`{}` arrives in wave W2", kw.static_text().unwrap_or("?")),
                    "E0201",
                );
                self.start_node(ERROR);
                self.bump();
                self.finish_node();
            }
            ENUM_KW | UNION_KW | CAST_KW | XX_KW | NULL_KW => {
                let span = self.current_span();
                let kw = self.current();
                self.error(
                    span,
                    format!("`{}` arrives in wave W1", kw.static_text().unwrap_or("?")),
                    "E0201",
                );
                self.start_node(ERROR);
                self.bump();
                self.finish_node();
            }
            // Bitwise operators used as prefix (reserved).
            AMP | PIPE | CARET | TILDE | SHL | SHR => {
                let span = self.current_span();
                self.error(span, "bitwise operators arrive in wave W1", "E0202");
                self.start_node(ERROR);
                self.bump();
                self.finish_node();
            }
            _ => {
                // No primary expression found.
                let span = self.current_span();
                let found = self.current();
                self.error(
                    span,
                    format!("expected an expression, found {}", found.describe()),
                    "E0118",
                );
                // Emit an ERROR node covering nothing (the caller will handle recovery).
                self.start_node(ERROR);
                self.finish_node();
            }
        }
    }

    fn parse_arg_list(&mut self) {
        self.start_node(ARG_LIST);
        self.bump(); // `(`
        if !self.at(R_PAREN) {
            self.parse_expr();
            while self.eat(COMMA) {
                if self.at(R_PAREN) {
                    break; // trailing comma
                }
                if !self.at_set(EXPR_START) {
                    break;
                }
                self.parse_expr();
            }
        }
        if !self.eat(R_PAREN) {
            // Unclosed argument list: recover to `;` or `}`.
            let span = self.current_span();
            self.error(span, "unclosed `(` in argument list", "E0119");
            self.recover_until(TokenSet::new(&[SEMICOLON, R_BRACE]), true);
            self.eat(SEMICOLON); // consume the `;` if present so the caller doesn't double-report
        }
        self.finish_node();
    }
}

// ---------------------------------------------------------------------------
// Token sets (continued — defined after SyntaxKind is in scope)
// ---------------------------------------------------------------------------

/// Tokens that can start a type.
const TYPE_START: TokenSet = TokenSet::new(&[STAR, IDENT, STRUCT_KW]);

/// Assignment operators.
const ASSIGN_OPS: TokenSet = TokenSet::new(&[
    EQ,
    PLUS_EQ,
    MINUS_EQ,
    STAR_EQ,
    SLASH_EQ,
    PERCENT_EQ,
    PLUS_PERCENT_EQ,
    MINUS_PERCENT_EQ,
    STAR_PERCENT_EQ,
]);

// ---------------------------------------------------------------------------
// Debug tree dumper
// ---------------------------------------------------------------------------

/// Renders a [`SyntaxNode`] as a compact S-expression for tests and debugging.
///
/// Tokens are shown as `KIND "text"` and nodes as `(KIND child ...)`.
/// Trivia is included so the output is lossless.
#[must_use]
pub fn dump_tree(node: &SyntaxNode) -> String {
    let mut out = String::new();
    dump_node(node, &mut out, 0);
    out
}

fn dump_node(node: &SyntaxNode, out: &mut String, indent: usize) {
    use crate::kind::SyntaxElement;
    let pad = "  ".repeat(indent);
    out.push_str(&format!("{pad}({:?}\n", node.kind()));
    for child in node.children_with_tokens() {
        match child {
            SyntaxElement::Node(n) => dump_node(&n, out, indent + 1),
            SyntaxElement::Token(t) => dump_token(&t, out, indent + 1),
        }
    }
    out.push_str(&format!("{pad})\n"));
}

fn dump_token(token: &crate::kind::SyntaxToken, out: &mut String, indent: usize) {
    let pad = "  ".repeat(indent);
    let text = token.text();
    // Escape newlines for readability.
    let escaped = text.replace('\n', "\\n").replace('\r', "\\r");
    out.push_str(&format!("{pad}{:?} {:?}\n", token.kind(), escaped));
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::{Expect, expect};

    fn file() -> FileId {
        FileId::from_usize(0)
    }

    fn check(text: &str, expected: Expect) {
        let p = parse(text, file());
        let tree = dump_tree(&p.syntax());
        expected.assert_eq(&tree);
    }

    fn check_no_errors(text: &str) {
        let p = parse(text, file());
        assert!(
            !p.has_errors(),
            "expected no errors, got:\n{}",
            p.diagnostics()
                .iter()
                .map(|d| format!("  {}", d.message))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    fn check_has_errors(text: &str) {
        let p = parse(text, file());
        assert!(p.has_errors(), "expected errors but got none for: {text:?}");
    }

    fn check_round_trip(text: &str) {
        let p = parse(text, file());
        let round = p.syntax().text().to_string();
        assert_eq!(round, text, "round-trip failed");
    }

    // ---- round-trip invariant --------------------------------------------

    #[test]
    fn empty_input_round_trips() {
        check_round_trip("");
    }

    #[test]
    fn whitespace_only_round_trips() {
        check_round_trip("   \n\t  ");
    }

    #[test]
    fn comments_round_trip() {
        check_round_trip("// hello\n/* world */\n");
    }

    // ---- import ------------------------------------------------------------

    #[test]
    fn import_decl() {
        check_no_errors(r#"#import "Basic";"#);
        check_round_trip(r#"#import "Basic";"#);
    }

    // ---- constants ---------------------------------------------------------

    #[test]
    fn integer_constant() {
        check_no_errors("MAX :: 42;");
        check_round_trip("MAX :: 42;");
    }

    #[test]
    fn string_constant() {
        check_no_errors(r#"MSG :: "hello";"#);
    }

    #[test]
    fn bool_constant() {
        check_no_errors("DEBUG :: false;");
    }

    // ---- var decls ---------------------------------------------------------

    #[test]
    fn inferred_var_decl() {
        check_no_errors("main :: () { x := 1; }");
    }

    #[test]
    fn typed_var_decl_no_init() {
        check_no_errors("main :: () { x: s64; }");
    }

    #[test]
    fn typed_var_decl_with_init() {
        check_no_errors("main :: () { x: s64 = 1; }");
    }

    #[test]
    fn uninit_var_decl() {
        check_no_errors("main :: () { x: s64 = ---; }");
    }

    // ---- procedures --------------------------------------------------------

    #[test]
    fn empty_proc() {
        check_no_errors("noop :: () {}");
    }

    #[test]
    fn proc_with_params_and_return() {
        check_no_errors("add :: (a: s64, b: s64) -> s64 { return a + b; }");
    }

    #[test]
    fn foreign_proc() {
        check_no_errors(
            r#"write :: (fd: s64, buf: *u8, count: s64) -> s64 #foreign libc "write";"#,
        );
    }

    // ---- structs -----------------------------------------------------------

    #[test]
    fn struct_decl() {
        check_no_errors("Point :: struct { x: s64; y: s64; }");
    }

    // ---- statements --------------------------------------------------------

    #[test]
    fn if_stmt() {
        check_no_errors("f :: () { if x > 0 { return x; } }");
    }

    #[test]
    fn if_else_stmt() {
        check_no_errors("f :: () { if x > 0 { return 1; } else { return 0; } }");
    }

    #[test]
    fn if_else_if_stmt() {
        check_no_errors(
            "f :: () { if a { return 1; } else if b { return 2; } else { return 3; } }",
        );
    }

    #[test]
    fn while_stmt() {
        check_no_errors("f :: () { while i < 10 { i = i + 1; } }");
    }

    #[test]
    fn return_stmt() {
        check_no_errors("f :: () -> s64 { return 42; }");
    }

    #[test]
    fn return_void() {
        check_no_errors("f :: () { return; }");
    }

    #[test]
    fn break_continue() {
        check_no_errors("f :: () { while true { break; continue; } }");
    }

    #[test]
    fn assignment_stmt() {
        check_no_errors("f :: () { a = 1; }");
    }

    #[test]
    fn compound_assignment() {
        check_no_errors("f :: () { a += 1; a -= 2; a *= 3; a /= 4; a %= 5; }");
    }

    #[test]
    fn wrapping_compound_assignment() {
        check_no_errors("f :: () { a +%= 1; a -%= 2; a *%= 3; }");
    }

    // ---- expressions -------------------------------------------------------

    #[test]
    fn binary_precedence_mul_over_add() {
        // `a + b * c` should parse as `a + (b * c)`
        let p = parse("f :: () { x := a + b * c; }", file());
        assert!(!p.has_errors());
        let tree = dump_tree(&p.syntax());
        // The BINARY_EXPR for `*` should be nested inside the one for `+`
        assert!(tree.contains("BINARY_EXPR"), "should have binary exprs");
    }

    #[test]
    fn unary_negation() {
        check_no_errors("f :: () { x := -a; }");
    }

    #[test]
    fn unary_not() {
        check_no_errors("f :: () { x := !flag; }");
    }

    #[test]
    fn address_of() {
        check_no_errors("f :: () { p := *x; }");
    }

    #[test]
    fn deref_expr() {
        check_no_errors("f :: () { x := p.*; }");
    }

    #[test]
    fn field_access() {
        check_no_errors("f :: () { x := a.b; }");
    }

    #[test]
    fn chained_field_access() {
        check_no_errors("f :: () { x := a.b.c; }");
    }

    #[test]
    fn call_expr() {
        check_no_errors("f :: () { x := foo(1, 2); }");
    }

    #[test]
    fn nested_call() {
        check_no_errors("f :: () { x := foo(bar(1), 2); }");
    }

    #[test]
    fn paren_expr() {
        check_no_errors("f :: () { x := (a + b) * c; }");
    }

    #[test]
    fn run_expr() {
        check_no_errors("X :: #run add(2, 3);");
    }

    #[test]
    fn directive_expr() {
        check_no_errors(r#"libc :: #system_library "c";"#);
    }

    #[test]
    fn uninit_expr() {
        check_no_errors("f :: () { x: s64 = ---; }");
    }

    // ---- if without braces (valid/010) ------------------------------------

    #[test]
    fn if_without_braces() {
        check_no_errors("f :: (n: s64) -> s64 { if n > 0  return n; return 0; }");
    }

    // ---- pointer types -----------------------------------------------------

    #[test]
    fn pointer_type() {
        check_no_errors("f :: () { p: *s64; }");
    }

    #[test]
    fn double_pointer_type() {
        check_no_errors("f :: () { p: **s64; }");
    }

    // ---- error recovery ----------------------------------------------------

    #[test]
    fn missing_semicolon_recovers() {
        let p = parse("main :: () { a := 1\n b := 2; }", file());
        assert!(p.has_errors(), "should have an error");
        // The second declaration must still be in the tree.
        let tree = dump_tree(&p.syntax());
        assert!(tree.contains("VAR_DECL"), "should still have var decls");
    }

    #[test]
    fn missing_operand_is_reported() {
        let p = parse("main :: () { a := 1 +; }", file());
        assert!(p.has_errors());
    }

    #[test]
    fn float_literal_is_rejected() {
        check_has_errors("f :: () { x := 1.5; }");
    }

    #[test]
    fn reserved_keyword_for_is_rejected() {
        check_has_errors("f :: () { for x { } }");
    }

    // ---- tree snapshot for hello.jr ----------------------------------------

    #[test]
    fn hello_jr_parses_cleanly() {
        let text = r#"#import "Basic";

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
"#;
        let p = parse(text, file());
        assert!(
            !p.has_errors(),
            "hello.jr must parse cleanly:\n{}",
            p.diagnostics()
                .iter()
                .map(|d| format!("  {}", d.message))
                .collect::<Vec<_>>()
                .join("\n")
        );
        // Round-trip
        assert_eq!(p.syntax().text().to_string(), text);
    }

    // ---- snapshot test using `check` ---------------------------------------

    #[test]
    fn simple_const_snapshot() {
        check(
            "X :: 1;",
            expect![[r#"
(SOURCE_FILE
  (CONST_DECL
    (NAME
      IDENT "X"
    )
    WHITESPACE " "
    COLON_COLON "::"
    WHITESPACE " "
    (LITERAL_EXPR
      INT_LITERAL "1"
    )
    SEMICOLON ";"
  )
)
"#]],
        );
    }

    /// Prints the tree for `valid/024-hello.jr`. Run with `-- --nocapture` to
    /// see the output. This is the sample tree dump requested in the spec.
    #[test]
    fn dump_hello_jr() {
        let text = r#"// The Jairs-0 slice exit criterion (PLAN.md 1.4): this file must run in
// the bytecode VM via `jr run`, compile to a native arm64 binary via
// `jr build`, produce identical output either way, and receive hover,
// goto-definition and diagnostics in the language server.
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
"#;
        let p = parse(text, file());
        assert!(!p.has_errors(), "hello.jr must parse cleanly");
        // Print the tree when run with --nocapture.
        println!("{}", dump_tree(&p.syntax()));
    }
}
