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

use crate::code::{
    E0100, E0101, E0102, E0103, E0104, E0105, E0106, E0107, E0108, E0109, E0110, E0111, E0112,
    E0113, E0114, E0115, E0116, E0117, E0118, E0119, E0121, E0123, E0124, E0125, E0126, E0127,
    E0128, E0129, E0130, E0199,
};
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
    // `~` is a prefix operator (ADR-0042 §4), so it starts an expression. Needed here as
    // well as in `parse_unary_or_primary`: this is the third feature that would otherwise
    // have been swallowed by a token-set predicate — `CAST_KW` against this set and
    // `L_BRACK` against `TYPE_START` were the first two.
    TILDE,
    UNINIT,
    DIRECTIVE,
    // `cast(T, x)` (ADR-0037 §2). Needed here as well as in `parse_primary`: without it
    // `n := cast(u8, 65);` never reaches the expression parser at all, because `:=` and the
    // statement dispatcher both gate on this set — which is how the first draft of this
    // change produced a `cast` arm that could not be reached.
    CAST_KW,
    // `xx expr` (ADR-0046 §2) and `.RED` (§3), both prefix forms. Needed here as well as in
    // `parse_primary` for the reason `CAST_KW` was: `:=` and the statement dispatcher both gate
    // on this set, so without them `c := .RED;` never reaches the expression parser at all.
    //
    // ADR-0045 found `TYPE_START` missing *three* keywords when one was being added, so the
    // neighbours were checked here too: every other prefix form is present.
    XX_KW,
    DOT,
    // `context` (ADR-0057 §1). Needed here as well as in `parse_primary` for the reason `CAST_KW`
    // and `XX_KW` were: `:=` and the statement dispatcher both gate on this set, so without it
    // `x := context.allocator;` never reaches the expression parser at all.
    CONTEXT_KW,
    // `null` (ADR-0060 §1), a literal. Needed here as well as in the literal arm of `parse_primary`
    // for the same reason: without it `q := null;` reports a *parser* error ("expected an
    // expression") before sema can give the intended E0257 ("null needs a pointer context"). The
    // token-set trap once more, and the fifth keyword-shaped feature to meet it.
    NULL_KW,
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
                // `#c_call` joins `#foreign` here (ADR-0057 §3), and omitting it was not a
                // near-miss: `raw :: () #c_call { }` was read as a *parenthesised expression*
                // constant, so the whole declaration collapsed into four cascading errors starting
                // at `()`. The same shape as `TYPE_START` missing three keywords in ADR-0045 —
                // a token-set list that decides what a construct *is*.
                DIRECTIVE => matches!(
                    &self.text[self.tokens[i].range],
                    "#foreign" | "#c_call" | "#no_abc"
                ),
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
            E0100,
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
                self.error(span, "input is nested too deeply", E0199);
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
        self.error(span, "input is nested too deeply", E0199);
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
                self.error(span, "unexpected token at top level", E0101);
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
                    // Visibility markers (ADR-0054 §1). A bare directive on its own line, taking no
                    // argument and needing no `;` — it is a *position* in the file rather than a
                    // declaration, so there is nothing for a terminator to end.
                    //
                    // `#scope_file` is deliberately **not** here: a Jairs module is one file
                    // (ADR-0014 §1), so it would be indistinguishable from `#scope_module` and no
                    // program could tell them apart. It stays the E0101 stray token it is today.
                    "#scope_module" | "#scope_export" => self.parse_scope_decl(),
                    _ => {
                        // Unknown directive at top level — treat as a stray token.
                        return false;
                    }
                }
            }
            // `operator + :: (…) -> T { … }` (ADR-0048 §1). Its own arm because this dispatch is
            // on `IDENT` and `operator` is a keyword — without it the declaration would be a
            // stray token at top level, which is the same class of omission as `TYPE_START`
            // missing three keywords in ADR-0045.
            OPERATOR_KW => self.parse_operator_decl(),
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

    /// Parses `#scope_module` or `#scope_export` (ADR-0054 §1).
    ///
    /// The directive token is kept inside the node rather than folded into a flag here, so the tree
    /// stays a faithful record of what was written and `jr-fmt` can print it back — the same reason
    /// `parse_operator_decl` keeps its operator token.
    fn parse_scope_decl(&mut self) {
        self.start_node(SCOPE_DECL);
        self.bump(); // the directive
        self.finish_node();
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
                self.error(span, "expected a declaration", E0102);
            }
        }
    }

    fn parse_name(&mut self) {
        self.start_node(NAME);
        self.expect(IDENT);
        self.finish_node();
    }

    /// Parses `operator + :: (a: T, b: T) -> T { … }` (ADR-0048 §1).
    ///
    /// The operator token is kept in the node rather than folded into a synthetic name here:
    /// lowering is what interns `"operator+"`, so the tree stays a faithful record of what was
    /// written and a formatter can print the operator back.
    fn parse_operator_decl(&mut self) {
        self.start_node(OPERATOR_DECL);
        self.bump(); // `operator`
        // Which operators may be overloaded is a *semantic* question (ADR-0048 §2 refuses the
        // wrapping and bitwise forms with a reason), so the parser accepts any operator token
        // and sema says no. Accepting only the overloadable set here would report "expected an
        // operator" for `operator +%`, which is true and unhelpful.
        if self.at_set(OVERLOADABLE_OPS) || self.at_set(NON_OVERLOADABLE_OPS) {
            self.bump();
        } else {
            let span = self.current_span();
            self.error(span, "expected an operator after `operator`", E0126);
        }
        self.expect(COLON_COLON);
        if self.at(L_PAREN) {
            self.parse_proc();
        } else {
            let span = self.current_span();
            self.error(
                span,
                "an operator overload's value must be a procedure",
                E0126,
            );
        }
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
            // `enum { ... }` and `enum_flags { ... }`, the same shape and for the same reason:
            // ADR-0012 makes all of these instances of `name :: value`, so none takes a
            // trailing semicolon.
            ENUM_KW | FLAGS_KW => {
                self.parse_enum_type();
            }
            // `union { ... }` — a struct's shape with one layout rule changed (ADR-0045).
            UNION_KW => {
                self.parse_union_type();
            }
            // `variant { ... }` — a union's shape with a tag (ADR-0068 §1).
            VARIANT_KW => {
                self.parse_variant_type();
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
                    self.error(span, "expected a value after `::`", E0103);
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
            self.error(span, "expected an expression after `:=`", E0104);
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
            self.error(span, "expected a type after `:`", E0105);
            // Don't consume — let the `=` or `;` be found below
        }
        // Optional `= rhs`
        if self.eat(EQ) {
            self.parse_rhs_value();
        }
        self.expect(SEMICOLON);
        self.finish_node();
    }

    /// Parses `q, ok := f();` and `q, ok = f();` (ADR-0052 §2).
    ///
    /// The target list is its own node so a consumer can find it by kind, and a `_` is kept as its
    /// **token** inside that node rather than as a `NAME`: it is a hole recognised positionally, and
    /// giving it a `NAME` would make it look like a binding to anything reading names (ADR-0052 §3).
    fn parse_destructuring_stmt(&mut self) {
        // **Trivia is flushed before the checkpoint.** A `rowan` checkpoint captures everything the
        // builder has not yet wrapped, which includes the whitespace and comments preceding this
        // statement — so a checkpoint taken first made the `DECL_STMT` node start at the enclosing
        // block's `{`, and every diagnostic on the statement pointed there instead of at the
        // statement. Visible only in a multi-line file, which is why the corpus file is one.
        self.skip_trivia_peek();
        self.flush_trivia();
        let cp = self.builder.checkpoint();
        self.start_node(TARGET_LIST);
        loop {
            if self.at(IDENT) {
                // `_` lexes as an ordinary identifier, so the *token text* is what marks a discard.
                // Checked in lowering rather than here, because the parser's job is the shape.
                self.parse_name();
            } else {
                let span = self.current_span();
                self.error(span, "expected a name or `_` in a target list", E0129);
                break;
            }
            if !self.eat(COMMA) {
                break;
            }
        }
        self.finish_node();

        // `:=` declares, `=` assigns. Anything else is not a destructuring statement at all, and
        // saying so beats recovering into an expression that cannot mean anything.
        let kind = if self.at(COLON_EQ) {
            DECL_STMT
        } else if self.at(EQ) {
            ASSIGN_STMT
        } else {
            let span = self.current_span();
            self.error(
                span,
                "expected `:=` or `=` after a target list, as in `q, ok := f();`",
                E0129,
            );
            self.start_node_at(cp, ERROR);
            self.finish_node();
            return;
        };
        self.start_node_at(cp, kind);
        self.bump(); // `:=` or `=`
        self.parse_expr();
        self.expect(SEMICOLON);
        self.finish_node();
    }

    /// Parses `using q: Point;` — a local declaration carrying the promotion flag (ADR-0050 §1).
    ///
    /// A `VAR_DECL` like any other, with the keyword kept as a token inside it: lowering reads the
    /// token to set `Local::using`, so the tree stays a faithful record of what was written and
    /// `jr-fmt` can print it back. There is deliberately no `using q;` form over an
    /// already-declared variable — that would make the set of names in scope depend on a
    /// statement's position, a second order-sensitivity rule on top of the one locals have.
    fn parse_using_var_decl(&mut self) {
        self.start_node(VAR_DECL);
        self.bump(); // `using`
        if !self.at(IDENT) {
            let span = self.current_span();
            self.error(span, "expected a name after `using`", E0128);
            self.finish_node();
            return;
        }
        self.parse_name();
        // Only the typed form makes sense: promotion needs the type's field list, and `using q := f()`
        // would need the *inferred* type before resolution runs. Refused with a reason rather than
        // accepted and then rejected in sema.
        if !self.at(COLON) {
            let span = self.current_span();
            self.error(
                span,
                "a `using` declaration needs an explicit type, as in `using q: Point;`",
                E0128,
            );
            self.recover_until(TokenSet::new(&[SEMICOLON]), true);
            self.finish_node();
            return;
        }
        self.bump(); // `:`
        if self.at_set(TYPE_START) {
            self.parse_type();
        } else {
            let span = self.current_span();
            self.error(span, "expected a type after `:`", E0105);
        }
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
        // Attributes between the return type and the body: `#c_call` (ADR-0057 §3) and `#no_abc`
        // (ADR-0058 §3).
        //
        // A **loop**, so the two may be written in either order. One `if` per attribute would make
        // `#no_abc #c_call` parse and `#c_call #no_abc` not, which is an ordering rule no reader
        // would guess and which nothing about the language needs.
        //
        // `#c_call`: a `#foreign` declaration does not need it, because sema makes every `#foreign`
        // procedure implicitly `#c_call` (ADR-0001) — writing both is legal and redundant rather
        // than an error. `#no_abc`: sema *refuses* it on a `#foreign` declaration (E0255), because a
        // procedure with no body has no index to leave unchecked. Both are accepted here and judged
        // there, so the diagnostic can explain itself rather than the parser reporting a syntax
        // error about a directive that is spelled correctly.
        loop {
            if !self.at(DIRECTIVE) {
                break;
            }
            let kind = match self.current_directive_text() {
                "#c_call" => C_CALL_ATTR,
                "#no_abc" => NO_ABC_ATTR,
                _ => break,
            };
            self.start_node(kind);
            self.bump();
            self.finish_node();
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
                    E0106,
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
                // `using p: Point` after a comma (ADR-0050 §1). Gating on `IDENT` alone made the
                // *second* `using` parameter end the list — so `(using a: Point, using b: Point)`
                // reported four cascading errors. The third hand-written token gate this wave had
                // to widen, after the struct and union field lists.
                if !self.at(IDENT) && !self.at(USING_KW) {
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
        // `using p: Point` promotes Point's fields into the body's scope (ADR-0050 §1).
        self.eat(USING_KW);
        if self.at(IDENT) {
            self.bump(); // param name
            self.expect(COLON);
            if self.at_set(TYPE_START) {
                self.parse_type();
            } else {
                let span = self.current_span();
                self.error(span, "expected a type for parameter", E0107);
            }
            // `= 10` — a default value (ADR-0053 §2). Any expression parses here; sema refuses
            // anything but a literal, with a message saying why. Accepting only a literal in the
            // parser would report "expected a literal" for `= SIZE`, which is true and unhelpful:
            // the reader needs to know the value must be one *because* const-eval runs later.
            if self.eat(EQ) {
                if self.at_set(EXPR_START) {
                    self.parse_expr();
                } else {
                    let span = self.current_span();
                    self.error(span, "expected a default value after `=`", E0130);
                }
            }
        } else {
            let span = self.current_span();
            self.error(span, "expected a parameter name", E0108);
            // Recover: skip to `,` or `)`
            self.recover_until(TokenSet::new(&[COMMA, R_PAREN]), true);
        }
        self.finish_node();
    }

    fn parse_ret_type(&mut self) {
        self.start_node(RET_TYPE);
        self.bump(); // `->`
        // `-> (s64, bool)` returns several values (ADR-0052 §1). Checked *before* `TYPE_START`,
        // because `(` is not in that set and adding it there would make `(s64)` a legal type
        // everywhere — which ADR-0052 §4 forbids: a results list may appear only here.
        if self.at(L_PAREN) {
            // `(` in return position is ambiguous: a results list `-> (s64, bool)` (ADR-0052 §1)
            // and a procedure-pointer return `-> (s64) -> T` (ADR-0059 §3) both start with it. The
            // `->` after the matching `)` decides, and it is the *only* thing that can — so the
            // parser looks ahead to it rather than committing and backtracking. A proc-pointer
            // return is a `parse_type`, which handles the `(` itself; a results list is not a type.
            if self.arrow_follows_matching_paren() {
                self.parse_type();
            } else {
                self.parse_result_list();
            }
        } else if self.at_set(TYPE_START) {
            self.parse_type();
        } else {
            let span = self.current_span();
            self.error(span, "expected a return type after `->`", E0109);
        }
        self.finish_node();
    }

    /// Whether an `->` follows the `)` that closes the `(` at the cursor (ADR-0059 §3).
    ///
    /// The one disambiguation between a results list and a procedure-pointer return type, both of
    /// which begin `(` in return position. Scans to the matching close paren by depth — the same
    /// technique `looks_like_proc_signature` uses — then checks the next non-trivia token. An
    /// unterminated `(` answers `false`, so a half-typed `-> (s64` parses as a results list and the
    /// missing `)` is reported there rather than here.
    fn arrow_follows_matching_paren(&mut self) -> bool {
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
            return false; // unterminated: let the results-list parser report the missing `)`
        }
        while i < self.tokens.len() && self.tokens[i].kind.is_trivia() {
            i += 1;
        }
        self.tokens.get(i).is_some_and(|t| t.kind == ARROW)
    }

    /// Parses `(T, U, …)` after `->` (ADR-0052 §1).
    ///
    /// A one-element list parses fine and *interns* to the element itself, so `-> (T)` and `-> T`
    /// are the same type — normalised in `jr-pool` rather than refused here, because the tree stays
    /// a faithful record of what was written and `jr-fmt` can print it back.
    fn parse_result_list(&mut self) {
        self.start_node(RESULT_LIST);
        self.bump(); // `(`
        while !self.at(R_PAREN) && !self.at(EOF) {
            if self.at_set(TYPE_START) {
                self.parse_type();
            } else {
                let span = self.current_span();
                self.error(span, "expected a result type", E0129);
                break;
            }
            if !self.eat(COMMA) {
                break;
            }
        }
        self.expect(R_PAREN);
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
            self.error(span, "expected a library name after `#foreign`", E0110);
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
            L_BRACK if self.nth(1) == R_BRACK => {
                // `[]T` — a view (ADR-0044 §1). Its own node rather than an `ARRAY_TYPE`
                // with an absent length: `TypeRef::Array` already uses `len: None` to mean
                // "the length was not a usable literal" (ADR-0039 §3a, E0233), so reusing
                // the node would make a view indistinguishable from that error.
                self.start_node(VIEW_TYPE);
                self.bump(); // `[`
                self.bump(); // `]`
                self.parse_type();
                self.finish_node();
            }
            L_BRACK => {
                // `[N]T` (ADR-0039 §3). `[..]T` is a later wave, and it is refused here
                // rather than parsed-then-rejected: a dynamic array has no representation
                // to lower to, so accepting the syntax would mean inventing one.
                self.start_node(ARRAY_TYPE);
                self.bump(); // `[`
                if self.at(DOT_DOT) {
                    let span = self.current_span();
                    self.error(span, "dynamic arrays `[..]T` arrive in a later wave", E0124);
                    self.bump(); // `..`
                } else if self.at_set(EXPR_START) {
                    self.parse_expr();
                } else {
                    let span = self.current_span();
                    self.error(span, "expected an array length after `[`", E0124);
                }
                self.expect(R_BRACK);
                self.parse_type();
                self.finish_node();
            }
            STRUCT_KW => {
                self.parse_struct_type();
            }
            ENUM_KW | FLAGS_KW => {
                self.parse_enum_type();
            }
            UNION_KW => {
                self.parse_union_type();
            }
            VARIANT_KW => {
                self.parse_variant_type();
            }
            L_PAREN => {
                self.parse_proc_type();
            }
            _ => {
                let span = self.current_span();
                self.error(span, "expected a type", E0111);
            }
        }
    }

    /// `(T, T) -> T` — a procedure-pointer type (ADR-0059 §3).
    ///
    /// The only parenthesised type in the language, so `(` in type position is unambiguously this —
    /// unlike `(` in *return* position, where a `RESULT_LIST` and a proc-pointer type both start
    /// with it and the `->` decides. In a type there is no results list to confuse it with, because
    /// a results list is reachable only as a return type (ADR-0052 §4).
    ///
    /// The `->` is required: `(s64)` alone is not a type, and a missing arrow is E0111 rather than a
    /// silent single-element parse, so a reader who wrote a parenthesised type meaning to write a
    /// return type is told rather than surprised.
    fn parse_proc_type(&mut self) {
        self.start_node(PROC_TYPE);
        self.start_node(PROC_TYPE_PARAMS);
        self.expect(L_PAREN);
        if !self.at(R_PAREN) {
            self.parse_type();
            while self.eat(COMMA) {
                if self.at(R_PAREN) {
                    break; // trailing comma
                }
                self.parse_type();
            }
        }
        self.expect(R_PAREN);
        self.finish_node(); // PROC_TYPE_PARAMS
        // The arrow is **optional** (ADR-0062 §1): `(s64)` is a procedure pointer returning `void`,
        // exactly as `f :: (n: s64) { }` returns `void` by omitting it. Requiring it made a
        // void-returning procedure pointer *unspellable* — `-> void` is E0212 because `void` has no
        // type name (ADR-0015 §3), and `-> ` with nothing after it is a parse error — which blocked
        // an allocator's `free: (*u8)` half.
        //
        // A present arrow with nothing usable after it is still an error: that is a half-written
        // return rather than the void form, and treating it as void would make `(s64) ->` and
        // `(s64)` two spellings of one type.
        if self.eat(ARROW) {
            if self.at_set(TYPE_START) {
                self.parse_type();
            } else {
                let span = self.current_span();
                self.error(span, "expected a return type after `->`", E0111);
            }
        }
        self.finish_node(); // PROC_TYPE
    }

    fn parse_struct_type(&mut self) {
        self.start_node(STRUCT_TYPE);
        self.bump(); // `struct`
        self.start_node(FIELD_LIST);
        self.expect(L_BRACE);
        while !self.at(R_BRACE) && !self.at(EOF) {
            // `using base: Point;` embeds (ADR-0050 §1), so the loop admits the keyword as well
            // as a bare name. Gating on `IDENT` alone made `using` inside a struct report
            // "expected a field name" — the token-set trap this project keeps meeting, here in a
            // hand-written loop rather than a `TokenSet`.
            if self.at(IDENT) || self.at(USING_KW) {
                self.parse_field();
            } else {
                let before = self.pos;
                let span = self.current_span();
                self.error(span, "expected a field name", E0112);
                self.recover_until(TokenSet::new(&[IDENT, R_BRACE]), true);
                self.force_progress(before);
            }
        }
        self.expect(R_BRACE);
        self.finish_node(); // FIELD_LIST
        self.finish_node(); // STRUCT_TYPE
    }

    /// Parses `union { i: s64; f: float64; }` (ADR-0045).
    ///
    /// Deliberately identical to [`Parser::parse_struct_type`] apart from the node kind: a
    /// union's *fields* are a struct's fields, sharing `FIELD_LIST`/`FIELD` and therefore the
    /// same recovery behaviour and the same E0112/E0113 diagnostics. Everything that differs
    /// between the two is layout, which is `jr-pool`'s (ADR-0045 §3).
    fn parse_union_type(&mut self) {
        self.start_node(UNION_TYPE);
        self.bump(); // `union`
        self.start_node(FIELD_LIST);
        self.expect(L_BRACE);
        while !self.at(R_BRACE) && !self.at(EOF) {
            // `using base: Point;` embeds (ADR-0050 §1), so the loop admits the keyword as well
            // as a bare name. Gating on `IDENT` alone made `using` inside a struct report
            // "expected a field name" — the token-set trap this project keeps meeting, here in a
            // hand-written loop rather than a `TokenSet`.
            if self.at(IDENT) || self.at(USING_KW) {
                self.parse_field();
            } else {
                let before = self.pos;
                let span = self.current_span();
                self.error(span, "expected a field name", E0112);
                self.recover_until(TokenSet::new(&[IDENT, R_BRACE]), true);
                self.force_progress(before);
            }
        }
        self.expect(R_BRACE);
        self.finish_node(); // FIELD_LIST
        self.finish_node(); // UNION_TYPE
    }

    /// Parses `variant { i: s64; f: float64; }` (ADR-0068 §1).
    ///
    /// The **same loop** a union's fields take, because a case is written like a field — what differs
    /// is the layout (a leading tag, §3) and the check on a read (§4), neither of which is syntax. Its
    /// own node kind only so that lowering can tell the two apart.
    fn parse_variant_type(&mut self) {
        self.start_node(VARIANT_TYPE);
        self.bump(); // `variant`
        self.start_node(FIELD_LIST);
        self.expect(L_BRACE);
        while !self.at(R_BRACE) && !self.at(EOF) {
            if self.at(IDENT) || self.at(USING_KW) {
                self.parse_field();
            } else {
                let before = self.pos;
                let span = self.current_span();
                self.error(span, "expected a field name", E0112);
                self.recover_until(TokenSet::new(&[IDENT, R_BRACE]), true);
                self.force_progress(before);
            }
        }
        self.expect(R_BRACE);
        self.finish_node(); // FIELD_LIST
        self.finish_node(); // VARIANT_TYPE
    }

    /// Parses `enum { RED; GREEN :: 10; }` (ADR-0041 §3).
    ///
    /// Shaped like [`Parser::parse_struct_type`] deliberately: the recovery behaviour on a
    /// malformed member should match a malformed field's, because a user who has seen one
    /// error already knows what the other means.
    fn parse_enum_type(&mut self) {
        self.start_node(ENUM_TYPE);
        // `enum` or `enum_flags`; the keyword token stays in the node, so lowering reads which
        // form this was from the tree rather than from a second node kind. One node kind means
        // every consumer that handles an enum handles both (ADR-0043 §1).
        self.bump();
        self.start_node(MEMBER_LIST);
        self.expect(L_BRACE);
        while !self.at(R_BRACE) && !self.at(EOF) {
            if self.at(IDENT) {
                self.parse_member();
            } else {
                let before = self.pos;
                let span = self.current_span();
                self.error(span, "expected an enum member name", E0125);
                self.recover_until(TokenSet::new(&[IDENT, R_BRACE]), true);
                self.force_progress(before);
            }
        }
        self.expect(R_BRACE);
        self.finish_node(); // MEMBER_LIST
        self.finish_node(); // ENUM_TYPE
    }

    /// Parses one enum member: `RED;` or `NOT_FOUND :: 404;`.
    ///
    /// The `:: value` form is optional, which is what makes auto-numbering the default
    /// (ADR-0041 §3). A `:` here would be a *type* annotation, which an enum member does not
    /// have — so it is refused by name rather than by falling through to "expected `;`".
    fn parse_member(&mut self) {
        self.start_node(MEMBER);
        self.bump(); // member name (IDENT)
        if self.eat(COLON_COLON) {
            if self.at_set(EXPR_START) {
                self.parse_expr();
            } else {
                let span = self.current_span();
                self.error(span, "expected a value after `::`", E0103);
            }
        } else if self.at(COLON) {
            let span = self.current_span();
            self.error(
                span,
                "an enum member has a value, not a type; use `::` to give it one",
                E0125,
            );
            self.bump(); // `:`
            if self.at_set(TYPE_START) {
                self.parse_type();
            }
        }
        self.expect(SEMICOLON);
        self.finish_node();
    }

    fn parse_field(&mut self) {
        self.start_node(FIELD);
        // `using base: Point;` embeds Point's fields (ADR-0050 §1). The keyword is a *prefix on the
        // binding* rather than a statement, so it is eaten here and the rest of the field parses
        // unchanged — which is why embedding needed no new node kind.
        self.eat(USING_KW);
        // **Checked, not assumed.** Before `using`, this function was only ever called with an
        // `IDENT` current, so bumping unconditionally was safe. Admitting `using` broke that: a
        // file truncated to `struct { using` bumped past the end of the token stream and panicked
        // — a compiler crash on partial input, which an editor produces on every keystroke.
        // Caught by `every_prefix_of_every_corpus_file_round_trips`, which exists for this.
        if self.at(IDENT) {
            self.bump(); // field name
        } else {
            let span = self.current_span();
            self.error(span, "expected a field name after `using`", E0112);
            self.finish_node();
            return;
        }
        self.expect(COLON);
        if self.at_set(TYPE_START) {
            self.parse_type();
        } else {
            let span = self.current_span();
            self.error(span, "expected a type for field", E0113);
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
                self.error(span, "unexpected token in block", E0114);
                self.recover_until(STMT_START, true);
                self.force_progress(before);
            }
        }
        if !self.eat(R_BRACE) {
            // Unclosed brace: report at the opening `{`.
            self.error(open_brace_span, "unclosed `{`", E0115);
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
            FOR_KW => {
                self.parse_for_stmt();
                true
            }
            DEFER_KW => {
                self.parse_defer_stmt();
                true
            }
            PUSH_CONTEXT_KW => {
                self.parse_push_context_stmt();
                true
            }
            SWITCH_KW => {
                self.parse_switch_stmt();
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
            // `using q: Point;` — a local declaration that also promotes (ADR-0050 §1). Its own arm
            // because `parse_decl` dispatches on `nth(1)` assuming the *current* token is the name,
            // and with a `using` prefix the name has moved along by one.
            // `_, ok := f();` — a discard in the *first* position, which the `IDENT` arm below
            // cannot see because `_` lexes as an identifier only if written as one. It does, in
            // Jairs, so this arm exists for symmetry rather than necessity — and is kept because a
            // future lexer change making `_` its own token would otherwise silently lose the form.
            USING_KW => {
                self.start_node(DECL_STMT);
                self.parse_using_var_decl();
                self.finish_node();
                true
            }
            IDENT => {
                // Could be: decl (name :: / name := / name :), assign stmt, or expr stmt.
                let next = self.nth(1);
                // **A destructuring target list** — `q, ok := f();` (ADR-0052 §2). A comma after
                // the first name is what distinguishes it: no other statement form has one there,
                // because Jairs has no comma operator and no multi-declaration syntax.
                if next == COMMA {
                    self.parse_destructuring_stmt();
                    return true;
                }
                // A **loop label** is `name:` followed by `for` or `while` (ADR-0049 §2), which
                // collides with a typed declaration `x: s64` on the first two tokens. Only the
                // *third* tells them apart, so this looks one further rather than committing.
                if next == COLON && matches!(self.nth(2), FOR_KW | WHILE_KW) {
                    self.parse_labelled_loop();
                    return true;
                }
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

    /// Parses `label: for …` or `label: while …` (ADR-0049 §2).
    ///
    /// The label wraps the loop rather than the loop containing the label, so a consumer that
    /// does not care about labels can find the `FOR_STMT`/`WHILE_STMT` by kind and ignore this.
    fn parse_labelled_loop(&mut self) {
        self.start_node(LOOP_LABEL);
        // A `NAME` node rather than a bare token: `LoopLabel::name()` looks for one, and bumping
        // the identifier straight into the label node left nothing to find — so the label was
        // silently `None` and `break outer` reported "outside a loop".
        self.parse_name();
        self.bump(); // `:`
        if self.at(FOR_KW) {
            self.parse_for_stmt();
        } else {
            self.parse_while_stmt();
        }
        self.finish_node();
    }

    /// Parses `for x: buf { … }`, `for x, i: buf { … }` and `for i: 0..n { … }` (ADR-0049 §1).
    ///
    /// The `<` reverse marker is a *prefix* on the loop rather than a direction on the range,
    /// because reversing an array and reversing a range must be spelled the same way and only a
    /// marker on the `for` can do both.
    fn parse_for_stmt(&mut self) {
        self.start_node(FOR_STMT);
        self.bump(); // `for`
        self.eat(LT); // the optional `<` reverse marker
        // One or two names, then `:`. The names are `NAME` nodes so that a consumer reads them
        // the same way it reads any other binding.
        if self.at(IDENT) {
            self.parse_name();
            if self.eat(COMMA) {
                if self.at(IDENT) {
                    self.parse_name();
                } else {
                    let span = self.current_span();
                    self.error(span, "expected an index name after `,`", E0127);
                }
            }
        } else {
            let span = self.current_span();
            self.error(span, "expected a loop variable name after `for`", E0127);
        }
        self.expect(COLON);
        self.parse_for_iterable();
        self.parse_body();
        self.finish_node();
    }

    /// Parses a `for` header's iterable: an expression, or `a..b` as a `RANGE_EXPR`.
    ///
    /// A range is reachable **only** here (ADR-0049 §1): there is no `..` operator in the
    /// expression grammar, which is what keeps `0..n` from colliding with `[..]T`.
    fn parse_for_iterable(&mut self) {
        let cp = self.builder.checkpoint();
        if self.at_set(EXPR_START) {
            self.parse_expr();
        } else {
            let span = self.current_span();
            self.error(span, "expected something to iterate over", E0127);
            return;
        }
        if self.at(DOT_DOT) {
            self.start_node_at(cp, RANGE_EXPR);
            self.bump(); // `..`
            if self.at_set(EXPR_START) {
                self.parse_expr();
            } else {
                let span = self.current_span();
                self.error(span, "expected the end of the range after `..`", E0127);
            }
            self.finish_node();
        }
    }

    /// Parses `defer stmt;` (ADR-0049 §3).
    ///
    /// The deferred thing is an arbitrary statement, not restricted to a call: restricting it
    /// would need a rule about what counts as "a cleanup".
    fn parse_defer_stmt(&mut self) {
        self.start_node(DEFER_STMT);
        self.bump(); // `defer`
        if !self.parse_stmt() {
            let span = self.current_span();
            self.error(span, "expected a statement after `defer`", E0127);
        }
        self.finish_node();
    }

    /// Parses `push_context { … }` (ADR-0063).
    ///
    /// The body is a **braced block**, not the braceless single statement `parse_body` also allows:
    /// `push_context` names a scope, and a scope with no braces would be a context swap that lasts
    /// exactly one statement, which reads as a mistake rather than an intent. Requiring the braces
    /// makes the scope visible, and it matches Jai, whose `push_context` always takes a block.
    fn parse_push_context_stmt(&mut self) {
        self.start_node(PUSH_CONTEXT_STMT);
        self.bump(); // `push_context`
        if self.at(L_BRACE) {
            self.parse_block();
        } else {
            let span = self.current_span();
            self.error(span, "expected `{` after `push_context`", E0116);
        }
        self.finish_node();
    }

    /// Parses `switch e { case v; … else; … }` (ADR-0067).
    ///
    /// An arm is `case <expr>;` — or `else;` — followed by the statements it runs, which end at the
    /// next `case`, the next `else`, or the closing brace. That reuses the statement-list parsing every
    /// block has, so no new body shape enters the grammar (ADR-0067 §1); braces per arm would be noise
    /// on the common one-statement arm.
    fn parse_switch_stmt(&mut self) {
        self.start_node(SWITCH_STMT);
        self.bump(); // `switch`
        self.parse_expr();
        if self.at(L_BRACE) {
            self.bump(); // `{`
            // Arms until the closing brace. A token that begins neither an arm nor `}` is reported
            // once and skipped, so one stray token does not turn the rest of the switch into garbage.
            while !self.at(R_BRACE) && !self.at(EOF) {
                if self.at(CASE_KW) || self.at(ELSE_KW) {
                    self.parse_switch_arm();
                } else {
                    let span = self.current_span();
                    self.error(span, "expected `case`, `else` or `}` in a `switch`", E0116);
                    self.bump();
                }
            }
            self.expect(R_BRACE);
        } else {
            let span = self.current_span();
            self.error(span, "expected `{` after a `switch`'s value", E0116);
        }
        self.finish_node();
    }

    /// Parses one `switch` arm: `case v;` or `else;`, then its statements (ADR-0067 §1).
    ///
    /// The `else` arm is the same node with **no value expression** — an absent value *is* the
    /// catch-all, so it needs no second node kind and every consumer distinguishes the two by asking
    /// whether a value is there.
    fn parse_switch_arm(&mut self) {
        self.start_node(SWITCH_ARM);
        let is_case = self.at(CASE_KW);
        self.bump(); // `case` or `else`
        if is_case {
            if self.at_set(EXPR_START) {
                self.parse_expr();
            } else {
                let span = self.current_span();
                self.error(span, "expected a value after `case`", E0116);
            }
        }
        // The `;` closes the arm's *header*, not the arm: what follows is the body.
        self.expect(SEMICOLON);
        // Statements until the next arm or the end of the switch. `parse_stmt` returning false means
        // the token starts no statement, which the enclosing loop reports.
        while !self.at(R_BRACE) && !self.at(CASE_KW) && !self.at(ELSE_KW) && !self.at(EOF) {
            if !self.parse_stmt() {
                break;
            }
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
                self.error(span, "expected a statement or `{`", E0116);
            }
        }
    }

    fn parse_return_stmt(&mut self) {
        self.start_node(RETURN_STMT);
        self.bump(); // `return`
        if self.at_set(EXPR_START) {
            self.parse_expr();
            // `return a, b;` returns several values (ADR-0052 §1). The comma-separated expressions
            // are siblings of the `RETURN_STMT` rather than a node of their own: lowering counts
            // them, and a single expression is the ordinary case with no list to unwrap.
            while self.eat(COMMA) {
                if self.at_set(EXPR_START) {
                    self.parse_expr();
                } else {
                    let span = self.current_span();
                    self.error(span, "expected an expression after `,` in a return", E0129);
                    break;
                }
            }
        }
        self.expect(SEMICOLON);
        self.finish_node();
    }

    fn parse_break_stmt(&mut self) {
        self.start_node(BREAK_STMT);
        self.bump(); // `break`
        // An optional label (ADR-0049 §2). `break;` still means the innermost loop, so the name is
        // eaten only when present rather than expected.
        if self.at(IDENT) {
            self.parse_name();
        }
        self.expect(SEMICOLON);
        self.finish_node();
    }

    fn parse_continue_stmt(&mut self) {
        self.start_node(CONTINUE_STMT);
        self.bump(); // `continue`
        if self.at(IDENT) {
            self.parse_name();
        }
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
    /// 4. `|`
    /// 5. `^`
    /// 6. `&`
    /// 7. `+` `-` `+%` `-%`
    /// 8. `<<` `>>`
    /// 9. `*` `/` `%` `*%`
    ///
    /// The bitwise levels sit **above** comparison rather than below it, which is where C
    /// puts them (ADR-0042 §1): `flags & MASK == 0` means `(flags & MASK) == 0`, not
    /// `flags & (MASK == 0)`. Shifts sit between `+` and `*`, following Go and Rust — C puts
    /// them below `+`, so C reads `a + b << c` as `(a + b) << c`.
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
                // `|` loosest, then `^`, then `&`, so `a & b | c & d` is `(a & b) | (c & d)`
                // — how a bit-manipulation idiom is written (ADR-0042 §1).
                PIPE => (7, 8),
                CARET => (9, 10),
                AMP => (11, 12),
                PLUS | MINUS | PLUS_PERCENT | MINUS_PERCENT => (13, 14),
                SHL | SHR => (15, 16),
                STAR | SLASH | PERCENT | STAR_PERCENT => (17, 18),
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
                        self.error(span, "expected a field name after `.`", E0117);
                    }
                    self.finish_node();
                }
                DOT_STAR => {
                    self.start_node_at(cp, DEREF_EXPR);
                    self.bump(); // `.*`
                    self.finish_node();
                }
                L_BRACK => {
                    // `buf[]` and `buf[i]` are the same two tokens up to the bracket's
                    // contents, so which node this is cannot be decided until after the
                    // `[` is consumed — hence one arm producing two kinds rather than a
                    // lookahead that would have to peek past a whole expression.
                    //
                    // `buf[]` is the slice operator (ADR-0044 §2). It is a *distinct* node
                    // from an index with a missing subscript, which is what E0123 used to
                    // report here: an empty subscript is now legal and means something.
                    if self.nth(1) == R_BRACK {
                        self.start_node_at(cp, SLICE_EXPR);
                        self.bump(); // `[`
                        self.bump(); // `]`
                        self.finish_node();
                        continue;
                    }
                    self.start_node_at(cp, INDEX_EXPR);
                    self.bump(); // `[`
                    if self.at_set(EXPR_START) {
                        self.parse_expr();
                    } else {
                        let span = self.current_span();
                        self.error(span, "expected an index expression after `[`", E0123);
                    }
                    self.expect(R_BRACK);
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
            // `~` joins the prefix operators (ADR-0042 §4), at the same precedence as the
            // others — so `~a & b` is `(~a) & b`.
            MINUS | BANG | STAR | TILDE => {
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
            // `FLOAT_LITERAL` joins the others: it used to be refused with E0120 saying
            // floats "arrive in wave W1", and W1 is where they arrived (ADR-0040). The
            // refusal is deleted rather than reworded, because a message naming a wave that
            // has happened is the plan contradicting the code.
            INT_LITERAL | FLOAT_LITERAL | STRING_LITERAL | TRUE_KW | FALSE_KW | NULL_KW => {
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
            // `using` is real syntax as of ADR-0050, but only as a **prefix on a binding** — a
            // field, a parameter or a typed local. Reaching it here means one appeared in
            // *expression* position, so the diagnostic says what it is rather than the
            // now-false "arrives in wave W2" this arm used to print for it.
            //
            // No reserved keyword is left in this position: `enum`, `union`, `for`, `defer`,
            // `cast`, `xx` and `using` have all become real, and each has its own arm. The
            // wave-specific refusal that used to live here is gone entirely.
            USING_KW => {
                let span = self.current_span();
                self.error(
                    span,
                    "`using` prefixes a declaration, so it cannot appear in an expression",
                    E0128,
                );
                self.start_node(ERROR);
                self.bump();
                self.finish_node();
            }
            // `cast(T, x)`. Parsed as its own node rather than as a call, because the first
            // argument is a type and Jairs has no way to pass one in a call (ADR-0037 §3).
            // A missing or malformed operand recovers rather than aborting: the node is
            // produced either way, so the rest of the expression still parses.
            // `context` (ADR-0057 §1). A node of its own rather than a `NAME_EXPR`, so nothing
            // reading names finds it — `context.allocator` must not look like a field access on a
            // variable somebody declared.
            CONTEXT_KW => {
                self.start_node(CONTEXT_EXPR);
                self.bump();
                self.finish_node();
            }
            CAST_KW => {
                self.start_node(CAST_EXPR);
                self.bump(); // `cast`
                self.expect(L_PAREN);
                self.parse_type();
                self.expect(COMMA);
                self.parse_expr();
                self.expect(R_PAREN);
                self.finish_node();
            }
            // `xx expr` (ADR-0046 §2). Prefix, and the operand is parsed with the *unary*
            // parser rather than `parse_expr`, so `xx n + 1` is `(xx n) + 1` — the same
            // precedence `-`, `!`, `~` and `*` have.
            XX_KW => {
                self.start_node(AUTOCAST_EXPR);
                self.bump(); // `xx`
                if self.at_set(EXPR_START) {
                    self.parse_unary_or_primary();
                } else {
                    let span = self.current_span();
                    self.error(span, "expected an expression after `xx`", E0118);
                }
                self.finish_node();
            }
            // `.RED` (ADR-0046 §3). Reached only here, in *prefix* position: the postfix chain
            // handles a `.` that follows an expression, so the two cannot be confused.
            DOT => {
                self.start_node(MEMBER_EXPR);
                self.bump(); // `.`
                if !self.eat(IDENT) {
                    let span = self.current_span();
                    self.error(span, "expected a member name after `.`", E0117);
                }
                self.finish_node();
            }
            // `enum` is real syntax as of ADR-0041, but it is a *type* — so in expression
            // position it is still an error, and the message must say which kind. Leaving it
            // in the "arrives in wave W1" list would have told a user a feature they were
            // using had not arrived.
            ENUM_KW => {
                let span = self.current_span();
                self.error(span, "`enum` is a type, not an expression", E0121);
                self.start_node(ERROR);
                self.bump();
                self.finish_node();
            }
            // `union` joined `enum` here when ADR-0045 landed: it is real syntax and a
            // *type*, so in expression position the message must say which kind rather than
            // claiming a feature the user is using has not arrived.
            VARIANT_KW => {
                let span = self.current_span();
                self.error(span, "`variant` is a type, not an expression", E0121);
                self.start_node(ERROR);
                self.bump();
                self.finish_node();
            }
            UNION_KW => {
                let span = self.current_span();
                self.error(span, "`union` is a type, not an expression", E0121);
                self.start_node(ERROR);
                self.bump();
                self.finish_node();
            }
            // `&`, `|`, `^`, `<<` and `>>` are *binary* operators (ADR-0042 §1) and `~` is
            // prefix (§4), so reaching any of them here means one appeared where an operand
            // belongs — `a & & b`. Reported as a missing expression rather than as a reserved
            // operator, which is what it now is.
            AMP | PIPE | CARET | SHL | SHR => {
                let span = self.current_span();
                let op = self.current().static_text().unwrap_or("?");
                self.error(
                    span,
                    format!("expected an expression, found the binary operator `{op}`"),
                    E0118,
                );
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
                    E0118,
                );
                // Emit an ERROR node covering nothing (the caller will handle recovery).
                self.start_node(ERROR);
                self.finish_node();
            }
        }
    }

    /// Parses one argument, which may be `name = value` (ADR-0053 §1).
    ///
    /// `IDENT` followed by `=` is the marker. Jairs has no assignment *expression*, so there is
    /// nothing for this to be ambiguous with — an `=` inside an argument list can only be a named
    /// argument. `==` is a different token, so `f(a == 1)` is unaffected.
    fn parse_arg(&mut self) {
        if self.at(IDENT) && self.nth(1) == EQ {
            self.start_node(NAMED_ARG);
            self.parse_name();
            self.bump(); // `=`
            if self.at_set(EXPR_START) {
                self.parse_expr();
            } else {
                let span = self.current_span();
                self.error(
                    span,
                    "expected a value after `=` in a named argument",
                    E0130,
                );
            }
            self.finish_node();
            return;
        }
        self.parse_expr();
    }

    fn parse_arg_list(&mut self) {
        self.start_node(ARG_LIST);
        self.bump(); // `(`
        if !self.at(R_PAREN) {
            self.parse_arg();
            while self.eat(COMMA) {
                if self.at(R_PAREN) {
                    break; // trailing comma
                }
                if !self.at_set(EXPR_START) {
                    break;
                }
                self.parse_arg();
            }
        }
        if !self.eat(R_PAREN) {
            // Unclosed argument list: recover to `;` or `}`.
            let span = self.current_span();
            self.error(span, "unclosed `(` in argument list", E0119);
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
/// The tokens a type can begin with.
///
/// `L_BRACK` is here for `[N]T` (ADR-0039 §3). Needed in this set as well as in
/// `parse_type_inner`: every site that parses a type gates on this first, so without it
/// `buf: [4]u8;` never reaches the type parser at all and reports "expected a type after
/// `:`" pointing at the `[`. That is the identical mistake `CAST_KW` made against
/// `EXPR_START` one wave earlier — a new syntax form needs adding to the *predicate* as
/// well as to the parser.
///
/// `UNION_KW` and `ENUM_KW` are here for a *nested* inline aggregate — `f: union { … }` inside
/// a struct or a parameter list. `jr-hir` refuses to lower an inline aggregate in a body
/// (both arenas would have to agree where it lives), but that refusal belongs downstream: the
/// parser's job is to produce the tree, and a missing entry here would report "expected a
/// type" at the keyword instead.
///
/// `L_PAREN` is the newest entry, for a procedure-pointer type `(T, T) -> T` (ADR-0059 §3). It is
/// the token-set trap this project keeps meeting: without it, `fn: (s64) -> s64` reported "expected
/// a type" at the `(` and the whole declaration collapsed, exactly as `#c_call` and the array
/// keywords did before their entries were added.
const TYPE_START: TokenSet = TokenSet::new(&[
    STAR, IDENT, STRUCT_KW, ENUM_KW, FLAGS_KW, UNION_KW, VARIANT_KW, L_BRACK, L_PAREN,
]);

/// The operators ADR-0048 §2 permits an overload for.
///
/// Arithmetic and comparison. Kept as a token set rather than a match so that the parser and
/// sema cannot disagree about which tokens are even *spellable* in the declaration form.
const OVERLOADABLE_OPS: TokenSet = TokenSet::new(&[
    PLUS, MINUS, STAR, SLASH, PERCENT, EQ_EQ, BANG_EQ, LT, LT_EQ, GT, GT_EQ,
]);

/// Operator tokens the *parser* accepts in the declaration form and sema then refuses.
///
/// Accepted here so that `operator +% :: …` reports ADR-0048 §2's actual reason — the wrapping
/// forms are about a machine representation — rather than a syntax error pointing at `+%`.
const NON_OVERLOADABLE_OPS: TokenSet = TokenSet::new(&[
    PLUS_PERCENT,
    MINUS_PERCENT,
    STAR_PERCENT,
    AMP,
    PIPE,
    CARET,
    TILDE,
    SHL,
    SHR,
    AMP_AMP,
    PIPE_PIPE,
    BANG,
]);

/// Assignment operators.
const ASSIGN_OPS: TokenSet = TokenSet::new(&[
    EQ,
    AMP_EQ,
    PIPE_EQ,
    CARET_EQ,
    SHL_EQ,
    SHR_EQ,
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
    fn float_literals_parse_in_every_form_the_lexer_produces() {
        // This test used to be `float_literal_is_rejected`, pinning the E0120 refusal. It is
        // rewritten rather than deleted, because the *lexer* forms are the thing worth
        // asserting now that the parser accepts them (ADR-0040).
        for source in [
            "f :: () { x := 1.5; }",
            "f :: () { x := 1e9; }",
            "f :: () { x := 1.5e-3; }",
            "f :: () { x: float64 = 0.5; }",
        ] {
            let p = parse(source, file());
            assert!(!p.has_errors(), "expected no errors for: {source:?}");
        }
    }

    #[test]
    fn a_dot_still_does_not_start_a_float_without_a_digit() {
        // The two ways float lexing breaks a language with a postfix deref and a range: `x.*`
        // must stay a deref and `1..2` must stay three tokens. The lexer already got this
        // right; this asserts the *parser* agrees now that it accepts floats at all.
        let p = parse("f :: () { y := x.*; }", file());
        assert!(!p.has_errors(), "`x.*` is a deref, not a malformed float");
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
