//! Code actions: the first capability that offers to *change* the code.
//!
//! # Why almost every action hangs off a diagnostic
//!
//! [ADR-0031](../../../docs/adr/0031-code-actions-and-hints.md) §4. A client sends
//! `textDocument/codeAction` with the diagnostics it currently has in the requested range,
//! and answering from those rather than from a fresh analysis is what makes an action
//! appear exactly where the user already sees a problem. It also means the *decision* that
//! something is wrong stays in the compiler: this module never concludes that a field name
//! is misspelled, it reads E0218's `help:` line, which `jr-sema` wrote (§1).
//!
//! The one action with no diagnostic is `//` → `///`, which is why it is a `refactor`
//! rather than a `quickfix`: nothing is wrong with an ordinary comment.
//!
//! # Why no action reformats
//!
//! Every edit here replaces a span the compiler produced, or inserts a whole line. None
//! re-indents, reflows, or re-parses. `jr-fmt` owns that, and an action that formatted
//! would be a second formatter — the shape of mistake ADR-0028 §1 exists to prevent one
//! layer up.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use jr_db::{Db, ModuleSearchPaths, SourceFile};
use jr_hir::ItemKind;
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Diagnostic, NumberOrString, TextEdit,
    WorkspaceEdit,
};

use crate::position::{Encoding, Positions};

/// Every action offered for a range and the diagnostics inside it.
///
/// Returns a list rather than `None` when there is nothing to offer, because an empty list
/// and a failed request are different things to a client — the same reason
/// [`completion()`](crate::completion()) does.
#[must_use]
pub fn code_actions(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    encoding: Encoding,
    range: lsp_types::Range,
    diagnostics: &[Diagnostic],
    workspace: &jr_db::WorkspaceFileList,
) -> Vec<CodeActionOrCommand> {
    let text = file.text(db);
    let index = jr_db::line_index(db, file);
    let positions = Positions::new(text.as_ref(), &index, encoding);
    let path = file.path(db);
    let Some(uri) = crate::uri::from_path(Path::new(path.as_ref())) else {
        return Vec::new();
    };

    let mut out = Vec::new();

    for diagnostic in diagnostics {
        let Some(NumberOrString::String(code)) = &diagnostic.code else {
            continue;
        };
        match code.as_str() {
            "E0201" => out.extend(auto_imports(
                db,
                file,
                search_paths,
                &positions,
                &uri,
                diagnostic,
                workspace,
            )),
            "E0231" => out.extend(remove_import(&uri, diagnostic)),
            // Both carry the suggestion as a `help:` line that `jr-sema` computed, so the
            // action is the same shape for either: replace the span the diagnostic points
            // at with the name it named.
            "E0218" | "E0212" => out.extend(did_you_mean(&uri, diagnostic)),
            "E0203" => out.extend(give_a_body(db, file, &positions, &uri, diagnostic)),
            _ => {}
        }
    }

    // Offered once for the file rather than once per diagnostic, and only when more than
    // one import is unused: with exactly one it would duplicate the single-import action
    // under a different title.
    out.extend(organise_imports(
        db,
        file,
        search_paths,
        &positions,
        &uri,
        &text,
    ));

    out.extend(document_comment(db, file, &positions, &uri, &text, range));

    out
}

/// One edit to one file, as a `WorkspaceEdit`.
fn one_edit(uri: &lsp_types::Uri, edits: Vec<TextEdit>) -> WorkspaceEdit {
    // `WorkspaceEdit::changes` is a `HashMap<Uri, _>` in the protocol type itself, so the
    // `mutable_key_type` lint cannot be designed around; `fluent_uri`'s interior `Cell` is
    // a lazily-computed authority cache and takes no part in `Hash` or `Eq`. Allowed here
    // rather than crate-wide so a genuinely mutable key elsewhere still fails the build.
    #[allow(
        clippy::mutable_key_type,
        reason = "the protocol's own type; Uri's Cell is a cache, not part of its hash"
    )]
    let mut changes: HashMap<lsp_types::Uri, Vec<TextEdit>> = HashMap::new();
    changes.insert(uri.clone(), edits);
    WorkspaceEdit {
        changes: Some(changes),
        ..WorkspaceEdit::default()
    }
}

/// A `quickfix` for one diagnostic.
///
/// `is_preferred` is set only by callers with exactly one candidate: a client may apply a
/// preferred action without showing it, so marking one of several would apply a guess.
fn quickfix(
    title: String,
    uri: &lsp_types::Uri,
    diagnostic: &Diagnostic,
    edits: Vec<TextEdit>,
    preferred: bool,
) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diagnostic.clone()]),
        edit: Some(one_edit(uri, edits)),
        is_preferred: Some(preferred),
        ..CodeAction::default()
    })
}

// ---------------------------------------------------------------------------
// Auto-import
// ---------------------------------------------------------------------------

