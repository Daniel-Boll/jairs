//! The three capabilities `PLAN.md` §1.4 asks for, as pure functions.
//!
//! # Why they are functions and not methods on a server
//!
//! [ADR-0024](../../../docs/adr/0024-language-server.md) §4: a handler takes a database
//! and typed parameters and returns a response, with no I/O and no `self`. That is what
//! lets `tests/handlers.rs` assert on a hover without a transport, a subprocess or a
//! client — and it is why [`crate::server`] can be a thin shell whose only job is
//! framing and threading.
//!
//! The separate stdio smoke test exists because these tests would pass with a completely
//! broken transport. That is not a hypothetical: the first native run of `024-hello.jr`
//! printed both its lines perfectly and exited **1**, and no in-process assertion
//! noticed.
//!
//! # Why none of them analyses anything
//!
//! ADR-0007's claim is that the LSP is a *consumer* of the same salsa queries as the
//! batch compiler, not a second front end, and this module is where that claim is either
//! true or false. [`diagnostics()`] is `jr_db::file_diagnostics` reshaped. [`hover()`] is
//! `jr_db::checked`'s `TypeMap`, looked up. [`goto_definition()`] is `jr_db::resolved`'s
//! `ResolveMap`, followed. The only thing here that is not a query is
//! [`crate::locate()`], and that is because ADR-0013 deferred the structure that would
//! make it one.
//!
//! If a capability ever needs a fact no query produces, the fix belongs in `jr-db`.

use std::sync::Arc;

use jr_db::{Db, ModuleSearchPaths, SourceFile};
use jr_hir::{ExprScope, FileHir, ItemKind, Res};
use lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Hover, HoverContents, Location,
    MarkupContent, MarkupKind, NumberOrString,
};

use crate::locate::{
    DeclSite, Located, item_name_span, local_name_span, locate, locate_declaration, param_name_span,
};
use crate::position::{Encoding, Positions};
use crate::render::{Card, Decl, binding_card, container_of, type_name};

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// Every diagnostic for one file, as the protocol wants them.
///
/// A `jr-diag` diagnostic can point at spans in *other* files — an import error names
/// the module — so a secondary label outside this file becomes
/// `relatedInformation` rather than being dropped or, worse, rendered at whatever
/// offset it happens to hold in this one.
#[must_use]
pub fn diagnostics(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    encoding: Encoding,
) -> Vec<Diagnostic> {
    let text = file.text(db);
    let index = jr_db::line_index(db, file);
    let positions = Positions::new(text.as_ref(), &index, encoding);
    let map = db.source_map();
    let this = map.file_id(file.path(db).as_ref());

    jr_db::file_diagnostics(db, file, search_paths)
        .iter()
        .map(|diag| Diagnostic {
            range: positions.range(diag.primary.span),
            severity: Some(severity(diag.severity)),
            code: diag
                .code
                .map(|code| NumberOrString::String(code.to_owned())),
            source: Some(String::from("jairs")),
            message: message_of(diag),
            related_information: related(diag, this, &map),
            ..Diagnostic::default()
        })
        .collect()
}

/// The headline plus whatever the primary label and the notes add.
///
/// Flattened into one string because the protocol has one message field and a client
/// renders it as a block. Dropping the notes would lose the half of a `jr-diag`
/// diagnostic that explains what to do — E0230's note, for instance, is the only place
/// that says `#run` is evaluated in the bytecode VM.
fn message_of(diag: &jr_diag::Diagnostic) -> String {
    let mut out = diag.message.clone();
    if let Some(label) = &diag.primary.message {
        out.push_str("\n  ");
        out.push_str(label);
    }
    for (severity, note) in &diag.notes {
        out.push('\n');
        out.push_str(match severity {
            jr_diag::Severity::Help => "help: ",
            jr_diag::Severity::Note => "note: ",
            jr_diag::Severity::Warning => "warning: ",
            jr_diag::Severity::Error => "error: ",
        });
        out.push_str(note);
    }
    out
}

