//! The canonical formatter for Jairs source code.
//!
//! # Design
//!
//! The formatter walks the lossless rowan CST, including trivia tokens
//! (whitespace and comments), and emits normalised source into a string
//! builder that tracks the current indentation level.
//!
//! ## Key invariants
//!
//! 1. **Refuse broken input.** If `parse()` reports any diagnostics the
//!    formatter returns `Err(diagnostics)` without touching the source.
//!
//! 2. **Idempotence.** `format(format(x)) == format(x)`.
//!
//! 3. **Comment preservation.** Every comment in the input appears in the
//!    output. The formatter walks the CST including trivia.
//!
//! ## Line wrapping
//!
//! Line wrapping is **not implemented**. `max_width` is accepted and stored
//! but is currently advisory only — it is not used to break long lines.
//!
//! ## Aligned `::` columns
//!
//! The formatter **normalises** alignment: it emits exactly one space before
//! and after `::`, `:=`, and `:`. Alignment is fragile and non-idempotent.

use jr_base::{FileId, TextSize};
use jr_diag::Diagnostics;
use jr_syntax::{SyntaxElement, SyntaxKind, SyntaxKind::*, SyntaxNode, SyntaxToken, parser::parse};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Formatting configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Maximum line width (advisory; line wrapping is not yet implemented).
    ///
    /// The value is stored and will be honoured in a future wave. For now
    /// the formatter emits lines without breaking them regardless of this
    /// setting.
    pub max_width: usize,
    /// Number of spaces per indentation level.
    pub indent_width: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_width: 100,
            indent_width: 4,
        }
    }
}

/// Formats `text` as a Jairs source file.
///
/// Returns the formatted source, or the parse diagnostics if the input does
/// not parse cleanly. Like `rustfmt`, the formatter refuses to reformat code
/// it did not understand.
///
/// # Errors
///
/// Returns `Err(diagnostics)` when `parse()` reports at least one error.
pub fn format(text: &str, file: FileId, config: &Config) -> Result<String, Diagnostics> {
    let parsed = parse(text, file);
    if parsed.has_errors() {
        return Err(parsed.diagnostics().clone());
    }
    let root = parsed.syntax();
    let mut fmt = Formatter::new(config);
    fmt.format_source_file(&root);
    Ok(fmt.finish())
}

/// Convenience wrapper: formats `text` with [`Config::default()`].
///
/// # Errors
///
/// Returns `Err(diagnostics)` when `parse()` reports at least one error.
pub fn format_default(text: &str, file: FileId) -> Result<String, Diagnostics> {
    format(text, file, &Config::default())
}

// ---------------------------------------------------------------------------
// Formatter internals
// ---------------------------------------------------------------------------

/// The internal formatter state.
struct Formatter {
    /// Output buffer.
    out: String,
    /// Current indentation level (number of levels, not spaces).
    indent: usize,
    /// Spaces per indent level.
    indent_width: usize,
}

impl Formatter {
    fn new(config: &Config) -> Self {
        Self {
            out: String::new(),
            indent: 0,
            indent_width: config.indent_width,
        }
    }

    fn finish(mut self) -> String {
        // Trim trailing blank lines.
        while self.out.ends_with("\n\n") {
            self.out.pop();
        }
        // Trim a lone trailing newline if the output is otherwise empty.
        if self.out == "\n" {
            self.out.clear();
        }
        // Ensure exactly one trailing newline for non-empty output.
        if !self.out.ends_with('\n') && !self.out.is_empty() {
            self.out.push('\n');
        }
        self.out
    }

    // ---- output helpers ---------------------------------------------------

    fn indent_str(&self) -> String {
        " ".repeat(self.indent * self.indent_width)
    }

    fn emit(&mut self, s: &str) {
        self.out.push_str(s);
    }

    fn newline(&mut self) {
        self.out.push('\n');
    }

    fn emit_indent(&mut self) {
        let s = self.indent_str();
        self.out.push_str(&s);
    }

    /// Trim trailing spaces (but not newlines) from the output.
    fn trim_trailing_spaces(&mut self) {
        while self.out.ends_with(' ') || self.out.ends_with('\t') {
            self.out.pop();
        }
    }

    /// Ensure the output ends with exactly one blank line (two newlines).
    fn ensure_blank_line(&mut self) {
        self.trim_trailing_spaces();
        if self.out.ends_with("\n\n") {
            return;
        }
        if self.out.ends_with('\n') {
            self.out.push('\n');
        } else if !self.out.is_empty() {
            self.out.push('\n');
            self.out.push('\n');
        }
    }

    /// Ensure the output ends with a newline (but not a blank line).
    fn ensure_newline(&mut self) {
        self.trim_trailing_spaces();
        if !self.out.ends_with('\n') && !self.out.is_empty() {
            self.out.push('\n');
        }
    }

    // ---- comment emission ------------------------------------------------

    /// Emit a comment token, preserving its text exactly.
    fn emit_comment(&mut self, tok: &SyntaxToken) {
        self.emit(tok.text());
    }

    // ---- source file -------------------------------------------------------

    /// Format the root `SOURCE_FILE` node.
    ///
    /// Strategy: walk the children of root (both tokens and nodes) in order,
    /// emitting trivia and items as we encounter them. This avoids the
    /// double-emission problem that arises when separate "leading trivia" and
    /// "item leading comments" functions both walk the same tokens.
    ///
    /// Blank lines between items are preserved from the source (runs of 2+
    /// are collapsed to 1). Items that were adjacent in the source remain
    /// adjacent in the output.
    fn format_source_file(&mut self, root: &SyntaxNode) {
        debug_assert_eq!(root.kind(), SOURCE_FILE);

        if !root.children().any(|n| n.kind() != ERROR) {
            // No items: emit all trivia (comments) preserving blank lines.
            self.emit_file_only_trivia(root);
            return;
        }

        // Whether we have emitted anything yet (to avoid leading blank lines).
        let mut emitted_anything = false;
        // Whether a blank line is pending (seen between items).
        let mut pending_blank = false;

        for child in root.children_with_tokens() {
            match child {
                SyntaxElement::Node(node) => {
                    if node.kind() == ERROR {
                        continue;
                    }
                    // Emit the item.
                    if pending_blank && emitted_anything {
                        // Preserve the blank line from the source.
                        self.ensure_blank_line();
                    } else if emitted_anything {
                        // Adjacent items: just ensure we are on a new line.
                        self.ensure_newline();
                    }
                    self.format_item(&node);
                    emitted_anything = true;
                    pending_blank = false;
                }
                SyntaxElement::Token(tok) => match tok.kind() {
                    // A guard rather than two named arms, so that a comment kind added
                    // later cannot fall into the `_` arm below and be deleted (ADR-0027 §4).
                    k if k.is_comment() => {
                        if pending_blank && emitted_anything {
                            self.ensure_blank_line();
                            pending_blank = false;
                        }
                        self.emit_comment(&tok);
                        self.newline();
                        emitted_anything = true;
                    }
                    WHITESPACE => {
                        let newlines = tok.text().chars().filter(|&c| c == '\n').count();
                        if newlines >= 2 {
                            pending_blank = true;
                        }
                    }
                    _ => {}
                },
            }
        }
    }