/// `#import "M";` for every discovered module that exports the unresolved name.
///
/// ADR-0031 §5: the discovered modules are parsed *on this request*, because ADR-0029
/// deliberately yielded paths rather than loaded files. Where several modules export the
/// name, all are offered as separate actions rather than one guess — and none is preferred,
/// so a client cannot silently pick.
///
/// The name comes from the diagnostic's own message rather than from the cursor, because a
/// code-action request carries a range and not a position, and the range may cover the
/// whole line.
fn auto_imports(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    positions: &Positions<'_>,
    uri: &lsp_types::Uri,
    diagnostic: &Diagnostic,
    workspace: &jr_db::WorkspaceFileList,
) -> Vec<CodeActionOrCommand> {
    let Some(name) = backticked(&diagnostic.message) else {
        return Vec::new();
    };
    let Some(symbol) = db.interner().get(&name) else {
        return Vec::new();
    };

    let hir = jr_db::file_hir(db, file);
    let already: Vec<&str> = hir
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Import { path, .. } => Some(path.as_str()),
            _ => None,
        })
        .collect();

    let insert = import_insertion_point(db, file, positions);
    let own_path = file.path(db);

    let mut out = Vec::new();
    let mut offered: Vec<String> = Vec::new();
    for candidate in workspace.files.iter() {
        if candidate.to_string_lossy().as_ref() == own_path.as_ref() {
            continue;
        }
        let module = crate::render::container_of(candidate.to_string_lossy().as_ref());
        // Already imported, or already offered by another file of the same module name.
        if already.contains(&module.as_str()) || offered.contains(&module) {
            continue;
        }
        // The module must resolve *by name* to this very file, or `#import "M";` would
        // import something else: discovery finds `x.jr` anywhere in the tree, and only a
        // file on a search path is reachable by an import at all.
        let lookup = jr_db::module_file(db, search_paths, Arc::from(module.as_str()));
        let Some(found) = lookup.found else { continue };
        if found != *candidate {
            continue;
        }
        let Some(source) = db.source_file_for_path(found.to_string_lossy().as_ref()) else {
            continue;
        };
        // Only a module that actually exports the name is offered. This is the whole point
        // of parsing them: an offer for a module that does not export it replaces one error
        // with two.
        if jr_db::file_exports(db, source).get(symbol).is_none() {
            continue;
        }

        offered.push(module.clone());
        out.push(quickfix(
            format!("import `{module}` for `{name}`"),
            uri,
            diagnostic,
            vec![TextEdit {
                range: insert,
                new_text: format!("#import \"{module}\";\n"),
            }],
            false,
        ));
    }
    out
}

/// Where a new `#import` line goes: after the last existing one, else at the top.
///
/// An empty range, so the edit inserts rather than replaces. Placed after the last import
/// rather than at the very top so that a file's `//!` module documentation — which must be
/// the first thing in the file to be recognised (ADR-0027) — is never pushed down.
///
/// `pub(crate)` rather than private because completion's auto-import needs the **same** point
/// (ADR-0199 §5). Two insertion rules would be a real divergence, not a stylistic one: the quick
/// fix and the completion item would put the line in different places in the same file, and only
/// one of them can be after the module docs.
pub(crate) fn import_insertion_point(
    db: &dyn Db,
    file: SourceFile,
    positions: &Positions<'_>,
) -> lsp_types::Range {
    let hir = jr_db::file_hir(db, file);
    let last = hir
        .items
        .iter()
        .filter(|item| matches!(item.kind, ItemKind::Import { .. }))
        .map(|item| item.span)
        .next_back();

    match last {
        // The line *after* the last import: its span ends at the `;`, and the range is
        // collapsed to a point so nothing is overwritten.
        Some(span) => {
            let end = positions.range(span).end;
            lsp_types::Range {
                start: lsp_types::Position {
                    line: end.line + 1,
                    character: 0,
                },
                end: lsp_types::Position {
                    line: end.line + 1,
                    character: 0,
                },
            }
        }
        None => {
            // No imports yet. After a leading `//!` block if there is one, so module
            // documentation stays attached to the file.
            let line = first_line_after_module_docs(db, file);
            lsp_types::Range {
                start: lsp_types::Position { line, character: 0 },
                end: lsp_types::Position { line, character: 0 },
            }
        }
    }
}

/// The first line that is not part of a leading `//!` block or a blank line after it.
fn first_line_after_module_docs(db: &dyn Db, file: SourceFile) -> u32 {
    let text = file.text(db);
    let mut line = 0u32;
    for candidate in text.lines() {
        let trimmed = candidate.trim_start();
        if trimmed.starts_with("//!") {
            line += 1;
            continue;
        }
        // A blank line immediately after the block belongs to it.
        if line > 0 && trimmed.is_empty() {
            line += 1;
            continue;
        }
        break;
    }
    line
}