/// Secondary labels, as related information.
///
/// Only labels in *other* files become related information; a secondary label in this
/// file is already visible in the same buffer, and duplicating it clutters the list
/// clients render in the problems panel.
fn related(
    diag: &jr_diag::Diagnostic,
    this: Option<jr_base::FileId>,
    map: &jr_base::SourceMap,
) -> Option<Vec<DiagnosticRelatedInformation>> {
    let out: Vec<DiagnosticRelatedInformation> = diag
        .secondary
        .iter()
        .filter(|label| Some(label.span.file) != this)
        .filter_map(|label| {
            let url = crate::uri::from_path(map.file(label.span.file).path())?;
            Some(DiagnosticRelatedInformation {
                location: Location {
                    uri: url,
                    // A span in another file needs that file's line index to convert
                    // exactly. Rather than load it, point at the start of the line the
                    // source map already knows: an approximate location in a file the
                    // user is not looking at is better than none, and better than a
                    // wrong one computed from this file's lines.
                    range: lsp_types::Range::default(),
                },
                message: label
                    .message
                    .clone()
                    .unwrap_or_else(|| String::from("related")),
            })
        })
        .collect();
    // `None` rather than an empty vector: a client renders an empty list as a stub
    // expander, and there is nothing behind it.
    (!out.is_empty()).then_some(out)
}

fn severity(severity: jr_diag::Severity) -> DiagnosticSeverity {
    match severity {
        jr_diag::Severity::Error => DiagnosticSeverity::ERROR,
        jr_diag::Severity::Warning => DiagnosticSeverity::WARNING,
        jr_diag::Severity::Note => DiagnosticSeverity::INFORMATION,
        jr_diag::Severity::Help => DiagnosticSeverity::HINT,
    }
}

// ---------------------------------------------------------------------------
// Hover
// ---------------------------------------------------------------------------

/// The declaration under the cursor, or failing that its type.
///
/// ADR-0028 §4 fixes the order: **resolve first**, render the type only when the cursor
/// is not on a name. The old implementation did the second half only, which is why a
/// procedure hovered as `(s64, s64) -> s64` — for a procedure the *type* is the
/// signature shape, so the name, the parameter names and the origin were never in scope
/// to be lost.
///
/// `None` for whitespace, a comment, a brace, or a token lowering did not turn into an
/// expression. That is a real answer: an editor showing nothing there is correct.
#[must_use]
pub fn hover(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    encoding: Encoding,
    position: lsp_types::Position,
) -> Option<Hover> {
    let text = file.text(db);
    let index = jr_db::line_index(db, file);
    let positions = Positions::new(text.as_ref(), &index, encoding);
    let offset = positions.offset(position);

    let hir = jr_db::file_hir(db, file);

    // A name *used* as an expression resolves to what it means, which is the better
    // answer; a declaration's own name token is not an expression at all and is checked
    // only when that fails. ADR-0028 §4.
    if let Some(found) = locate(hir.as_ref(), offset) {
        let card = declaration_card(db, file, search_paths, hir.as_ref(), &found)
            .or_else(|| type_card(db, file, search_paths, hir.as_ref(), &found))?;
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: card.to_markdown(),
            }),
            range: Some(positions.range(found.span)),
        });
    }

    let site = locate_declaration(hir.as_ref(), offset)?;
    let (card, span) = declaration_site_card(db, file, search_paths, hir.as_ref(), site)?;
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: card.to_markdown(),
        }),
        range: Some(positions.range(span)),
    })
}