    /// Emit all trivia in a file that has no items (only comments).
    fn emit_file_only_trivia(&mut self, root: &SyntaxNode) {
        let mut last_was_comment = false;
        for tok in root.children_with_tokens().filter_map(|e| e.into_token()) {
            match tok.kind() {
                k if k.is_comment() => {
                    self.emit_comment(&tok);
                    self.newline();
                    last_was_comment = true;
                }
                WHITESPACE => {
                    let newlines = tok.text().chars().filter(|&c| c == '\n').count();
                    if newlines >= 2 && last_was_comment {
                        self.newline();
                    }
                    last_was_comment = false;
                }
                _ => {}
            }
        }
    }

    // ---- items -------------------------------------------------------------

    fn format_item(&mut self, node: &SyntaxNode) {
        match node.kind() {
            CONST_DECL => self.format_const_decl(node),
            VAR_DECL => self.format_var_decl(node),
            IMPORT_DECL => self.format_import_decl(node),
            RUN_DECL => self.format_run_decl(node),
            _ => {
                self.emit_indent();
                self.emit(&node.text().to_string());
                self.newline();
            }
        }
    }

    // ---- const decl --------------------------------------------------------

    fn format_const_decl(&mut self, node: &SyntaxNode) {
        self.emit_indent();
        let name = node
            .children()
            .find(|n| n.kind() == NAME)
            .map(|n| n.text().to_string())
            .unwrap_or_default();
        self.emit(&name);
        self.emit(" :: ");

        // The value: PROC, STRUCT_TYPE, or an expression.
        for child in node.children() {
            match child.kind() {
                NAME => {}
                PROC => {
                    self.format_proc(&child);
                    return;
                }
                STRUCT_TYPE => {
                    self.format_struct_type(&child);
                    return;
                }
                _ if is_expr_kind(child.kind()) => {
                    self.format_expr(&child);
                    self.emit_trailing_comment(node);
                    self.emit(";");
                    self.newline();
                    return;
                }
                _ => {}
            }
        }
        self.emit(";");
        self.newline();
    }

    // ---- var decl ----------------------------------------------------------

    fn format_var_decl(&mut self, node: &SyntaxNode) {
        self.emit_indent();
        let name = node
            .children()
            .find(|n| n.kind() == NAME)
            .map(|n| n.text().to_string())
            .unwrap_or_default();
        self.emit(&name);

        let has_colon_eq = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == COLON_EQ);

        if has_colon_eq {
            self.emit(" := ");
            if let Some(expr) = node.children().find(|n| is_expr_kind(n.kind())) {
                self.format_expr(&expr);
            }
        } else {
            self.emit(": ");
            if let Some(ty) = node.children().find(|n| is_type_kind(n.kind())) {
                self.format_type(&ty);
            }
            let has_eq = node
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .any(|t| t.kind() == EQ);
            if has_eq {
                self.emit(" = ");
                if let Some(expr) = node.children().find(|n| is_expr_kind(n.kind())) {
                    self.format_expr(&expr);
                }
            }
        }