// ---------------------------------------------------------------------------
// Unused imports
// ---------------------------------------------------------------------------

/// Delete the whole line an unused `#import` sits on.
///
/// The diagnostic's range covers the declaration and not its trailing newline, so removing
/// only that range leaves a blank line behind. Extended to the start of the next line,
/// which is what a user means by "remove it".
fn remove_import(uri: &lsp_types::Uri, diagnostic: &Diagnostic) -> Vec<CodeActionOrCommand> {
    let module = backticked(&diagnostic.message).unwrap_or_else(|| String::from("this module"));
    vec![quickfix(
        format!("remove unused import `{module}`"),
        uri,
        diagnostic,
        vec![TextEdit {
            range: whole_lines(diagnostic.range),
            new_text: String::new(),
        }],
        true,
    )]
}

/// Remove every unused import in the file, as `source.organizeImports`.
///
/// Offered only when **two or more** are unused: with exactly one it would be the
/// single-import action under a second title, and two identical offers is worse than one.
///
/// Recomputed from `unused_imports` rather than from the client's diagnostic list, because
/// a client sends only the diagnostics in the requested range and "organise imports" is a
/// whole-file operation.
fn organise_imports(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    positions: &Positions<'_>,
    uri: &lsp_types::Uri,
    _text: &str,
) -> Vec<CodeActionOrCommand> {
    let unused = jr_db::unused_imports(db, file, search_paths);
    if unused.len() < 2 {
        return Vec::new();
    }
    let edits: Vec<TextEdit> = unused
        .imports
        .iter()
        .map(|import| TextEdit {
            range: whole_lines(positions.range(import.span)),
            new_text: String::new(),
        })
        .collect();

    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("remove {} unused imports", edits.len()),
        kind: Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS),
        edit: Some(one_edit(uri, edits)),
        ..CodeAction::default()
    })]
}

/// A range extended to cover whole lines, so deleting it leaves no blank line.
fn whole_lines(range: lsp_types::Range) -> lsp_types::Range {
    lsp_types::Range {
        start: lsp_types::Position {
            line: range.start.line,
            character: 0,
        },
        end: lsp_types::Position {
            line: range.end.line + 1,
            character: 0,
        },
    }
}

// ---------------------------------------------------------------------------
// Did you mean
// ---------------------------------------------------------------------------

/// Replace a misspelled name with the one the diagnostic suggested.
///
/// The suggestion is **read**, never computed: `jr-sema` put it in a `help:` line so that
/// `jr check` gains it too (ADR-0031 §1). If the note is absent the action is not offered,
/// which is correct — no suggestion means nothing was near enough to be worth acting on.
fn did_you_mean(uri: &lsp_types::Uri, diagnostic: &Diagnostic) -> Vec<CodeActionOrCommand> {
    let Some(name) = suggested_name(&diagnostic.message) else {
        return Vec::new();
    };
    vec![quickfix(
        format!("change to `{name}`"),
        uri,
        diagnostic,
        vec![TextEdit {
            range: diagnostic.range,
            new_text: name,
        }],
        true,
    )]
}

/// The name out of a `did you mean \`x\`?` help line.
///
/// The message a client holds is the flattened form `handlers::message_of` built — headline,
/// primary label, then each note prefixed by its severity — so the suggestion is found by
/// its own wording rather than by position.
fn suggested_name(message: &str) -> Option<String> {
    let line = message
        .lines()
        .find(|line| line.trim_start().starts_with("help: did you mean"))?;
    backticked(line)
}

/// The contents of the first pair of backticks in a string.
fn backticked(text: &str) -> Option<String> {
    let start = text.find('`')? + 1;
    let rest = text.get(start..)?;
    let end = rest.find('`')?;
    Some(rest.get(..end)?.to_owned())
}

// ---------------------------------------------------------------------------
// A procedure with no body
// ---------------------------------------------------------------------------

/// Give a bodyless procedure an empty body.
///
/// E0203 is "neither a body nor `#foreign`", and the other repair — making it `#foreign` —
/// is deliberately **not** offered: it needs a library name this action cannot invent, and
/// an action that produces `#foreign ??? "name"` is worse than none.
fn give_a_body(
    db: &dyn Db,
    file: SourceFile,
    positions: &Positions<'_>,
    uri: &lsp_types::Uri,
    diagnostic: &Diagnostic,
) -> Vec<CodeActionOrCommand> {
    // The declaration's span, from HIR rather than from the diagnostic's range: the range
    // is where the error is reported, and the edit has to go at the *end* of the
    // declaration.
    let hir = jr_db::file_hir(db, file);
    let target = hir.procs.iter().find_map(|proc| {
        let range = positions.range(proc.span);
        (proc.body.is_none() && range.start.line == diagnostic.range.start.line).then_some(range)
    });
    let Some(range) = target else {
        return Vec::new();
    };

    // Appended at the end of the declaration. A procedure with no body ends at its `;` or
    // at its return type, and in either case the text a reader wants is ` { }` there.
    let at_end = lsp_types::Range {
        start: range.end,
        end: range.end,
    };
    vec![quickfix(
        String::from("give this procedure an empty body"),
        uri,
        diagnostic,
        vec![TextEdit {
            range: at_end,
            new_text: String::from(" {\n}"),
        }],
        false,
    )]
}