/// The card for a declaration hovered at its own name.
///
/// Returns the name's span as well, so the client highlights the name rather than the
/// whole declaration — hovering `add` should not light up its entire body.
fn declaration_site_card(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    hir: &FileHir,
    site: DeclSite,
) -> Option<(Card, jr_base::Span)> {
    let sigs = jr_db::file_signatures(db, file, search_paths).signatures;
    let container = container_of(file.path(db).as_ref());

    match site {
        DeclSite::Item(item) => {
            let docs = jr_db::file_docs(db, file);
            let consts = jr_db::file_consts(db, file, search_paths).values;
            let span = item_name_span(hir, item)?;
            let pool = db.read_pool();
            let card = Decl {
                hir,
                sigs: sigs.as_ref(),
                docs: docs.as_ref(),
                consts: Some(consts.as_ref()),
                pool: &pool,
                interner: db.interner(),
                container: &container,
            }
            .card(item)?;
            Some((card, span))
        }
        // An import's card is built by `import_card` rather than by `Decl`, because none of
        // `Decl`'s inputs apply: an import has no type, no signature and no `///` of its own
        // (ADR-0035 §2). Note this reaches for the *module's* docs, not this file's — the
        // `//!` block being shown belongs to the file being imported.
        DeclSite::Import(item) => {
            let jr_hir::ItemKind::Import { path, .. } = &hir.items.get(item.index())?.kind else {
                return None;
            };
            let span = hir.items.get(item.index())?.span;
            let found = jr_db::module_file(db, search_paths, Arc::from(path.as_str())).found;
            // The module's own `//!`, when the file is loaded. A discovered-but-unloaded
            // module yields no docs rather than loading it here: a hover must not be the
            // thing that pulls a file into the database.
            let docs = found
                .as_deref()
                .and_then(|path| db.source_file_for_path(path.to_string_lossy().as_ref()))
                .map(|module| jr_db::file_docs(db, module));
            let card = crate::render::import_card(
                path,
                found.as_deref(),
                docs.as_ref().and_then(|docs| docs.module()),
            );
            Some((card, span))
        }
        DeclSite::Param { proc, param } => {
            let span = param_name_span(hir, proc, param)?;
            let name = hir.procs.get(proc.index())?.params.get(param.index())?.name;
            let ty = sigs
                .proc_sig(proc)
                .and_then(|sig| sig.params.get(param.index()).copied());
            let pool = db.read_pool();
            let rendered = ty.map(|ty| type_name(&pool, sigs.as_ref(), ty));
            Some((
                binding_card(&container, db.interner().resolve(name), rendered),
                span,
            ))
        }
        DeclSite::Local { body, local } => {
            let types = jr_db::checked(db, file, search_paths).types;
            let span = local_name_span(hir.bodies.get(body.index())?, local)?;
            let name = hir
                .bodies
                .get(body.index())?
                .locals
                .get(local.index())?
                .name;
            let ty = types.local_type(body, local);
            let pool = db.read_pool();
            let rendered = ty.map(|ty| type_name(&pool, sigs.as_ref(), ty));
            Some((
                binding_card(&container, db.interner().resolve(name), rendered),
                span,
            ))
        }
    }
}

/// The card for whatever the name under the cursor resolves to.
///
/// Every arm of `Res` is handled, including `Res::Imported`, which renders the *other*
/// file's declaration using the *other* file's HIR, signatures and docs. Rendering it
/// with this file's would produce a plausible wrong card, which is worse than none.
fn declaration_card(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    hir: &FileHir,
    found: &Located,
) -> Option<Card> {
    let res = jr_db::resolved(db, file, search_paths)
        .map
        .get(found.scope, found.expr)?;

    match res {
        Res::Item(item) => {
            let sigs = jr_db::file_signatures(db, file, search_paths).signatures;
            let docs = jr_db::file_docs(db, file);
            let consts = jr_db::file_consts(db, file, search_paths).values;
            let container = container_of(file.path(db).as_ref());
            let pool = db.read_pool();
            Decl {
                hir,
                sigs: sigs.as_ref(),
                docs: docs.as_ref(),
                consts: Some(consts.as_ref()),
                pool: &pool,
                interner: db.interner(),
                container: &container,
            }
            .card(item)
        }
        Res::Imported(import, name) => imported_card(db, hir, search_paths, import, name),
        // A parameter or a local has no documentation and no container of its own, so
        // its card is its declared type. Better than the type alone: the name confirms
        // which binding the cursor found, which matters where one shadows another.
        // A promoted name hovers as its own type, like any other binding (ADR-0050 §2). Sharing
        // this arm is right rather than convenient: what the reader wants is the type of the
        // *name under the cursor*, and `expr_type` already answers that for a promoted name
        // because sema typed it through its base.
        Res::Param(_) | Res::Local(_) | Res::Promoted { .. } => {
            let types = jr_db::checked(db, file, search_paths).types;
            let ty = types.expr_type(found.scope, found.expr)?;
            let sigs = jr_db::file_signatures(db, file, search_paths).signatures;
            let name = name_at(hir, found)?;
            let container = container_of(file.path(db).as_ref());
            let pool = db.read_pool();
            Some(binding_card(
                &container,
                db.interner().resolve(name),
                Some(type_name(&pool, sigs.as_ref(), ty)),
            ))
        }
        Res::Error => None,
    }
}