        self.emit_trailing_comment(node);
        self.emit(";");
        self.newline();
    }

    // ---- import decl -------------------------------------------------------

    fn format_import_decl(&mut self, node: &SyntaxNode) {
        self.emit_indent();
        self.emit("#import ");
        if let Some(tok) = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == STRING_LITERAL)
        {
            self.emit(tok.text());
        }
        self.emit_trailing_comment(node);
        self.emit(";");
        self.newline();
    }

    // ---- run decl ----------------------------------------------------------

    fn format_run_decl(&mut self, node: &SyntaxNode) {
        self.emit_indent();
        self.emit("#run ");
        if let Some(expr) = node.children().find(|n| is_expr_kind(n.kind())) {
            self.format_expr(&expr);
        }
        self.emit_trailing_comment(node);
        self.emit(";");
        self.newline();
    }

    // ---- proc --------------------------------------------------------------

    fn format_proc(&mut self, node: &SyntaxNode) {
        if let Some(params) = node.children().find(|n| n.kind() == PARAM_LIST) {
            self.format_param_list(&params);
        }
        if let Some(ret) = node.children().find(|n| n.kind() == RET_TYPE) {
            self.emit(" -> ");
            if let Some(ty) = ret.children().find(|n| is_type_kind(n.kind())) {
                self.format_type(&ty);
            }
        }
        if let Some(body) = node.children().find(|n| n.kind() == BLOCK) {
            self.emit(" ");
            self.format_block(&body);
            self.newline();
        } else if let Some(foreign) = node.children().find(|n| n.kind() == FOREIGN_ATTR) {
            self.emit(" ");
            self.format_foreign_attr(&foreign);
            self.emit(";");
            self.newline();
        }
    }

    fn format_param_list(&mut self, node: &SyntaxNode) {
        self.emit("(");
        let params: Vec<SyntaxNode> = node.children().filter(|n| n.kind() == PARAM).collect();
        for (i, param) in params.iter().enumerate() {
            if i > 0 {
                self.emit(", ");
            }
            self.format_param(param);
        }
        self.emit(")");
    }

    fn format_param(&mut self, node: &SyntaxNode) {
        if let Some(name_tok) = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == IDENT)
        {
            self.emit(name_tok.text());
        }
        self.emit(": ");
        if let Some(ty) = node.children().find(|n| is_type_kind(n.kind())) {
            self.format_type(&ty);
        }
    }

    fn format_foreign_attr(&mut self, node: &SyntaxNode) {
        self.emit("#foreign");
        if let Some(lib) = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == IDENT)
        {
            self.emit(" ");
            self.emit(lib.text());
        }
        if let Some(sym) = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == STRING_LITERAL)
        {
            self.emit(" ");
            self.emit(sym.text());
        }
    }

    // ---- struct type -------------------------------------------------------

    fn format_struct_type(&mut self, node: &SyntaxNode) {
        self.emit("struct {");
        if let Some(field_list) = node.children().find(|n| n.kind() == FIELD_LIST) {
            let has_fields = field_list.children().any(|n| n.kind() == FIELD);
            if has_fields {
                self.newline();
                self.indent += 1;
                self.emit_field_list(&field_list);
                self.indent -= 1;
            } else {
                self.newline();
                // An empty field list can still hold comments, and they are the only
                // content there is to keep.
                self.indent += 1;
                self.emit_block_interior_comments(&field_list);
                self.indent -= 1;
            }
        } else {
            self.newline();
        }
        self.emit_indent();
        self.emit("}");
        self.newline();
    }

    /// Emit a struct's fields together with the comments between them.
    ///
    /// Iterates tokens as well as nodes. The previous version walked only `FIELD`
    /// children, so **every comment inside a struct body was silently deleted** — an
    /// ordinary `//` aside as much as a `///` doc comment. It predates ADR-0027 and gate 5
    /// never caught it, because no corpus struct contained a comment; `026-doc-comments.jr`
    /// now does. Data loss in a formatter is the worst kind of bug this project can ship,
    /// because the input is gone by the time anyone notices.
    ///
    /// One pass, with the newline after a field *deferred*: whether a comment is that
    /// field's trailing comment or the next field's leading one is decided by whether a
    /// newline token separates them, which is exactly what the source says. A first
    /// version scanned ahead for a trailing comment instead and emitted every one twice —
    /// once from the scan and once from the iteration.
    fn emit_field_list(&mut self, field_list: &SyntaxNode) {
        // `true` when a field has been emitted and its newline is still owed, so a
        // comment arriving next belongs on the same line.
        let mut owes_newline = false;
        let mut pending_blank = false;
        let mut emitted = false;

        for element in field_list.children_with_tokens() {
            match element {
                SyntaxElement::Node(field) if field.kind() == FIELD => {
                    if owes_newline {
                        self.newline();
                    }
                    if pending_blank && emitted {
                        self.newline();
                    }
                    pending_blank = false;
                    self.emit_indent();
                    self.format_field(&field);
                    self.emit(";");
                    owes_newline = true;
                    emitted = true;
                }
                SyntaxElement::Token(tok) if tok.kind().is_comment() => {
                    if owes_newline {
                        // Same line as the field before it: `x: s64;  // why`.
                        self.emit("  ");
                        self.emit_comment(&tok);
                        self.newline();
                        owes_newline = false;
                    } else {
                        if pending_blank && emitted {
                            self.newline();
                        }
                        self.emit_indent();
                        self.emit_comment(&tok);
                        if tok.kind().is_line_comment() {
                            self.newline();
                        }
                    }
                    pending_blank = false;
                    emitted = true;
                }
                SyntaxElement::Token(tok) if tok.kind() == WHITESPACE => {
                    let newlines = tok.text().chars().filter(|&c| c == '\n').count();
                    if newlines >= 1 && owes_newline {
                        self.newline();
                        owes_newline = false;
                    }
                    if newlines >= 2 {
                        pending_blank = true;
                    }
                }
                SyntaxElement::Node(_) | SyntaxElement::Token(_) => {}
            }
        }

        if owes_newline {
            self.newline();
        }
    }

    fn format_field(&mut self, node: &SyntaxNode) {
        if let Some(name_tok) = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == IDENT)
        {
            self.emit(name_tok.text());
        }
        self.emit(": ");
        if let Some(ty) = node.children().find(|n| is_type_kind(n.kind())) {
            self.format_type(&ty);
        }
    }

    // ---- types -------------------------------------------------------------

    fn format_type(&mut self, node: &SyntaxNode) {
        match node.kind() {
            NAME_TYPE => {
                if let Some(tok) = node
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| t.kind() == IDENT)
                {
                    self.emit(tok.text());
                }
            }
            POINTER_TYPE => {
                self.emit("*");
                if let Some(inner) = node.children().find(|n| is_type_kind(n.kind())) {
                    self.format_type(&inner);
                }
            }
            STRUCT_TYPE => {
                self.format_struct_type(node);
            }
            _ => {
                self.emit(&node.text().to_string());
            }
        }
    }

    // ---- block -------------------------------------------------------------

    fn format_block(&mut self, node: &SyntaxNode) {
        self.emit("{");
        let stmts: Vec<SyntaxNode> = node.children().filter(|n| is_stmt_kind(n.kind())).collect();

        if stmts.is_empty() {
            // Empty block: always use two-line form for consistency with corpus.
            // Check for comments inside.
            let has_comments = node
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .any(|t| t.kind().is_comment());
            self.newline();
            if has_comments {
                self.indent += 1;
                self.emit_block_interior_comments(node);
                self.indent -= 1;
            }
            self.emit_indent();
            self.emit("}");
            return;
        }

        self.newline();
        self.indent += 1;

        let open_brace_end = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == L_BRACE)
            .map(|t| t.text_range().end())
            .unwrap_or_else(|| node.text_range().start());

        let mut prev_end = open_brace_end;

        for stmt in &stmts {
            self.emit_between_stmts(node, prev_end, stmt.text_range().start());
            self.format_stmt(stmt);
            prev_end = stmt.text_range().end();
        }

        // Emit any trailing comments before the closing brace.
        let close_brace_start = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == R_BRACE)
            .map(|t| t.text_range().start())
            .unwrap_or_else(|| node.text_range().end());
        self.emit_between_stmts(node, prev_end, close_brace_start);

        self.indent -= 1;
        self.emit_indent();
        self.emit("}");
    }

    /// Emit comments and blank lines that appear between two positions in a block.
    fn emit_between_stmts(&mut self, block: &SyntaxNode, from: TextSize, to: TextSize) {
        for tok in block.children_with_tokens().filter_map(|e| e.into_token()) {
            let start = tok.text_range().start();
            let end = tok.text_range().end();
            if end <= from || start >= to {
                continue;
            }
            match tok.kind() {
                k if k.is_comment() => {
                    self.emit_indent();
                    self.emit_comment(&tok);
                    // `///` and `//!` run to end of line exactly as `//` does, so they
                    // force the same break. Testing for `LINE_COMMENT` alone would have
                    // put the next statement inside the comment.
                    if k.is_line_comment() {
                        self.newline();
                    }
                }
                WHITESPACE => {
                    let newlines = tok.text().chars().filter(|&c| c == '\n').count();
                    if newlines >= 2 {
                        self.ensure_newline();
                        self.newline();
                    }
                }
                _ => {}
            }
        }
    }

    /// Emit comments inside an empty block.
    fn emit_block_interior_comments(&mut self, block: &SyntaxNode) {
        for tok in block.children_with_tokens().filter_map(|e| e.into_token()) {
            match tok.kind() {
                k if k.is_comment() => {
                    self.emit_indent();
                    self.emit_comment(&tok);
                    // `///` and `//!` run to end of line exactly as `//` does, so they
                    // force the same break. Testing for `LINE_COMMENT` alone would have
                    // put the next statement inside the comment.
                    if k.is_line_comment() {
                        self.newline();
                    }
                }
                _ => {}
            }
        }
    }

    // ---- statements --------------------------------------------------------

    fn format_stmt(&mut self, node: &SyntaxNode) {
        match node.kind() {
            DECL_STMT => {
                if let Some(inner) = node.children().next() {
                    self.format_item(&inner);
                }
            }
            EXPR_STMT => {
                self.emit_indent();
                if let Some(expr) = node.children().find(|n| is_expr_kind(n.kind())) {
                    self.format_expr(&expr);
                }
                self.emit_trailing_comment(node);
                self.emit(";");
                self.newline();
            }
            ASSIGN_STMT => {
                self.emit_indent();
                self.format_assign_stmt(node);
                self.emit_trailing_comment(node);
                self.emit(";");
                self.newline();
            }
            IF_STMT => {
                self.emit_indent();
                self.format_if_stmt(node);
            }
            WHILE_STMT => {
                self.emit_indent();
                self.format_while_stmt(node);
            }
            RETURN_STMT => {
                self.emit_indent();
                self.emit("return");
                if let Some(expr) = node.children().find(|n| is_expr_kind(n.kind())) {
                    self.emit(" ");
                    self.format_expr(&expr);
                }
                self.emit_trailing_comment(node);
                self.emit(";");
                self.newline();
            }
            BREAK_STMT => {
                self.emit_indent();
                self.emit("break");
                self.emit_trailing_comment(node);
                self.emit(";");
                self.newline();
            }
            CONTINUE_STMT => {
                self.emit_indent();
                self.emit("continue");
                self.emit_trailing_comment(node);
                self.emit(";");
                self.newline();
            }
            BLOCK => {
                self.emit_indent();
                self.format_block(node);
                self.newline();
            }
            _ => {
                self.emit_indent();
                self.emit(&node.text().to_string());
                self.newline();
            }
        }
    }

    fn format_assign_stmt(&mut self, node: &SyntaxNode) {
        let mut exprs = node.children().filter(|n| is_expr_kind(n.kind()));
        if let Some(lhs) = exprs.next() {
            self.format_expr(&lhs);
        }
        if let Some(op) = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| is_assign_op(t.kind()))
        {
            self.emit(" ");
            self.emit(op.text());
            self.emit(" ");
        }
        if let Some(rhs) = exprs.next() {
            self.format_expr(&rhs);
        }
    }

    fn format_if_stmt(&mut self, node: &SyntaxNode) {
        self.emit("if ");
        if let Some(cond) = node.children().find(|n| is_expr_kind(n.kind())) {
            self.format_expr(&cond);
        }
        if let Some(body) = node.children().find(|n| n.kind() == BLOCK) {
            self.emit(" ");
            self.format_block(&body);
        } else {
            // Single statement without braces.
            let stmts: Vec<SyntaxNode> =
                node.children().filter(|n| is_stmt_kind(n.kind())).collect();
            if let Some(stmt) = stmts.first() {
                self.emit(" ");
                self.format_single_stmt_inline(stmt);
            }
        }
        if let Some(else_branch) = node.children().find(|n| n.kind() == ELSE_BRANCH) {
            self.emit(" else ");
            if let Some(else_if) = else_branch.children().find(|n| n.kind() == IF_STMT) {
                self.format_if_stmt(&else_if);
                return;
            }
            if let Some(else_block) = else_branch.children().find(|n| n.kind() == BLOCK) {
                self.format_block(&else_block);
            }
        }
        self.newline();
    }

    /// Format a single statement inline (without leading indent).
    fn format_single_stmt_inline(&mut self, node: &SyntaxNode) {
        match node.kind() {
            RETURN_STMT => {
                self.emit("return");
                if let Some(expr) = node.children().find(|n| is_expr_kind(n.kind())) {
                    self.emit(" ");
                    self.format_expr(&expr);
                }
                self.emit(";");
            }
            EXPR_STMT => {
                if let Some(expr) = node.children().find(|n| is_expr_kind(n.kind())) {
                    self.format_expr(&expr);
                }
                self.emit(";");
            }
            ASSIGN_STMT => {
                self.format_assign_stmt(node);
                self.emit(";");
            }
            BREAK_STMT => self.emit("break;"),
            CONTINUE_STMT => self.emit("continue;"),
            _ => {
                self.emit(node.text().to_string().trim());
            }
        }
    }

    fn format_while_stmt(&mut self, node: &SyntaxNode) {
        self.emit("while ");
        if let Some(cond) = node.children().find(|n| is_expr_kind(n.kind())) {
            self.format_expr(&cond);
        }
        if let Some(body) = node.children().find(|n| n.kind() == BLOCK) {
            self.emit(" ");
            self.format_block(&body);
        }
        self.newline();
    }

    // ---- expressions -------------------------------------------------------

    fn format_expr(&mut self, node: &SyntaxNode) {
        match node.kind() {
            LITERAL_EXPR => {
                if let Some(tok) = node
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| {
                        matches!(
                            t.kind(),
                            INT_LITERAL | FLOAT_LITERAL | STRING_LITERAL | TRUE_KW | FALSE_KW
                        )
                    })
                {
                    self.emit(tok.text());
                }
            }
            NAME_EXPR => {
                if let Some(tok) = node
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| t.kind() == IDENT)
                {
                    self.emit(tok.text());
                }
            }
            BINARY_EXPR => {
                let mut exprs = node.children().filter(|n| is_expr_kind(n.kind()));
                if let Some(lhs) = exprs.next() {
                    self.format_expr(&lhs);
                }
                if let Some(op) = node
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| is_binary_op(t.kind()))
                {
                    self.emit(" ");
                    self.emit(op.text());
                    self.emit(" ");
                }
                if let Some(rhs) = exprs.next() {
                    self.format_expr(&rhs);
                }
            }
            UNARY_EXPR => {
                if let Some(op) = node
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| matches!(t.kind(), MINUS | BANG | STAR))
                {
                    self.emit(op.text());
                }
                if let Some(operand) = node.children().find(|n| is_expr_kind(n.kind())) {
                    self.format_expr(&operand);
                }
            }
            PAREN_EXPR => {
                self.emit("(");
                if let Some(inner) = node.children().find(|n| is_expr_kind(n.kind())) {
                    self.format_expr(&inner);
                }
                self.emit(")");
            }
            CALL_EXPR => {
                if let Some(callee) = node.children().find(|n| is_expr_kind(n.kind())) {
                    self.format_expr(&callee);
                }
                if let Some(arg_list) = node.children().find(|n| n.kind() == ARG_LIST) {
                    self.format_arg_list(&arg_list);
                }
            }
            FIELD_EXPR => {
                if let Some(obj) = node.children().find(|n| is_expr_kind(n.kind())) {
                    self.format_expr(&obj);
                }
                self.emit(".");
                if let Some(field) = node
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| t.kind() == IDENT)
                {
                    self.emit(field.text());
                }
            }
            DEREF_EXPR => {
                if let Some(ptr) = node.children().find(|n| is_expr_kind(n.kind())) {
                    self.format_expr(&ptr);
                }
                self.emit(".*");
            }
            UNINIT_EXPR => {
                self.emit("---");
            }
            RUN_EXPR => {
                self.emit("#run ");
                if let Some(inner) = node.children().find(|n| is_expr_kind(n.kind())) {
                    self.format_expr(&inner);
                }
            }
            DIRECTIVE_EXPR => {
                if let Some(dir) = node
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| t.kind() == DIRECTIVE)
                {
                    self.emit(dir.text());
                }
                if let Some(arg) = node
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| t.kind() == STRING_LITERAL)
                {
                    self.emit(" ");
                    self.emit(arg.text());
                }
            }
            _ => {
                self.emit(&node.text().to_string());
            }
        }
    }

    fn format_arg_list(&mut self, node: &SyntaxNode) {
        self.emit("(");
        let args: Vec<SyntaxNode> = node.children().filter(|n| is_expr_kind(n.kind())).collect();
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                self.emit(", ");
            }
            self.format_expr(arg);
        }
        self.emit(")");
    }

    // ---- trailing comment --------------------------------------------------

    /// Emit a trailing inline comment on the same line as the current construct.
    fn emit_trailing_comment(&mut self, node: &SyntaxNode) {
        let last_non_trivia = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| !t.kind().is_trivia())
            .last();

        let Some(last) = last_non_trivia else { return };
        let after = last.text_range().end();

        for tok in node.children_with_tokens().filter_map(|e| e.into_token()) {
            if tok.text_range().start() < after {
                continue;
            }
            match tok.kind() {
                WHITESPACE => {
                    if tok.text().contains('\n') {
                        return;
                    }
                }
                k if k.is_comment() => {
                    self.emit("  ");
                    self.emit_comment(&tok);
                    return;
                }
                _ => return,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Kind classification helpers
// ---------------------------------------------------------------------------

/// Returns `true` if `kind` is an expression node kind.
fn is_expr_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        LITERAL_EXPR
            | NAME_EXPR
            | BINARY_EXPR
            | UNARY_EXPR
            | PAREN_EXPR
            | CALL_EXPR
            | FIELD_EXPR
            | DEREF_EXPR
            | UNINIT_EXPR
            | RUN_EXPR
            | DIRECTIVE_EXPR
    )
}