// ---------------------------------------------------------------------------
// `//` to `///`
// ---------------------------------------------------------------------------

/// Turn an ordinary comment above a declaration into documentation.
///
/// A `refactor.rewrite` and not a `quickfix`, because nothing is wrong with the comment.
/// Offered only above a **named** declaration, since that is the only place `///` means
/// anything to `file_docs` (ADR-0027 §2) — above anything else it would be silently
/// dropped, which is an action that appears to do nothing.
fn document_comment(
    db: &dyn Db,
    file: SourceFile,
    positions: &Positions<'_>,
    uri: &lsp_types::Uri,
    text: &str,
    range: lsp_types::Range,
) -> Vec<CodeActionOrCommand> {
    let line_number = range.start.line;
    let Some(line) = text.lines().nth(line_number as usize) else {
        return Vec::new();
    };
    let trimmed = line.trim_start();
    // `///` is already documentation, and `////` is deliberately an ordinary comment
    // (ADR-0027 §1) that this must not silently promote.
    if !trimmed.starts_with("//") || trimmed.starts_with("///") || trimmed.starts_with("//!") {
        return Vec::new();
    }

    // The next non-blank, non-comment line must begin a named declaration.
    let declares = text
        .lines()
        .enumerate()
        .skip(line_number as usize + 1)
        .find(|(_, candidate)| {
            let candidate = candidate.trim_start();
            !candidate.is_empty() && !candidate.starts_with("//")
        })
        .is_some_and(|(index, _)| declaration_on_line(db, file, positions, index as u32));
    if !declares {
        return Vec::new();
    }

    let indent = (line.len() - trimmed.len()) as u32;
    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: String::from("make this comment documentation"),
        kind: Some(CodeActionKind::REFACTOR_REWRITE),
        edit: Some(one_edit(
            uri,
            vec![TextEdit {
                // Just the `//`, so the comment's text is untouched — replacing the whole
                // line would have to reproduce it, and reproducing text is how an edit
                // loses a character.
                range: lsp_types::Range {
                    start: lsp_types::Position {
                        line: line_number,
                        character: indent,
                    },
                    end: lsp_types::Position {
                        line: line_number,
                        character: indent + 2,
                    },
                },
                new_text: String::from("///"),
            }],
        )),
        ..CodeAction::default()
    })]
}

/// Whether a named declaration's name sits on `line`.
fn declaration_on_line(
    db: &dyn Db,
    file: SourceFile,
    positions: &Positions<'_>,
    line: u32,
) -> bool {
    let hir = jr_db::file_hir(db, file);
    hir.items
        .iter()
        .filter(|item| item.name.is_some())
        .any(|item| positions.range(item.name_span).start.line == line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_suggestion_is_read_out_of_the_help_line() {
        // The flattened shape `handlers::message_of` produces.
        let message = "no field `widht` on type `Rect`\nhelp: did you mean `width`?";
        assert_eq!(suggested_name(message), Some(String::from("width")));
    }

    #[test]
    fn a_diagnostic_without_a_suggestion_offers_nothing() {
        let message = "no field `z` on type `Point`";
        assert_eq!(suggested_name(message), None);
        // Specifically not the name from the headline, which is the misspelling itself —
        // an action replacing `z` with `z` would appear to do nothing.
        assert_eq!(backticked(message), Some(String::from("z")));
    }

    #[test]
    fn a_note_that_merely_mentions_a_name_is_not_a_suggestion() {
        // E0212 carries this note as well, and it holds four backticked names.
        let message = "unknown type name `int`\nnote: the builtin types are `s64`, `u8`, \
                       `bool` and `string`";
        assert_eq!(suggested_name(message), None);
    }

    #[test]
    fn deleting_a_line_covers_its_newline() {
        let range = lsp_types::Range {
            start: lsp_types::Position {
                line: 3,
                character: 0,
            },
            end: lsp_types::Position {
                line: 3,
                character: 17,
            },
        };
        let whole = whole_lines(range);
        assert_eq!(whole.start.line, 3);
        assert_eq!(whole.start.character, 0);
        // The start of the *next* line, so no blank line is left behind.
        assert_eq!(whole.end.line, 4);
        assert_eq!(whole.end.character, 0);
    }
}