/// The card for a declaration in an imported module.
///
/// Resolved through `module_file` and the other file's own queries, the same way
/// [`goto_definition`] does it and for the same reason (ADR-0014 §4).
fn imported_card(
    db: &dyn Db,
    hir: &FileHir,
    search_paths: ModuleSearchPaths,
    import: jr_hir::ItemId,
    name: jr_base::Symbol,
) -> Option<Card> {
    let ItemKind::Import { path, .. } = &hir.items.get(import.index())?.kind else {
        return None;
    };
    let lookup = jr_db::module_file(db, search_paths, Arc::from(path.as_str()));
    let found = lookup.found?;
    let module = db.source_file_for_path(found.to_string_lossy().as_ref())?;

    let other = jr_db::file_hir(db, module);
    let item = other.scope.get(name)?;
    let sigs = jr_db::file_signatures(db, module, search_paths).signatures;
    let docs = jr_db::file_docs(db, module);
    let container = container_of(found.to_string_lossy().as_ref());
    let pool = db.read_pool();

    Decl {
        hir: other.as_ref(),
        sigs: sigs.as_ref(),
        docs: docs.as_ref(),
        // Deliberately not fetched: computing another file's constants to render a
        // hover would make reading one file evaluate another's `#run`.
        consts: None,
        pool: &pool,
        interner: db.interner(),
        container: &container,
    }
    .card(item)
}

/// The fallback: the type of an expression that is not a name.
///
/// `4 + 5`, a literal, a field access. There is no declaration to show, so the card
/// carries the type alone and no signature line beyond it.
fn type_card(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    _hir: &FileHir,
    found: &Located,
) -> Option<Card> {
    let types = jr_db::checked(db, file, search_paths).types;
    let ty = types.expr_type(found.scope, found.expr)?;
    let signatures = jr_db::file_signatures(db, file, search_paths).signatures;
    let container = container_of(file.path(db).as_ref());
    let pool = db.read_pool();

    Some(Card {
        container,
        signature: type_name(&pool, signatures.as_ref(), ty),
        docs: None,
    })
}