/// Returns `true` if `kind` is a type node kind.
fn is_type_kind(kind: SyntaxKind) -> bool {
    matches!(kind, NAME_TYPE | POINTER_TYPE | STRUCT_TYPE)
}

/// Returns `true` if `kind` is a statement node kind.
fn is_stmt_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        DECL_STMT
            | EXPR_STMT
            | ASSIGN_STMT
            | IF_STMT
            | WHILE_STMT
            | RETURN_STMT
            | BREAK_STMT
            | CONTINUE_STMT
            | BLOCK
    )
}

/// Returns `true` if `kind` is a binary operator token.
fn is_binary_op(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        PIPE_PIPE
            | AMP_AMP
            | EQ_EQ
            | BANG_EQ
            | LT
            | LT_EQ
            | GT
            | GT_EQ
            | PLUS
            | MINUS
            | PLUS_PERCENT
            | MINUS_PERCENT
            | STAR
            | SLASH
            | PERCENT
            | STAR_PERCENT
    )
}

/// Returns `true` if `kind` is an assignment operator token.
fn is_assign_op(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        EQ | PLUS_EQ
            | MINUS_EQ
            | STAR_EQ
            | SLASH_EQ
            | PERCENT_EQ
            | PLUS_PERCENT_EQ
            | MINUS_PERCENT_EQ
            | STAR_PERCENT_EQ
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use jr_base::FileId;

    fn file() -> FileId {
        FileId::from_usize(0)
    }

    fn fmt(src: &str) -> String {
        format(src, file(), &Config::default()).expect("format failed")
    }

    fn assert_idempotent(src: &str) {
        let once = fmt(src);
        let twice = fmt(&once);
        assert_eq!(
            once, twice,
            "formatter is not idempotent!\nFirst pass:\n{once}\nSecond pass:\n{twice}"
        );
    }

    fn assert_parses(src: &str) {
        use jr_syntax::parser::parse;
        let p = parse(src, file());
        assert!(
            !p.has_errors(),
            "formatted output does not parse cleanly:\n{src}\nErrors: {:?}",
            p.diagnostics()
        );
    }

    // ---- doc comments (ADR-0027 §4) ----------------------------------------
    //
    // Every comment site in this file matched `LINE_COMMENT | BLOCK_COMMENT` and ended
    // in `_ => {}`. Adding a kind without converting them would have deleted every doc
    // comment in a file, and `jr fmt --check` over the corpus would still have passed,
    // because no corpus file had one. Hence these.

    #[test]
    fn a_doc_comment_on_a_declaration_survives() {
        let src = "/// Adds two numbers.\nadd :: (a: s64, b: s64) -> s64 {\n    return a + b;\n}\n";
        let out = fmt(src);
        assert!(
            out.contains("/// Adds two numbers."),
            "doc comment was dropped:\n{out}"
        );
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn a_module_doc_comment_survives() {
        let src = "//! The Basic module.\n\nX :: 1;\n";
        let out = fmt(src);
        assert!(out.contains("//! The Basic module."), "dropped:\n{out}");
        assert_idempotent(src);
    }

    #[test]
    fn doc_comments_survive_everywhere_a_comment_can_appear() {
        // One per formatter site: file level, inside a body between statements, inside
        // an otherwise empty body, and trailing on a line.
        let src = concat!(
            "//! module\n",
            "/// on an item\n",
            "X :: 1;\n",
            "/// on a proc\n",
            "f :: () {\n",
            "    /// between statements\n",
            "    y := 1;\n",
            "}\n",
            "/// on an empty proc\n",
            "g :: () {\n",
            "    /// inside an empty body\n",
            "}\n",
        );
        let out = fmt(src);
        for expected in [
            "//! module",
            "/// on an item",
            "/// on a proc",
            "/// between statements",
            "/// on an empty proc",
            "/// inside an empty body",
        ] {
            assert!(out.contains(expected), "{expected:?} was dropped:\n{out}");
        }
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn a_doc_comment_forces_a_break_like_a_line_comment() {
        // `is_line_comment` rather than `== LINE_COMMENT`: if a `///` did not force a
        // newline, the statement after it would be commented out — a formatter that
        // silently changes what the program means.
        let out = fmt("f :: () {\n    /// doc\n    y := 1;\n}\n");
        let doc_line = out
            .lines()
            .position(|l| l.contains("/// doc"))
            .expect("doc comment kept");
        let stmt_line = out
            .lines()
            .position(|l| l.contains("y := 1"))
            .expect("statement kept");
        assert!(
            stmt_line > doc_line,
            "statement ended up on the doc comment's line:\n{out}"
        );
        assert_parses(&out);
    }

    // ---- comments inside a struct body ------------------------------------
    //
    // A pre-existing data-loss bug, not one ADR-0027 introduced: `format_struct_type`
    // walked only `FIELD` children, so every comment between fields was dropped. Gate 5
    // passed throughout because no corpus struct contained one.

    #[test]
    fn a_comment_between_fields_survives() {
        let src = "Point :: struct {\n    // an aside\n    x: s64;\n}\n";
        let out = fmt(src);
        assert!(out.contains("// an aside"), "comment was dropped:\n{out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn a_field_doc_comment_survives() {
        let src = "Point :: struct {\n    /// The horizontal coordinate.\n    x: s64;\n}\n";
        let out = fmt(src);
        assert!(
            out.contains("/// The horizontal coordinate."),
            "field doc comment was dropped:\n{out}"
        );
        assert_idempotent(src);
    }

    #[test]
    fn a_fields_trailing_comment_stays_on_its_line() {
        let src = "Point :: struct {\n    x: s64; // why\n    y: s64;\n}\n";
        let out = fmt(src);
        let line = out
            .lines()
            .find(|l| l.contains("x: s64"))
            .expect("the field survives");
        assert!(
            line.contains("// why"),
            "the trailing comment left its line:\n{out}"
        );
        // And exactly once: a first fix emitted it twice, from a look-ahead scan and from
        // the iteration both.
        assert_eq!(out.matches("// why").count(), 1, "emitted twice:\n{out}");
        assert_idempotent(src);
    }

    #[test]
    fn a_blank_line_between_fields_survives() {
        let src = "Point :: struct {\n    x: s64;\n\n    y: s64;\n}\n";
        let out = fmt(src);
        assert!(out.contains("x: s64;\n\n"), "blank line lost:\n{out}");
        assert_idempotent(src);
    }

    #[test]
    fn a_comment_in_an_empty_struct_survives() {
        let src = "Empty :: struct {\n    // nothing yet\n}\n";
        let out = fmt(src);
        assert!(out.contains("// nothing yet"), "dropped:\n{out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn four_slashes_stay_an_ordinary_comment() {
        let src = "//// ----------\nX :: 1;\n";
        let out = fmt(src);
        assert!(out.contains("//// ----------"), "dropped:\n{out}");
        assert_idempotent(src);
    }

    // ---- basic sanity -------------------------------------------------------

    #[test]
    fn empty_input() {
        let out = fmt("");
        assert_eq!(out, "");
        assert_idempotent("");
    }

    #[test]
    fn whitespace_only() {
        let out = fmt("   \n\n   ");
        assert_eq!(out, "");
        assert_idempotent("   \n\n   ");
    }

    // ---- refuse broken input -----------------------------------------------

    #[test]
    fn broken_input_returns_err() {
        let result = format("broken :: ;", file(), &Config::default());
        assert!(result.is_err(), "expected Err for broken input");
    }

    #[test]
    fn missing_semicolon_returns_err() {
        let result = format("MAX :: 42", file(), &Config::default());
        assert!(result.is_err());
    }

    // ---- import ------------------------------------------------------------

    #[test]
    fn import_decl() {
        let src = "#import \"Basic\";\n";
        let out = fmt(src);
        assert_eq!(out, "#import \"Basic\";\n");
        assert_idempotent(src);
        assert_parses(&out);
    }

    // ---- constants ---------------------------------------------------------

    #[test]
    fn integer_constant() {
        let src = "MAX :: 42;\n";
        let out = fmt(src);
        assert_eq!(out, "MAX :: 42;\n");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn string_constant() {
        let src = "MSG :: \"hello\";\n";
        let out = fmt(src);
        assert_eq!(out, "MSG :: \"hello\";\n");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn bool_constant() {
        let src = "DEBUG :: false;\n";
        let out = fmt(src);
        assert_eq!(out, "DEBUG :: false;\n");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn aligned_constants_normalised() {
        // Aligned `::` columns are normalised to single space.
        let src = "GREETING     :: \"hello\";\nDEBUG        :: false;\n";
        let out = fmt(src);
        assert!(out.contains("GREETING :: \"hello\";"), "got: {out}");
        assert!(out.contains("DEBUG :: false;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    // ---- var decls ---------------------------------------------------------

    #[test]
    fn inferred_var_decl() {
        let src = "main :: () {\n    x := 1;\n}\n";
        let out = fmt(src);
        assert!(out.contains("x := 1;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn typed_var_decl_no_init() {
        let src = "main :: () {\n    x: s64;\n}\n";
        let out = fmt(src);
        assert!(out.contains("x: s64;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn typed_var_decl_with_init() {
        let src = "main :: () {\n    x: s64 = 1;\n}\n";
        let out = fmt(src);
        assert!(out.contains("x: s64 = 1;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn uninit_var_decl() {
        let src = "main :: () {\n    x: s64 = ---;\n}\n";
        let out = fmt(src);
        assert!(out.contains("x: s64 = ---;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    // ---- procedures --------------------------------------------------------

    #[test]
    fn empty_proc() {
        let src = "noop :: () {\n}\n";
        let out = fmt(src);
        assert!(out.contains("noop :: () {"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn proc_with_params_and_return() {
        let src = "add :: (a: s64, b: s64) -> s64 {\n    return a + b;\n}\n";
        let out = fmt(src);
        assert!(
            out.contains("add :: (a: s64, b: s64) -> s64 {"),
            "got: {out}"
        );
        assert!(out.contains("return a + b;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn foreign_proc() {
        let src = "write :: (fd: s64, buf: *u8, count: s64) -> s64 #foreign libc \"write\";\n";
        let out = fmt(src);
        assert!(out.contains("#foreign libc \"write\""), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    // ---- structs -----------------------------------------------------------

    #[test]
    fn struct_decl() {
        let src = "Point :: struct {\n    x: s64;\n    y: s64;\n}\n";
        let out = fmt(src);
        assert!(out.contains("Point :: struct {"), "got: {out}");
        assert!(out.contains("    x: s64;"), "got: {out}");
        assert!(out.contains("    y: s64;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn empty_struct() {
        let src = "Marker :: struct {\n}\n";
        let out = fmt(src);
        assert!(out.contains("Marker :: struct {"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    // ---- statements --------------------------------------------------------

    #[test]
    fn if_stmt() {
        let src = "f :: () {\n    if x > 0 {\n        return x;\n    }\n}\n";
        let out = fmt(src);
        assert!(out.contains("if x > 0 {"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn if_else_stmt() {
        let src = "f :: () {\n    if x > 0 {\n        return 1;\n    } else {\n        return 0;\n    }\n}\n";
        let out = fmt(src);
        assert!(out.contains("} else {"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn if_else_if_stmt() {
        let src = "f :: () {\n    if a {\n        return 1;\n    } else if b {\n        return 2;\n    } else {\n        return 3;\n    }\n}\n";
        let out = fmt(src);
        assert!(out.contains("} else if b {"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn while_stmt() {
        let src = "f :: () {\n    while i < 10 {\n        i = i + 1;\n    }\n}\n";
        let out = fmt(src);
        assert!(out.contains("while i < 10 {"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn return_stmt() {
        let src = "f :: () -> s64 {\n    return 42;\n}\n";
        let out = fmt(src);
        assert!(out.contains("return 42;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn return_void() {
        let src = "f :: () {\n    return;\n}\n";
        let out = fmt(src);
        assert!(out.contains("return;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn break_continue() {
        let src = "f :: () {\n    while true {\n        break;\n        continue;\n    }\n}\n";
        let out = fmt(src);
        assert!(out.contains("break;"), "got: {out}");
        assert!(out.contains("continue;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn assignment_stmt() {
        let src = "f :: () {\n    a = 1;\n}\n";
        let out = fmt(src);
        assert!(out.contains("a = 1;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn compound_assignment() {
        let src = "f :: () {\n    a += 1;\n    a -= 2;\n    a *= 3;\n    a /= 4;\n    a %= 5;\n}\n";
        let out = fmt(src);
        assert!(out.contains("a += 1;"), "got: {out}");
        assert!(out.contains("a -= 2;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn wrapping_compound_assignment() {
        let src = "f :: () {\n    a +%= 1;\n    a -%= 2;\n    a *%= 3;\n}\n";
        let out = fmt(src);
        assert!(out.contains("a +%= 1;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    // ---- expressions -------------------------------------------------------

    #[test]
    fn binary_expr_spaces() {
        let src = "f :: () {\n    x := a + b;\n}\n";
        let out = fmt(src);
        assert!(out.contains("a + b"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn unary_negation() {
        let src = "f :: () {\n    x := -a;\n}\n";
        let out = fmt(src);
        assert!(out.contains("-a"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn paren_expr() {
        let src = "f :: () {\n    x := (a + b) * 2;\n}\n";
        let out = fmt(src);
        assert!(out.contains("(a + b) * 2"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn call_expr() {
        let src = "f :: () {\n    x := foo(1, 2);\n}\n";
        let out = fmt(src);
        assert!(out.contains("foo(1, 2)"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn field_expr() {
        let src = "f :: () {\n    x := a.b;\n}\n";
        let out = fmt(src);
        assert!(out.contains("a.b"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn deref_expr() {
        let src = "f :: () {\n    x := p.*;\n}\n";
        let out = fmt(src);
        assert!(out.contains("p.*"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn pointer_type() {
        let src = "f :: () {\n    p: *s64 = *a;\n}\n";
        let out = fmt(src);
        assert!(out.contains("p: *s64 = *a;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn run_expr() {
        let src = "COMPUTED :: #run add(2, 3);\n";
        let out = fmt(src);
        assert!(out.contains("#run add(2, 3)"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    // ---- comments ----------------------------------------------------------

    #[test]
    fn line_comment_before_decl() {
        let src = "// A comment.\nMAX :: 42;\n";
        let out = fmt(src);
        assert!(out.contains("// A comment."), "got: {out}");
        assert!(out.contains("MAX :: 42;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn line_comment_inside_block() {
        let src = "f :: () {\n    // comment\n    x := 1;\n}\n";
        let out = fmt(src);
        assert!(out.contains("// comment"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn blank_line_between_top_level() {
        let src = "MAX :: 42;\n\nMIN :: 0;\n";
        let out = fmt(src);
        assert!(out.contains("MAX :: 42;\n\nMIN :: 0;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn multiple_blank_lines_collapsed() {
        let src = "MAX :: 42;\n\n\n\nMIN :: 0;\n";
        let out = fmt(src);
        // Should have exactly one blank line between them.
        assert!(out.contains("MAX :: 42;\n\nMIN :: 0;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    // ---- block scope -------------------------------------------------------

    #[test]
    fn nested_blocks() {
        let src = "main :: () {\n    outer := 1;\n    {\n        inner := 2;\n    }\n}\n";
        let out = fmt(src);
        assert!(out.contains("outer := 1;"), "got: {out}");
        assert!(out.contains("inner := 2;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    // ---- corpus round-trip tests -------------------------------------------

    /// For every valid corpus file: format it and assert idempotence.
    /// This is the primary correctness test.
    #[test]
    fn corpus_idempotence() {
        let corpus_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/corpus/valid");
        let mut entries: Vec<_> = std::fs::read_dir(corpus_dir)
            .expect("corpus dir not found")
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "jr"))
            .collect();
        entries.sort_by_key(|e| e.path());

        assert!(!entries.is_empty(), "no corpus files found");

        for entry in entries {
            let path = entry.path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let src = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {name}: {e}"));

            let once = format(&src, file(), &Config::default())
                .unwrap_or_else(|e| panic!("{name}: format failed: {e:?}"));
            let twice = format(&once, file(), &Config::default())
                .unwrap_or_else(|e| panic!("{name}: second format failed: {e:?}"));

            assert_eq!(
                once, twice,
                "{name}: formatter is not idempotent!\nFirst:\n{once}\nSecond:\n{twice}"
            );
        }
    }

    /// For every valid corpus file: `parse(format(x))` must produce zero
    /// diagnostics. This is the single most important test.
    #[test]
    fn corpus_round_trip_parses() {
        use jr_syntax::parser::parse;

        let corpus_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/corpus/valid");
        let mut entries: Vec<_> = std::fs::read_dir(corpus_dir)
            .expect("corpus dir not found")
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "jr"))
            .collect();
        entries.sort_by_key(|e| e.path());

        for entry in entries {
            let path = entry.path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let src = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {name}: {e}"));

            let formatted = format(&src, file(), &Config::default())
                .unwrap_or_else(|e| panic!("{name}: format failed: {e:?}"));

            let p = parse(&formatted, file());
            assert!(
                !p.has_errors(),
                "{name}: formatted output does not parse cleanly:\n{formatted}\nErrors: {:?}",
                p.diagnostics()
            );
        }
    }

    /// For every invalid corpus file: format must return Err.
    #[test]
    fn invalid_corpus_returns_err() {
        let corpus_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/corpus/invalid");
        if !std::path::Path::new(corpus_dir).exists() {
            return;
        }
        let entries: Vec<_> = std::fs::read_dir(corpus_dir)
            .expect("invalid corpus dir not found")
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "jr"))
            .collect();

        for entry in entries {
            let path = entry.path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let src = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {name}: {e}"));

            let result = format(&src, file(), &Config::default());
            assert!(
                result.is_err(),
                "{name}: expected Err for invalid input, got Ok"
            );
        }
    }

    /// Assert that each valid corpus file is already in canonical form.
    ///
    /// The corpus was canonicalised by running `jr fmt` over it once the
    /// formatter's normalisation rules were reviewed and accepted (notably:
    /// aligned `::` columns are collapsed to a single space, because alignment
    /// is not idempotent under renaming and causes diff churn).
    ///
    /// Keeping this enforced means the specification examples and the
    /// formatter's output can never silently drift apart.
    #[test]
    fn corpus_already_canonical() {
        let corpus_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/corpus/valid");
        let mut entries: Vec<_> = std::fs::read_dir(corpus_dir)
            .expect("corpus dir not found")
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "jr"))
            .collect();
        entries.sort_by_key(|e| e.path());

        let mut failures = Vec::new();
        for entry in entries {
            let path = entry.path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let src = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {name}: {e}"));

            let formatted = format(&src, file(), &Config::default())
                .unwrap_or_else(|e| panic!("{name}: format failed: {e:?}"));

            if formatted != src {
                failures.push(name);
            }
        }

        assert!(
            failures.is_empty(),
            "These corpus files are not in canonical form: {failures:?}"
        );
    }

    /// Show diffs between corpus files and formatter output (for the report).
    #[test]
    #[ignore = "diagnostic test for report generation"]
    fn show_corpus_diffs() {
        let corpus_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/corpus/valid");
        let mut entries: Vec<_> = std::fs::read_dir(corpus_dir)
            .expect("corpus dir not found")
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "jr"))
            .collect();
        entries.sort_by_key(|e| e.path());

        for entry in entries {
            let path = entry.path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let src = std::fs::read_to_string(&path).unwrap();
            let formatted = format(&src, file(), &Config::default()).unwrap();
            if formatted != src {
                println!("=== {} ===", name);
                let src_lines: Vec<&str> = src.lines().collect();
                let fmt_lines: Vec<&str> = formatted.lines().collect();
                let max = src_lines.len().max(fmt_lines.len());
                for i in 0..max {
                    let s = src_lines.get(i).copied().unwrap_or("<missing>");
                    let f = fmt_lines.get(i).copied().unwrap_or("<missing>");
                    if s != f {
                        println!("  -{}", s);
                        println!("  +{}", f);
                    }
                }
                println!();
            }
        }
    }

    // ---- stack overflow safety (adversarial input) -------------------------

    /// The formatter must not overflow the stack on deeply nested input.
    ///
    /// Uses an explicitly spawned thread with a 1 MiB stack, matching the
    /// approach in `crates/jr-syntax/tests/robustness.rs`.
    #[test]
    fn deeply_nested_does_not_overflow() {
        let depth = 512usize;
        let mut src = String::from("f :: () {\n    x := ");
        for _ in 0..depth {
            src.push('(');
        }
        src.push('1');
        for _ in 0..depth {
            src.push(')');
        }
        src.push_str(";\n}\n");

        let result = std::thread::Builder::new()
            .stack_size(1024 * 1024) // 1 MiB
            .spawn(move || format(&src, file(), &Config::default()))
            .expect("thread spawn failed")
            .join()
            .expect("thread panicked");

        // The parser will reject deeply nested input (depth guard), so we
        // accept either Ok or Err — the important thing is no panic/overflow.
        let _ = result;
    }
}
