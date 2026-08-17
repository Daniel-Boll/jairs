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
            OPERATOR_DECL => self.format_operator_decl(node),
            VAR_DECL => self.format_var_decl(node),
            IMPORT_DECL => self.format_import_decl(node),
            RUN_DECL => self.format_run_decl(node),
            // A visibility marker (ADR-0054 §1). **It survived without this arm**, through the
            // raw-text fallback below — which round-trips and stops *canonicalising*, the trap
            // ADR-0048 recorded when `operator   +   ::` passed through unchanged. An explicit arm
            // emits the directive alone on its line whatever the node contains.
            SCOPE_DECL => {
                self.emit_indent();
                if let Some(directive) = node
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| t.kind() == DIRECTIVE)
                {
                    self.emit(directive.text());
                }
                self.newline();
            }
            _ => {
                self.emit_indent();
                self.emit(&node.text().to_string());
                self.newline();
            }
        }
    }

    // ---- const decl --------------------------------------------------------

    /// Formats `operator + :: (…) -> T { … }` (ADR-0048 §1).
    ///
    /// Its own function rather than sharing `format_const_decl`, which reads a `NAME` child that
    /// an operator declaration does not have: sharing would have emitted `` :: `` with an empty
    /// name and dropped the operator. Sixth consecutive wave for that trap, so it is written
    /// first rather than discovered by `jr fmt --check`.
    fn format_operator_decl(&mut self, node: &SyntaxNode) {
        self.emit_indent();
        self.emit("operator ");
        // The operator is the one token that is neither the keyword nor the `::`. Found by
        // exclusion rather than by position, because a malformed declaration may be missing it
        // and a positional read would then emit the `::` as the operator.
        if let Some(op) = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| !t.kind().is_trivia() && t.kind() != OPERATOR_KW && t.kind() != COLON_COLON)
        {
            self.emit(op.text());
        }
        self.emit(" :: ");
        if let Some(proc) = node.children().find(|n| n.kind() == PROC) {
            self.format_proc(&proc);
        } else {
            self.emit(";");
            self.newline();
        }
    }

    fn format_const_decl(&mut self, node: &SyntaxNode) {
        self.emit_indent();
        let name = node
            .children()
            .find(|n| n.kind() == NAME)
            .map(|n| n.text().to_string())
            .unwrap_or_default();
        self.emit(&name);
        self.emit(" :: ");

        // The value: PROC, STRUCT_TYPE, ENUM_TYPE, or an expression.
        //
        // The `_ => {}` at the end of this match is why `ENUM_TYPE` had to be added *here*
        // as well as to `is_type_kind`: an unmatched value kind falls through to the bare
        // `;` below, so `Colour :: enum { … }` formatted to `Colour :: ;`. The same silent
        // deletion `cast` suffered in ADR-0037's wave, in a second dispatch site.
        for child in node.children() {
            match child.kind() {
                NAME => {}
                PROC => {
                    self.format_proc(&child);
                    return;
                }
                STRUCT_TYPE | UNION_TYPE | VARIANT_TYPE => {
                    self.format_struct_type(&child);
                    return;
                }
                ENUM_TYPE => {
                    self.format_enum_type(&child);
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
        self.emit_using(node);
        // A destructuring declaration — `q, ok := f();` (ADR-0052 §2). The parser reuses `VAR_DECL`
        // for it, so the *presence* of a target list is what distinguishes the two forms here, as
        // it is in lowering.
        if let Some(list) = node.children().find(|n| n.kind() == TARGET_LIST) {
            self.format_target_list(&list);
            self.emit(" := ");
            if let Some(expr) = node.children().find(|n| is_expr_kind(n.kind())) {
                self.format_expr(&expr);
            }
            self.emit_trailing_comment(node);
            self.emit(";");
            self.newline();
            return;
        }
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
            // `-> (s64, bool)` (ADR-0052 §1). A `RESULT_LIST` is **not** a type node, so
            // `is_type_kind` does not find it — and without this arm the formatter emitted
            // `-> ` with nothing after it, deleting the whole result list. Fifth consecutive
            // wave for this trap, and the second where no *kind predicate* was missing: the
            // list is its own node and the emitter simply had no case for it.
            if let Some(list) = ret.children().find(|n| n.kind() == RESULT_LIST) {
                self.format_result_list(&list);
            } else if let Some(ty) = ret.children().find(|n| is_type_kind(n.kind())) {
                self.format_type(&ty);
            }
        }
        // `#c_call` (ADR-0057 §3), before the body. Deleting it changed what the program *means* — a
        // `#c_call` procedure with the attribute dropped would silently start taking a context and
        // its callers would be recompiled against a different ABI. Seventh consecutive wave for this
        // trap, and the worst kind: not lost formatting but a lost calling convention.
        // Emitted in **source order**, not in a fixed order of the two kinds. The parser accepts
        // `#c_call #no_abc` and `#no_abc #c_call` (either order is legal, because an ordering rule
        // would be one no reader could guess), so a formatter that emitted `#c_call` first would
        // *reorder* one of them. Reordering is not lost source, but it means `jr fmt` is not
        // idempotent on the input it did not choose — and gate 5 would fail on a corpus file
        // written the other way round.
        for attr in node.children() {
            match attr.kind() {
                C_CALL_ATTR => self.emit(" #c_call"),
                // `#no_abc` (ADR-0058 §3). Eighth consecutive wave for this trap, and this one is
                // the *safe* direction to lose: dropping it restores a bounds check, so the program
                // gets slower rather than unsound. `#c_call` above is the opposite — dropping that
                // silently changes a calling convention.
                NO_ABC_ATTR => self.emit(" #no_abc"),
                // `#expand` (ADR-0090 §1). The trap again, and this one is the *unsound* direction like
                // `#c_call`: dropping it turns a macro into an ordinary procedure, so a body meant to be
                // spliced into the caller's scope — reading the caller's locals — becomes a call that
                // cannot see them. Caught by gate 5 on this wave's own corpus file, which is what that
                // gate is for.
                // `@note` — metadata a metaprogram reads (ADR-0098 §1). Emitted in **source order** with the
                // directives, since the loop takes them in any order and reordering would make `jr fmt`
                // non-idempotent on input it did not write. Dropping a note deletes a metaprogram's *input*,
                // so a build script that collects `@X` would silently find nothing — caught by gate 5 on
                // this wave's own corpus file, the trap this file has now hit in most of the last dozen waves.
                NOTE => {
                    self.emit(" @");
                    if let Some(name) = attr
                        .children_with_tokens()
                        .filter_map(|e| e.into_token())
                        .find(|t| t.kind() == IDENT)
                    {
                        self.emit(name.text());
                    }
                    if let Some(payload) = attr
                        .children_with_tokens()
                        .filter_map(|e| e.into_token())
                        .find(|t| t.kind() == STRING_LITERAL)
                    {
                        self.emit(" ");
                        self.emit(payload.text());
                    }
                }
                EXPAND_ATTR => self.emit(" #expand"),
                // `#modify { … }` carries a **block**, so it is emitted with its body rather than as a bare
                // word (ADR-0093 §1). Dropping it would delete a compile-time predicate — the program would
                // then accept instantiations the author rejected, which is the *unsound* direction, like
                // `#c_call` and `#expand` before it.
                MODIFY_ATTR => {
                    self.emit(" #modify ");
                    if let Some(block) = attr.children().find(|n| n.kind() == BLOCK) {
                        self.format_block(&block);
                    }
                }
                _ => {}
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
        self.emit_using(node);
        // `$N: s64` — a comptime-value parameter (ADR-0087 §1). The `$` precedes the name and dropping
        // it would silently turn a comptime parameter into an ordinary one — a change in what the
        // program means, the lossy-CST failure this file guards against, so a round-trip corpus file
        // pins it. It is a `DOLLAR` token child of `PARAM`, not part of the type (that would be a `$T`).
        if node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == DOLLAR)
        {
            self.emit("$");
        }
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
        // `= 10` — a default value (ADR-0053 §2). **Sixth consecutive wave** for this trap: without
        // this the formatter deleted every default, turning a callable `f(1)` into an arity error.
        // That is a change in what the program *means*, like ADR-0050's deleted `using` and
        // ADR-0052's truncated `return a, b;`.
        if let Some(default) = node.children().find(|n| is_expr_kind(n.kind())) {
            self.emit(" = ");
            self.format_expr(&default);
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

    /// Formats `struct { … }` **and** `union { … }`.
    ///
    /// The keyword comes from the *node kind*, not from a literal. Emitting `"struct {"`
    /// unconditionally would rewrite `union` to `struct` — a different working program with
    /// overlapping fields turned into non-overlapping ones, which is precisely the mistake
    /// ADR-0043 caught when a literal `"enum"` rewrote an `enum_flags` (§7's standing trap).
    fn format_struct_type(&mut self, node: &SyntaxNode) {
        // A **match on the kind**, not a two-way `if`: the third form arrived (ADR-0068) and an
        // `else` branch meaning "struct" turned every `variant` into a `struct` — which is exactly the
        // mistake this function's own docs warn about for `enum_flags`, made again one form later.
        self.emit(match node.kind() {
            UNION_TYPE => "union",
            VARIANT_TYPE => "variant",
            _ => "struct",
        });
        // `struct($T) { … }` — the type parameters, when present, sit between the keyword and the brace
        // (ADR-0085 §3). Dropping them was a silent data-loss bug that turned a parameterised struct
        // into an ordinary one, so a `struct($T)` corpus file failed to round-trip.
        if let Some(params) = node.children().find(|n| n.kind() == STRUCT_TYPE_PARAMS) {
            self.format_struct_type_params(&params);
        }
        self.emit(" {");
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
        self.emit_item_list(field_list, FIELD, Self::format_field);
    }

    /// Emit an enum's members together with the comments between them.
    ///
    /// Shares [`Formatter::emit_item_list`] with the struct case rather than repeating it,
    /// because the comment handling there was written to fix real data loss and a second copy
    /// would be a second chance to lose a comment.
    fn emit_member_list(&mut self, member_list: &SyntaxNode) {
        self.emit_item_list(member_list, MEMBER, Self::format_member);
    }

    /// One member: `RED` or `NOT_FOUND :: 404`.
    ///
    /// The `;` is emitted by the caller, exactly as it is for a field.
    fn format_member(&mut self, node: &SyntaxNode) {
        if let Some(name_tok) = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == IDENT)
        {
            self.emit(name_tok.text());
        }
        // An auto-numbered member has no value node, and must not gain one: printing the
        // number the compiler computed would make `jr fmt` change what the source says.
        if let Some(value) = node.children().find(|n| is_expr_kind(n.kind())) {
            self.emit(" :: ");
            self.format_expr(&value);
        }
    }

    /// The shared body of [`Formatter::emit_field_list`] and
    /// [`Formatter::emit_member_list`].
    fn emit_item_list(
        &mut self,
        list: &SyntaxNode,
        item_kind: SyntaxKind,
        format_one: fn(&mut Self, &SyntaxNode),
    ) {
        // `true` when a field has been emitted and its newline is still owed, so a
        // comment arriving next belongs on the same line.
        let mut owes_newline = false;
        let mut pending_blank = false;
        let mut emitted = false;

        for element in list.children_with_tokens() {
            match element {
                SyntaxElement::Node(field) if field.kind() == item_kind => {
                    if owes_newline {
                        self.newline();
                    }
                    if pending_blank && emitted {
                        self.newline();
                    }
                    pending_blank = false;
                    self.emit_indent();
                    format_one(self, &field);
                    self.emit(";");
                    owes_newline = true;
                    emitted = true;
                }
                SyntaxElement::Token(tok) if tok.kind().is_comment() => {
                    if owes_newline {
                        // Same line as the item before it: `x: s64;  // why`.
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

    /// `enum { RED; GREEN :: 5; }` — mirroring [`Formatter::format_struct_type`].
    fn format_enum_type(&mut self, node: &SyntaxNode) {
        // The keyword comes from the *token*, not from a literal: emitting `"enum {"`
        // unconditionally rewrote `enum_flags` to `enum`, which changes the numbering rule and
        // which operators apply (ADR-0043). Worse than deleting the construct — a deletion
        // fails to parse, while this produced a different working program.
        let keyword = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| matches!(t.kind(), ENUM_KW | FLAGS_KW))
            .map_or("enum", |t| {
                if t.kind() == FLAGS_KW {
                    "enum_flags"
                } else {
                    "enum"
                }
            });
        self.emit(keyword);
        self.emit(" {");
        if let Some(member_list) = node.children().find(|n| n.kind() == MEMBER_LIST) {
            let has_members = member_list.children().any(|n| n.kind() == MEMBER);
            if has_members {
                self.newline();
                self.indent += 1;
                self.emit_member_list(&member_list);
                self.indent -= 1;
            } else {
                self.newline();
                self.indent += 1;
                self.emit_block_interior_comments(&member_list);
                self.indent -= 1;
            }
        } else {
            self.newline();
        }
        self.emit_indent();
        self.emit("}");
        self.newline();
    }

    /// Emits `using ` when the node carries the keyword (ADR-0050 §1).
    ///
    /// Shared by the field, parameter and local emitters because dropping it is not a cosmetic
    /// loss: `using p: Point` formatted as `p: Point` **changes what the program means** — every
    /// promoted bare name in the body stops resolving. The formatter has deleted a construct in
    /// four consecutive waves now (`cast`, `xx`, `for`/`defer`, and this), so the three call sites
    /// share one helper rather than repeating a condition that can be forgotten at one of them.
    fn emit_using(&mut self, node: &SyntaxNode) {
        if node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == USING_KW)
        {
            self.emit("using ");
        }
    }

    fn format_field(&mut self, node: &SyntaxNode) {
        self.emit_using(node);
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
                // `Box(s64)` — the type arguments, when present, follow the name with no space
                // (ADR-0085 §3). Dropping them would turn `Box(s64)` into a bare `Box`, the lossy-CST
                // failure this file guards against.
                if let Some(args) = node.children().find(|n| n.kind() == TYPE_ARGUMENTS) {
                    self.format_type_arguments(&args);
                }
            }
            // `$T` (ADR-0081 §1): a `$` then the variable name. Handled explicitly, because a
            // formatter that dropped it would silently turn a polymorphic parameter into a bare one —
            // the lossy-CST failure this file keeps having to guard against.
            POLY_TYPE => {
                self.emit("$");
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
            VIEW_TYPE => {
                // No length child to format, which is the whole difference from `ARRAY_TYPE`
                // (ADR-0044 §1). Its own arm rather than a shared one whose length happens to
                // be absent: `is_expr_kind` would find no length in a *malformed* array
                // either, and emitting `[]` for that would silently change the program.
                self.emit("[]");
                if let Some(elem) = node.children().find(|n| is_type_kind(n.kind())) {
                    self.format_type(&elem);
                }
            }
            DYNAMIC_ARRAY_TYPE => {
                // `[..]T` (ADR-0136 §1). No length or capacity child; the `..` is a marker
                // token rather than an expression, so it does not need re-emitting from a
                // child.
                self.emit("[..]");
                if let Some(elem) = node.children().find(|n| is_type_kind(n.kind())) {
                    self.format_type(&elem);
                }
            }
            ARRAY_TYPE => {
                self.emit("[");
                // The length is an *expression* child (ADR-0039 §3), so it is formatted as
                // one rather than emitted as raw text — which keeps `[ 4 ]u8` normalising
                // to `[4]u8` like every other construct.
                if let Some(len) = node.children().find(|n| is_expr_kind(n.kind())) {
                    self.format_expr(&len);
                }
                self.emit("]");
                if let Some(elem) = node.children().find(|n| is_type_kind(n.kind())) {
                    self.format_type(&elem);
                }
            }
            STRUCT_TYPE | UNION_TYPE | VARIANT_TYPE => {
                self.format_struct_type(node);
            }
            ENUM_TYPE => {
                self.format_enum_type(node);
            }
            PROC_TYPE => {
                // `(T, T) -> T` (ADR-0059 §3). An explicit arm rather than the raw-text fallback
                // below, for the reason this file keeps re-learning: raw text stops canonicalising,
                // so `( s64 ,bool )->T` would survive as written instead of normalising like every
                // other construct. The parameters live in a `PROC_TYPE_PARAMS` child; the return
                // type is the one type child outside it.
                self.emit("(");
                if let Some(params) = node.children().find(|n| n.kind() == PROC_TYPE_PARAMS) {
                    let types: Vec<SyntaxNode> = params
                        .children()
                        .filter(|n| is_type_kind(n.kind()))
                        .collect();
                    for (i, ty) in types.iter().enumerate() {
                        if i > 0 {
                            self.emit(", ");
                        }
                        self.format_type(ty);
                    }
                }
                self.emit(")");
                // The arrow is **optional** (ADR-0062 §1): `(s64)` is a void-returning procedure
                // pointer. Emitting `") -> "` unconditionally produced `(s64) -> ` with nothing
                // after it — which does not parse, so the formatter turned a legal program into an
                // illegal one. Found by the round-trip test rather than by reading.
                if let Some(ret) = node
                    .children()
                    .filter(|n| n.kind() != PROC_TYPE_PARAMS)
                    .find(|n| is_type_kind(n.kind()))
                {
                    self.emit(" -> ");
                    self.format_type(&ret);
                }
            }
            _ => {
                self.emit(&node.text().to_string());
            }
        }
    }

    /// `(s64, bool)` — the type arguments of `Box(s64)` (ADR-0085 §3).
    ///
    /// Comma-and-space separated, no space before the `(`, mirroring `PROC_TYPE`'s parameter list —
    /// an explicit walk of the type children rather than raw text, so `Box( s64 )` canonicalises.
    fn format_type_arguments(&mut self, node: &SyntaxNode) {
        self.emit("(");
        let types: Vec<SyntaxNode> = node.children().filter(|n| is_type_kind(n.kind())).collect();
        for (i, ty) in types.iter().enumerate() {
            if i > 0 {
                self.emit(", ");
            }
            self.format_type(ty);
        }
        self.emit(")");
    }

    /// `($T)` or `($K, $V)` — the type parameters of `struct($T) { … }` (ADR-0085 §3).
    ///
    /// Each parameter is a `$T` (`POLY_TYPE`), formatted the same way [`Formatter::format_type`] does,
    /// so the `$` survives — the lossy-CST failure this file guards against.
    fn format_struct_type_params(&mut self, node: &SyntaxNode) {
        self.emit("(");
        let vars: Vec<SyntaxNode> = node.children().filter(|n| n.kind() == POLY_TYPE).collect();
        for (i, var) in vars.iter().enumerate() {
            if i > 0 {
                self.emit(", ");
            }
            self.format_type(var);
        }
        self.emit(")");
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
                // A destructuring declaration's `DECL_STMT` wraps the `TARGET_LIST` *directly*
                // rather than a `VAR_DECL` — the parser reuses the statement kind and there is no
                // inner declaration node to delegate to (ADR-0052 §2). Falling through to
                // `format_item` on the target list emitted the names and dropped the `:= f();`.
                if node.children().any(|n| n.kind() == TARGET_LIST) {
                    self.format_var_decl(node);
                    return;
                }
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
            FOR_STMT => {
                self.emit_indent();
                self.format_for_stmt(node);
            }
            DEFER_STMT => {
                self.emit_indent();
                self.emit("defer ");
                // The deferred thing is an arbitrary statement (ADR-0049 §3), and it is formatted
                // *inline* so that `defer close(a);` stays on one line. `format_stmt` would emit
                // its own indent and newline, putting the statement on the line below `defer`.
                if let Some(inner) = node.children().find(|n| is_stmt_kind(n.kind())) {
                    self.format_single_stmt_inline(&inner);
                }
                self.newline();
            }
            SWITCH_STMT => {
                self.emit_indent();
                self.emit("switch ");
                // The scrutinee is the switch's *own* expression child; an arm's case value lives under
                // a `SWITCH_ARM`, so `children()` here sees only this one (ADR-0067 §1).
                if let Some(value) = node.children().find(|n| is_expr_kind(n.kind())) {
                    self.format_expr(&value);
                }
                self.emit(" {");
                self.newline();
                self.indent += 1;
                for arm in node.children().filter(|n| n.kind() == SWITCH_ARM) {
                    self.emit_indent();
                    // An absent value is the `else` arm — but a *malformed* `case ;` also has none, so
                    // the keyword decides, exactly as `SwitchArm::is_else` does. Printing `else` for a
                    // broken `case` would change what the source says.
                    let is_else = arm
                        .children_with_tokens()
                        .filter_map(|e| e.into_token())
                        .any(|t| t.kind() == ELSE_KW);
                    if is_else {
                        self.emit("else;");
                    } else {
                        self.emit("case ");
                        if let Some(value) = arm.children().find(|n| is_expr_kind(n.kind())) {
                            self.format_expr(&value);
                        }
                        self.emit(";");
                    }
                    self.newline();
                    // The arm's statements, indented under its header. Each is a full statement, so
                    // `format_stmt` emits its own indent and newline.
                    self.indent += 1;
                    for stmt in arm.children().filter(|n| is_stmt_kind(n.kind())) {
                        self.format_stmt(&stmt);
                    }
                    self.indent -= 1;
                }
                self.indent -= 1;
                self.emit_indent();
                self.emit("}");
                self.newline();
            }
            // `#code { … }` (ADR-0080 §1) formats like `push_context`: a keyword-ish head then a braced
            // block. It must be handled explicitly, because a formatter that dropped the body would
            // silently delete the spliced code — the lossy-CST failure ADR-0072 §1 warned of and which
            // ADR-0073 actually hit when `#insert CODE;` formatted to `#insert;`.
            CODE_STMT => {
                self.emit_indent();
                self.emit("#code ");
                if let Some(body) = node.children().find(|n| n.kind() == BLOCK) {
                    self.format_block(&body);
                }
                self.newline();
            }
            PUSH_CONTEXT_STMT => {
                self.emit_indent();
                self.emit("push_context ");
                // The body is always a braced block (the parser requires it, ADR-0063 §1), so it is
                // formatted exactly as a `while`'s block is — `format_block` for the braces and the
                // indented statements. Emitting nothing for a missing block would drop the whole
                // construct, which is the formatter-loses-a-statement failure the last waves keep
                // hitting; this file has no such default, so a `BLOCK` child is found or nothing is.
                if let Some(body) = node.children().find(|n| n.kind() == BLOCK) {
                    self.format_block(&body);
                }
                self.newline();
            }
            LOOP_LABEL => {
                self.emit_indent();
                if let Some(name) = node.children().find(|n| n.kind() == NAME) {
                    self.emit(name.text().to_string().trim());
                }
                self.emit(": ");
                // The label *wraps* the loop, so the loop is formatted without its own indent —
                // it is already on this line, after the `name:`.
                if let Some(inner) = node.children().find(|n| n.kind() == FOR_STMT) {
                    self.format_for_stmt(&inner);
                } else if let Some(inner) = node.children().find(|n| n.kind() == WHILE_STMT) {
                    self.format_while_stmt(&inner);
                }
            }
            RETURN_STMT => {
                self.emit_indent();
                self.emit("return");
                // **Every** returned expression, not just the first (ADR-0052 §1). Taking only
                // `find(..)` silently dropped `return a, b;` to `return a;` — a change in what the
                // program *computes*, which is the worst thing this file can do and the same class
                // of loss as ADR-0050's deleted `using`.
                let exprs: Vec<SyntaxNode> =
                    node.children().filter(|n| is_expr_kind(n.kind())).collect();
                for (index, expr) in exprs.iter().enumerate() {
                    self.emit(if index == 0 { " " } else { ", " });
                    self.format_expr(expr);
                }
                self.emit_trailing_comment(node);
                self.emit(";");
                self.newline();
            }
            BREAK_STMT => {
                self.emit_indent();
                self.emit("break");
                self.emit_jump_label(node);
                self.emit_trailing_comment(node);
                self.emit(";");
                self.newline();
            }
            CONTINUE_STMT => {
                self.emit_indent();
                self.emit("continue");
                self.emit_jump_label(node);
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
        // `q, ok = f();` (ADR-0052 §2), the assignment counterpart of the declaration form above.
        if let Some(list) = node.children().find(|n| n.kind() == TARGET_LIST) {
            self.format_target_list(&list);
            self.emit(" = ");
            if let Some(expr) = node.children().find(|n| is_expr_kind(n.kind())) {
                self.format_expr(&expr);
            }
            return;
        }
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
            BREAK_STMT => {
                self.emit("break");
                self.emit_jump_label(node);
                self.emit(";");
            }
            CONTINUE_STMT => {
                self.emit("continue");
                self.emit_jump_label(node);
                self.emit(";");
            }
            // A `defer` can be the single braceless statement of an `if` — `if bad  defer f();`
            // parses — so this arm exists for the same reason the others do: the fallback emits
            // `node.text()` verbatim, which would carry the original spacing rather than
            // canonicalising it.
            DEFER_STMT => {
                self.emit("defer ");
                if let Some(inner) = node.children().find(|n| is_stmt_kind(n.kind())) {
                    self.format_single_stmt_inline(&inner);
                }
            }
            _ => {
                self.emit(node.text().to_string().trim());
            }
        }
    }

    /// Emits ` name` for a `break`/`continue` that names a loop, and nothing for one that does not.
    ///
    /// Shared by the block and the braceless-inline paths, because a label dropped on one of them
    /// would silently retarget the jump to the innermost loop — a *behaviour* change from
    /// formatting, which is the worst thing this file can do.
    fn emit_jump_label(&mut self, node: &SyntaxNode) {
        if let Some(name) = node.children().find(|n| n.kind() == NAME) {
            self.emit(" ");
            self.emit(name.text().to_string().trim());
        }
    }

    /// Formats `for x: buf`, `for x, i: buf` and `for < x: buf` (ADR-0049 §1).
    ///
    /// The `<` is emitted with a space either side — `for < x: buf` — because it is a *marker on
    /// the loop* rather than an operator, and running it into the name (`for <x`) would read as a
    /// comparison against `x`.
    fn format_for_stmt(&mut self, node: &SyntaxNode) {
        self.emit("for ");
        if node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == LT)
        {
            self.emit("< ");
        }
        let names: Vec<SyntaxNode> = node.children().filter(|n| n.kind() == NAME).collect();
        // A nameless `for xs { … }` has no NAME children — inject nothing, and skip the `:`
        // (ADR-0133). The named form still emits `name : iter`.
        for (i, name) in names.iter().enumerate() {
            if i > 0 {
                self.emit(", ");
            }
            self.emit(name.text().to_string().trim());
        }
        if !names.is_empty() {
            self.emit(": ");
        }
        if let Some(iterable) = node.children().find(|n| is_expr_kind(n.kind())) {
            self.format_expr(&iterable);
        }
        if let Some(body) = node.children().find(|n| n.kind() == BLOCK) {
            self.emit(" ");
            self.format_block(&body);
        }
        self.newline();
    }

    /// Formats `(s64, bool)` after `->` (ADR-0052 §1).
    ///
    /// One space after each comma and none inside the brackets, matching every other comma-separated
    /// list this formatter emits — a parameter list and an argument list both look like this.
    fn format_result_list(&mut self, node: &SyntaxNode) {
        self.emit("(");
        let tys: Vec<SyntaxNode> = node.children().filter(|n| is_type_kind(n.kind())).collect();
        for (index, ty) in tys.iter().enumerate() {
            if index > 0 {
                self.emit(", ");
            }
            self.format_type(ty);
        }
        self.emit(")");
    }

    /// Formats `q, ok` — a destructuring target list (ADR-0052 §2).
    ///
    /// A `_` is emitted from its `NAME` node's text like any other target, because the parser keeps
    /// a discard as a `NAME` whose text happens to be `_`: it is lowering that recognises the hole
    /// (ADR-0052 §3), so the formatter needs no special case and cannot lose one.
    fn format_target_list(&mut self, node: &SyntaxNode) {
        let names: Vec<SyntaxNode> = node.children().filter(|n| n.kind() == NAME).collect();
        for (index, name) in names.iter().enumerate() {
            if index > 0 {
                self.emit(", ");
            }
            self.emit(name.text().to_string().trim());
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
                            INT_LITERAL
                                | FLOAT_LITERAL
                                | STRING_LITERAL
                                | TRUE_KW
                                | FALSE_KW
                                | NULL_KW
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
                    // `TILDE` for `~`; without it the operator vanished and `~a` formatted
                    // to `a`, which is a *different program* rather than a formatting change.
                    .find(|t| matches!(t.kind(), MINUS | BANG | STAR | TILDE))
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
            SLICE_EXPR => {
                // The base and nothing else. Emitting the node's raw text would work today
                // and would stop normalising the base — `( buf )[]` must still become
                // `(buf)[]` with the parens formatted, not preserved verbatim.
                if let Some(base) = node.children().find(|n| is_expr_kind(n.kind())) {
                    self.format_expr(&base);
                }
                self.emit("[]");
            }
            INDEX_EXPR => {
                // The base is the first expression child and the index the second, matching
                // the order the postfix parser builds them in.
                let mut exprs = node.children().filter(|n| is_expr_kind(n.kind()));
                if let Some(base) = exprs.next() {
                    self.format_expr(&base);
                }
                self.emit("[");
                if let Some(index) = exprs.next() {
                    self.format_expr(&index);
                }
                self.emit("]");
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
            // `xx expr` — a prefix operator, so a space follows the keyword and nothing else
            // changes. Its own arm rather than joining the unary emitter's token list, because
            // that list works from an operator *token* and `xx` is a keyword.
            AUTOCAST_EXPR => {
                self.emit("xx ");
                if let Some(operand) = node.children().find(|n| is_expr_kind(n.kind())) {
                    self.format_expr(&operand);
                }
            }
            // `.RED` — the leading `.` and the name, and deliberately not the node's raw text:
            // that would stop normalising anything inside, and a formatter that preserves one
            // construct verbatim is a formatter with an exception to explain.
            MEMBER_EXPR => {
                self.emit(".");
                if let Some(name) = node
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| t.kind() == IDENT)
                {
                    self.emit(name.text());
                }
            }
            CAST_EXPR => {
                // `cast(T, x)` — the type first, then the operand. Both children are found
                // by *kind*, because the type is a `NAME_TYPE`/`POINTER_TYPE` and the operand
                // an expression, so a positional search would confuse them the day a type
                // becomes expressible as an expression.
                self.emit("cast(");
                if let Some(target) = node.children().find(|n| is_type_kind(n.kind())) {
                    self.format_type(&target);
                }
                self.emit(", ");
                if let Some(operand) = node.children().find(|n| is_expr_kind(n.kind())) {
                    self.format_expr(&operand);
                }
                self.emit(")");
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
                // A directive's operand is either a bare `STRING_LITERAL` *token* (the literal `#insert`
                // and `#system_library "c"`) or a full **operand expression** (`#insert CODE;`,
                // `#insert #run build();` — ADR-0073 §1). Emitting only the string token silently *dropped*
                // a computed operand, rewriting `#insert CODE;` to `#insert;` — the CST-preservation
                // failure ADR-0072 §1 warned of, now for the operand syntax. A child expression node is
                // formatted; a bare string token is emitted verbatim.
                if let Some(operand) = node.children().find(|n| is_expr_kind(n.kind())) {
                    self.emit(" ");
                    self.format_expr(&operand);
                } else if let Some(arg) = node
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| t.kind() == STRING_LITERAL)
                {
                    self.emit(" ");
                    self.emit(arg.text());
                }
            }
            // `context` — a keyword standing for the current context (ADR-0057 §1). Emitted from its
            // static text rather than the node's, which is empty of anything but the keyword token.
            CONTEXT_EXPR => self.emit("context"),
            RANGE_EXPR => {
                // `0..4` with no spaces around the `..`, matching every language that has ranges
                // and keeping the loop header short enough to read as one unit.
                let mut ends = node.children().filter(|n| is_expr_kind(n.kind()));
                if let Some(start) = ends.next() {
                    self.format_expr(&start);
                }
                self.emit("..");
                if let Some(end) = ends.next() {
                    self.format_expr(&end);
                }
            }
            _ => {
                self.emit(&node.text().to_string());
            }
        }
    }

    fn format_arg_list(&mut self, node: &SyntaxNode) {
        self.emit("(");
        // **A `NAMED_ARG` is not an expression kind**, so filtering on `is_expr_kind` alone dropped
        // every named argument silently — `draw(y = 2, x = 1)` became `draw()`, which changes what
        // the program computes. Both kinds are collected and each formatted by its own shape.
        let args: Vec<SyntaxNode> = node
            .children()
            .filter(|n| is_expr_kind(n.kind()) || n.kind() == NAMED_ARG)
            .collect();
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                self.emit(", ");
            }
            if arg.kind() == NAMED_ARG {
                self.format_named_arg(arg);
            } else {
                self.format_expr(arg);
            }
        }
        self.emit(")");
    }

    /// Formats `name = value` (ADR-0053 §1).
    ///
    /// One space either side of the `=`, matching an assignment statement — the two look the same
    /// because they read the same way, and Jairs has no assignment expression for this to be
    /// confused with.
    fn format_named_arg(&mut self, node: &SyntaxNode) {
        if let Some(name) = node.children().find(|n| n.kind() == NAME) {
            self.emit(name.text().to_string().trim());
        }
        self.emit(" = ");
        if let Some(value) = node.children().find(|n| is_expr_kind(n.kind())) {
            self.format_expr(&value);
        }
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
            // Omitting this is how the formatter *deleted* every `cast` outright: an
            // unrecognised expression kind is not emitted at all, so `small := cast(u8, big);`
            // formatted to `small := ;`. The same shape as the struct-comment deletion an
            // earlier wave fixed — silent data loss, caught only because the corpus
            // round-trip gate re-parses its own output.
            | CAST_EXPR
            | AUTOCAST_EXPR
            | MEMBER_EXPR
            // Same reason as `CAST_EXPR` above, and the reason the previous wave's trap
            // list says to check this on every new expression kind (ADR-0039).
            | INDEX_EXPR
            | SLICE_EXPR
            // A range reaches this only as a `for`'s iterable (ADR-0049 §1) — there is no `..`
            // in the expression grammar — but it is found *through* this predicate, so omitting
            // it would leave `for i: 0..4` with nothing to iterate.
            | RANGE_EXPR
            // `context` (ADR-0057 §1). Omitting it deleted every `context` — the seventh wave to lose
            // source this way, and the same trap as `NAMED_ARG` and `RESULT_LIST`: a node sitting
            // where an expression sits is not, by itself, an expression kind.
            | CONTEXT_EXPR
            | RUN_EXPR
            | DIRECTIVE_EXPR
    )
}

/// Returns `true` if `kind` is a type node kind.
fn is_type_kind(kind: SyntaxKind) -> bool {
    // `ARRAY_TYPE` belongs here for the same reason `INDEX_EXPR` belongs in
    // `is_expr_kind`: an unrecognised kind is not emitted, so omitting it would make the
    // formatter delete the type from `buf: [4]u8;`.
    matches!(
        kind,
        NAME_TYPE
            | POLY_TYPE
            | POINTER_TYPE
            | STRUCT_TYPE
            | UNION_TYPE
            | VARIANT_TYPE
            | ARRAY_TYPE
            | VIEW_TYPE
            | DYNAMIC_ARRAY_TYPE
            | ENUM_TYPE
            | PROC_TYPE
    )
}

/// Returns `true` if `kind` is a statement node kind.
///
/// `FOR_STMT`, `DEFER_STMT` and `LOOP_LABEL` belong here for the reason the whole file keeps
/// re-learning: a kind absent from one of these predicates is *silently dropped*, so leaving them
/// out deleted every `for` and every `defer` from the corpus (ADR-0049's consequence list, and the
/// third consecutive wave to lose source this way).
fn is_stmt_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        DECL_STMT
            | EXPR_STMT
            | ASSIGN_STMT
            | IF_STMT
            | WHILE_STMT
            | FOR_STMT
            | DEFER_STMT
            | CODE_STMT
            | PUSH_CONTEXT_STMT
            | SWITCH_STMT
            | LOOP_LABEL
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
            // The bitwise operators (ADR-0042). A binary operator missing from this set is
            // not emitted at all, so `6 & 3 | 1` formatted to `631` — the fourth
            // kind-filtered list this wave had to extend, and the one that loses data.
            | AMP
            | PIPE
            | CARET
            | SHL
            | SHR
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
            // Same reason as `is_binary_op`: an omitted assignment operator is not emitted,
            // so `a |= 1;` formatted to `a1;` (ADR-0042 §6).
            | AMP_EQ
            | PIPE_EQ
            | CARET_EQ
            | SHL_EQ
            | SHR_EQ
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

    /// Named arguments and defaults (ADR-0053), asserted to *survive*.
    ///
    /// **Sixth consecutive wave** for the formatter trap, and this one lost both halves: every
    /// default vanished (turning a callable `f(1)` into an arity error) and every named argument
    /// vanished with it, because `NAMED_ARG` is not an expression kind and the argument-list walk
    /// filtered on `is_expr_kind`. Both change what the program means.
    #[test]
    fn named_arguments_and_defaults_survive() {
        let src = "f :: (a: s64, b: s64 = 10, c: bool = true) -> s64 {\n    return a;\n}\n\ng :: () -> s64 {\n    return f(1, c = false, b = 2);\n}\n";
        let out = fmt(src);
        assert!(
            out.contains("b: s64 = 10"),
            "an integer default must survive: {out}"
        );
        assert!(
            out.contains("c: bool = true"),
            "a boolean default must survive: {out}"
        );
        assert!(
            out.contains("f(1, c = false, b = 2)"),
            "every named argument must survive, in order: {out}"
        );
        assert_idempotent(src);
        assert_parses(&out);
    }

    /// The formatter must *canonicalise* these forms, not pass them through.
    #[test]
    fn named_arguments_are_canonicalised() {
        let src =
            "f :: (a: s64,b: s64=10) -> s64 { return a; }\ng :: () -> s64 { return f(1,b=2); }\n";
        let out = fmt(src);
        assert!(out.contains("(a: s64, b: s64 = 10)"), "got: {out}");
        assert!(out.contains("f(1, b = 2)"), "got: {out}");
        assert_parses(&out);
    }

    /// Multiple returns (ADR-0052), asserted to *survive*.
    ///
    /// Fifth consecutive wave for the formatter trap, and this one lost source in **two** ways: the
    /// result list vanished entirely (`-> (s64, bool)` became `-> `), and a multi-value return was
    /// truncated to its first value (`return a, b;` became `return a;`). The second changes what the
    /// program computes, which is the same class of loss as ADR-0050's deleted `using`.
    #[test]
    fn multiple_returns_survive() {
        let src = "f :: () -> (s64, bool) {\n    return 1, true;\n}\n\ng :: () {\n    q, ok := f();\n    q, ok = f();\n}\n";
        let out = fmt(src);
        assert!(
            out.contains("-> (s64, bool)"),
            "the result list must survive: {out}"
        );
        assert!(
            out.contains("return 1, true;"),
            "every returned value must survive: {out}"
        );
        assert!(out.contains("q, ok := f();"), "got: {out}");
        assert!(out.contains("q, ok = f();"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    /// A `_` discard must survive in either position (ADR-0052 §3).
    #[test]
    fn discards_survive_in_either_position() {
        let src = "f :: () -> (s64, bool) {\n    return 1, true;\n}\n\ng :: () {\n    a, _ := f();\n    _, b := f();\n    _, _ := f();\n}\n";
        let out = fmt(src);
        assert!(out.contains("a, _ := f();"), "got: {out}");
        assert!(out.contains("_, b := f();"), "got: {out}");
        assert!(out.contains("_, _ := f();"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    /// The formatter must *canonicalise* these forms, not pass them through.
    #[test]
    fn multiple_returns_are_canonicalised() {
        let src = "f :: ()->(s64,bool){\nreturn 1,true;\n}\ng :: (){\nq,ok:=f();\n}\n";
        let out = fmt(src);
        assert!(out.contains("f :: () -> (s64, bool) {"), "got: {out}");
        assert!(out.contains("    return 1, true;"), "got: {out}");
        assert!(out.contains("    q, ok := f();"), "got: {out}");
        assert_parses(&out);
    }

    /// `context` and `#c_call` (ADR-0057), asserted to *survive*.
    ///
    /// Seventh consecutive wave for the formatter trap, and `#c_call` is the worst kind: dropping it
    /// does not lose formatting, it changes the *calling convention* — a `#c_call` procedure with the
    /// attribute gone silently starts taking a context, and every caller is recompiled against a
    /// different ABI. `context` deleting to nothing (`x := ;`) is the same node-in-expression-position
    /// trap as `RESULT_LIST` and `NAMED_ARG` before it.
    #[test]
    fn context_and_c_call_survive() {
        let src = "raw :: () #c_call {\n    x := context;\n}\n\nf :: () {\n    context.allocator = 1;\n}\n";
        let out = fmt(src);
        assert!(
            out.contains("() #c_call {"),
            "the `#c_call` attribute must survive: {out}"
        );
        assert!(
            out.contains("x := context;"),
            "`context` must survive: {out}"
        );
        assert!(
            out.contains("context.allocator = 1;"),
            "`context.field` must survive: {out}"
        );
        assert_idempotent(src);
        assert_parses(&out);
    }

    /// A procedure-pointer type survives and canonicalises (ADR-0059 §3).
    ///
    /// Ninth wave running that the formatter could lose a construct — and a proc-pointer type is the
    /// kind that lands in the raw-text fallback rather than being deleted, which is worse in a
    /// quieter way: it *survives* but stops normalising, so `( s64 , bool )->T` would round-trip as
    /// written. The test asserts canonical spacing, not just survival, for that reason. Both a
    /// parameter position and a return position are checked, because the return one goes through the
    /// results-list disambiguation and could regress independently.
    /// `null` survives formatting (ADR-0060 §1).
    ///
    /// A literal, so the easy case — it round-trips as its own text — but the formatter has lost a
    /// construct in eight of the last ten waves, and a literal that lands in a raw-text fallback
    /// stops canonicalising, so it is pinned rather than assumed.
    #[test]
    fn null_survives() {
        let src = "f :: () {\n    p: *u8 = null;\n}\n";
        let out = fmt(src);
        assert!(out.contains("p: *u8 = null;"), "`null` must survive: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    #[test]
    fn a_procedure_pointer_type_canonicalises() {
        let src = "apply :: (fn: (s64,bool)->s64, a: s64) -> s64 {\n    return a;\n}\n";
        let out = fmt(src);
        assert!(
            out.contains("fn: (s64, bool) -> s64"),
            "a proc-pointer parameter type must canonicalise: {out}"
        );
        assert_idempotent(src);
        assert_parses(&out);

        // Return position, where the `(` is disambiguated from a results list by the `->`.
        let ret = "pick :: () -> (s64, s64) -> s64 {\n    return add;\n}\n";
        let out = fmt(ret);
        assert!(
            out.contains("-> (s64, s64) -> s64"),
            "a proc-pointer return type must survive: {out}"
        );
        assert_idempotent(ret);
        assert_parses(&out);

        // A **void-returning** proc pointer, `(T)` with no arrow (ADR-0062 §1). The emitter wrote
        // `") -> "` unconditionally before this, producing `(*u8) -> ` with nothing after it — which
        // does not parse, so the formatter turned a legal program into an illegal one. `assert_parses`
        // is what catches that, and it is why it is in every one of these tests.
        let void_ret = "Sink :: struct {\n    put: (*u8);\n}\n";
        let out = fmt(void_ret);
        assert!(
            out.contains("put: (*u8);"),
            "a void-returning proc-pointer type must survive without an arrow: {out}"
        );
        assert!(!out.contains("->"), "and must not grow one: {out}");
        assert_idempotent(void_ret);
        assert_parses(&out);
    }

    /// `#no_abc` survives, canonicalises, and does not get **reordered** (ADR-0058 §3).
    ///
    /// Eighth consecutive wave for the formatter losing a construct. This one is the *safe*
    /// direction to lose — dropping it restores a bounds check, so the program gets slower rather
    /// than unsound — which is exactly why it needs a test: nothing about the program's behaviour
    /// would tell anyone it had happened.
    ///
    /// The reordering assertion is the one that is not obvious. The parser accepts the two
    /// attributes in either order, so a formatter emitting them in a fixed order would silently
    /// rewrite `#no_abc #c_call` into `#c_call #no_abc`. That is not lost source, but it means
    /// `jr fmt` is not idempotent on input it did not write, and gate 5 fails on a corpus file
    /// written the other way round.
    #[test]
    fn no_abc_survives_and_keeps_its_position() {
        let src = "read :: (buf: [4]s64, i: s64) -> s64 #no_abc {\n    return buf[i];\n}\n";
        let out = fmt(src);
        assert!(
            out.contains("-> s64 #no_abc {"),
            "the `#no_abc` attribute must survive, after the return type: {out}"
        );
        assert_idempotent(src);
        assert_parses(&out);

        // Both attributes, in each order, each preserved as written.
        let both = "a :: () #c_call #no_abc {\n}\n\nb :: () #no_abc #c_call {\n}\n";
        let out = fmt(both);
        assert!(
            out.contains("a :: () #c_call #no_abc {"),
            "the written order must be kept: {out}"
        );
        assert!(
            out.contains("b :: () #no_abc #c_call {"),
            "and kept in the other direction too, or the formatter is reordering: {out}"
        );
        assert_idempotent(both);
        assert_parses(&out);
    }

    /// `using` in all three positions (ADR-0050 §1), each asserted to *survive*.
    ///
    /// Fourth consecutive wave for this trap — `cast`, then every `xx`, then `for`/`defer`, now
    /// this — and the worst of the four: dropping `using` does not merely lose formatting, it
    /// **changes what the program means**, because every promoted bare name in the body stops
    /// resolving. The formatter deleted all three positions before these assertions existed.
    #[test]
    fn using_survives_in_all_three_positions() {
        let src = "Entity :: struct {\n    using base: Point;\n    hp: s64;\n}\n\nlen2 :: (using p: Point) -> s64 {\n    using q: Point;\n    return x;\n}\n";
        let out = fmt(src);
        assert!(
            out.contains("using base: Point;"),
            "an embedded field must keep its keyword: {out}"
        );
        assert!(
            out.contains("(using p: Point)"),
            "a promoted parameter must keep its keyword: {out}"
        );
        assert!(
            out.contains("using q: Point;"),
            "a promoted local must keep its keyword: {out}"
        );
        assert_idempotent(src);
        assert_parses(&out);
    }

    /// The formatter must *canonicalise* `using`, not merely pass it through.
    ///
    /// Written for the reason ADR-0049's equivalent test was: the round-trip and idempotence
    /// assertions above are both satisfied by a formatter that emits `node.text()` verbatim, so
    /// only a misformatted input proves the emitter runs.
    #[test]
    fn using_is_canonicalised_not_passed_through() {
        let src = "f :: (using    p:Point)->s64{\nusing   q:Point;\nreturn x;\n}\n";
        let out = fmt(src);
        assert!(out.contains("(using p: Point)"), "got: {out}");
        assert!(out.contains("    using q: Point;"), "got: {out}");
        assert_parses(&out);
    }

    /// The four `for` forms (ADR-0049 §1), each asserted to *survive*.
    ///
    /// This test exists because a kind missing from `is_stmt_kind` or `is_expr_kind` makes the
    /// formatter **delete** the construct rather than mangle it, and that has now happened in three
    /// consecutive waves — `cast`, then every `xx`, then every `for` and `defer` here. Asserting the
    /// keyword is still present is what turns the fourth occurrence into a test failure.
    #[test]
    fn for_stmt_forms() {
        let src = "f :: () {\n    for x: buf {\n        t = t + x;\n    }\n    for x, i: buf {\n        t = t + i;\n    }\n    for i: 0..4 {\n        t = t + i;\n    }\n    for < x: buf {\n        t = t + x;\n    }\n}\n";
        let out = fmt(src);
        assert!(out.contains("for x: buf {"), "got: {out}");
        assert!(out.contains("for x, i: buf {"), "got: {out}");
        assert!(out.contains("for i: 0..4 {"), "got: {out}");
        assert!(
            out.contains("for < x: buf {"),
            "the reverse marker must survive: {out}"
        );
        assert_idempotent(src);
        assert_parses(&out);
    }

    /// A label must survive on the loop *and* on the jump.
    ///
    /// Dropping it would not be a cosmetic loss: `break outer` silently becoming `break` retargets
    /// the jump to the innermost loop, so the formatter would change what the program *does*.
    #[test]
    fn loop_labels_survive() {
        let src = "f :: () {\n    outer: for a: rows {\n        for b: cols {\n            break outer;\n        }\n    }\n    lbl: while c {\n        continue lbl;\n    }\n}\n";
        let out = fmt(src);
        assert!(out.contains("outer: for a: rows {"), "got: {out}");
        assert!(out.contains("break outer;"), "got: {out}");
        assert!(out.contains("lbl: while c {"), "got: {out}");
        assert!(out.contains("continue lbl;"), "got: {out}");
        assert_idempotent(src);
        assert_parses(&out);
    }

    /// `defer` wraps an arbitrary statement (ADR-0049 §3), formatted inline.
    #[test]
    fn defer_stmt() {
        let src = "f :: () {\n    defer close(a);\n    defer n = n + 1;\n}\n";
        let out = fmt(src);
        assert!(out.contains("defer close(a);"), "got: {out}");
        assert!(
            out.contains("defer n = n + 1;"),
            "a `defer` over an assignment stays on one line: {out}"
        );
        assert_idempotent(src);
        assert_parses(&out);
    }

    /// The formatter must *canonicalise* these forms, not merely pass them through.
    ///
    /// Written because the round-trip and idempotence assertions above are both satisfied by a
    /// formatter that emits `node.text()` verbatim — which is what the fallback arm does, and which
    /// would leave `outer:for<x,i:buf` untouched while looking green.
    #[test]
    fn for_and_defer_are_canonicalised_not_passed_through() {
        let src = "f :: () {\nouter:for<x,i:buf{\ndefer n=n+1;\nbreak outer;\n}\n}\n";
        let out = fmt(src);
        assert!(out.contains("outer: for < x, i: buf {"), "got: {out}");
        assert!(out.contains("        defer n = n + 1;"), "got: {out}");
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