/// The name an `Expr::Name` node holds, for rendering a binding's card.
fn name_at(hir: &FileHir, found: &Located) -> Option<jr_base::Symbol> {
    let expr = match found.scope {
        ExprScope::TopLevel => hir.exprs.get(found.expr.index())?,
        ExprScope::Body(body) => hir
            .bodies
            .get(body.index())?
            .exprs
            .get(found.expr.index())?,
    };
    match expr {
        jr_hir::Expr::Name { name, .. } => Some(*name),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Goto definition
// ---------------------------------------------------------------------------

/// Where the name under the cursor was declared.
///
/// Handles all four resolutions `jr-hir` can produce, including `Res::Imported`, which
/// crosses into another file — because goto-definition on `print` landing in
/// `modules/Basic` is the one that demonstrates the module system actually resolved.
#[must_use]
pub fn goto_definition(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    encoding: Encoding,
    position: lsp_types::Position,
) -> Option<Location> {
    let text = file.text(db);
    let index = jr_db::line_index(db, file);
    let positions = Positions::new(text.as_ref(), &index, encoding);
    let offset = positions.offset(position);

    let hir = jr_db::file_hir(db, file);

    // An `#import` line is checked first, and before `locate`, because it is not an
    // expression at all — there is nothing on that line for a resolve map to hold. Until
    // ADR-0035 this handler consulted only `locate`, so goto-definition on the one
    // declaration in the language that names another *file* answered nothing at every
    // column, including on the module name itself.
    if let Some(target) = import_target(db, hir.as_ref(), search_paths, encoding, offset) {
        return Some(target);
    }

    let found = locate(hir.as_ref(), offset)?;
    let resolve = jr_db::resolved(db, file, search_paths).map;
    let res = resolve.get(found.scope, found.expr)?;

    match res {
        Res::Local(local) => {
            let ExprScope::Body(body) = found.scope else {
                return None;
            };
            let span = local_name_span(hir.bodies.get(body.index())?, local)?;
            Some(here(db, file, &positions, span))
        }
        Res::Param(param) => {
            let ExprScope::Body(body) = found.scope else {
                return None;
            };
            // A parameter's span lives on `Proc::params`, not in the body — `jr-hir`
            // does not store parameters as locals at all. So the owning procedure has
            // to be found by which one declares this body.
            let proc = owner_of(hir.as_ref(), body)?;
            let span = param_name_span(hir.as_ref(), proc, param)?;
            Some(here(db, file, &positions, span))
        }
        Res::Item(item) => {
            let span = item_name_span(hir.as_ref(), item)?;
            Some(here(db, file, &positions, span))
        }
        Res::Imported(import, name) => {
            imported(db, hir.as_ref(), search_paths, encoding, import, name)
        }
        // Highlighting a promoted name highlights the binding it reaches through, matching where
        // goto-definition sends the reader — the two must agree or the editor contradicts itself.
        Res::Promoted { .. } => {
            let mut target = &res;
            while let Res::Promoted { base, .. } = target {
                target = base;
            }
            let ExprScope::Body(body) = found.scope else {
                return None;
            };
            match target {
                Res::Local(local) => {
                    let span = local_name_span(hir.bodies.get(body.index())?, *local)?;
                    Some(here(db, file, &positions, span))
                }
                Res::Param(param) => {
                    let proc = owner_of(hir.as_ref(), body)?;
                    let span = param_name_span(hir.as_ref(), proc, *param)?;
                    Some(here(db, file, &positions, span))
                }
                Res::Item(_) | Res::Imported(_, _) | Res::Promoted { .. } | Res::Error => None,
            }
        }
        Res::Error => None,
    }
}

/// The module file an `#import` under the cursor names, as a location.
///
/// The whole `#import "Basic";` declaration is the target, not just the path string: the line
/// has one meaning and no sub-parts worth distinguishing, so a cursor anywhere on it means the
/// same thing (ADR-0035 §1).
///
/// The destination is the **start of the file**, because a module is a file (ADR-0014 §1) and
/// there is no "definition of a module" to land on. Landing on its first declaration would be
/// an arbitrary choice the user did not ask for.
///
/// `None` for a module that does not resolve — E0210 already reports that, and pointing at
/// where the file *would* be would open an empty buffer at a path nobody chose (§3).
fn import_target(
    db: &dyn Db,
    hir: &FileHir,
    search_paths: ModuleSearchPaths,
    encoding: Encoding,
    offset: jr_base::TextSize,
) -> Option<Location> {
    let DeclSite::Import(item) = locate_declaration(hir, offset)? else {
        return None;
    };
    let ItemKind::Import { path, .. } = &hir.items.get(item.index())?.kind else {
        return None;
    };
    let found = jr_db::module_file(db, search_paths, Arc::from(path.as_str())).found?;
    // The encoding is irrelevant to a zero range, but taken as a parameter anyway so that
    // this cannot silently become the one place that ignores the negotiated encoding if it
    // ever points somewhere other than the start of the file.
    let _ = encoding;
    Some(Location {
        uri: crate::uri::from_path(&found)?,
        range: lsp_types::Range::default(),
    })
}

/// The procedure whose body is `body`.
///
/// Linear, because nothing stores the reverse edge. Cheap: a file has few procedures,
/// and this runs once per goto-definition on a parameter.
fn owner_of(hir: &FileHir, body: jr_hir::BodyId) -> Option<jr_hir::ProcId> {
    hir.procs
        .iter()
        .position(|proc| proc.body == Some(body))
        .map(jr_hir::ProcId::from_usize)
}

/// A location in the file being edited.
fn here(db: &dyn Db, file: SourceFile, positions: &Positions<'_>, span: jr_base::Span) -> Location {
    let path = file.path(db);
    Location {
        uri: crate::uri::from_path(std::path::Path::new(path.as_ref()))
            .unwrap_or_else(|| "file:///".parse().expect("a valid fallback uri")),
        range: positions.range(span),
    }
}

/// A definition in an imported module.
///
/// Resolved the same way `jr-db`'s `imported_procs` does — through `module_file` and the
/// other file's HIR, and *only* its HIR. That is what ADR-0014 §4 requires and what
/// keeps one file's analysis off another file's.
fn imported(
    db: &dyn Db,
    hir: &FileHir,
    search_paths: ModuleSearchPaths,
    encoding: Encoding,
    import: jr_hir::ItemId,
    name: jr_base::Symbol,
) -> Option<Location> {
    let ItemKind::Import { path, .. } = &hir.items.get(import.index())?.kind else {
        return None;
    };
    let lookup = jr_db::module_file(db, search_paths, Arc::from(path.as_str()));
    let found = lookup.found?;
    let module = db.source_file_for_path(found.to_string_lossy().as_ref())?;

    let other = jr_db::file_hir(db, module);
    let item = other.scope.get(name)?;
    let span = item_name_span(other.as_ref(), item)?;

    // The other file's own text and line index, deliberately: converting a span from
    // one file using another file's lines produces a plausible wrong location, which is
    // worse than no location at all.
    let text = module.text(db);
    let index = jr_db::line_index(db, module);
    let range = Positions::new(text.as_ref(), &index, encoding).range(span);

    Some(Location {
        uri: crate::uri::from_path(&found)?,
        range,
    })
}
