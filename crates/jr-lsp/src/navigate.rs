//! References, rename, and symbols.
//!
//! # Why these are together
//!
//! All four are the same traversal seen from different angles
//! ([ADR-0030](../../../docs/adr/0030-references-and-rename.md)): `references` reports
//! `crate::defs::references`, `document_highlight` reports it confined to one file, and
//! `rename` turns it into edits. `document_symbol` is the odd one out — it needs no
//! traversal at all, only `FileHir` reshaped — but it renders through ADR-0028's
//! `render.rs` like everything else, so an outline entry and a hover card cannot disagree.
//!
//! # Why rename would rather do nothing
//!
//! §3 of the ADR. A rename that half-completes leaves a build that does not compile, and a
//! rename that resolves a collision by shadowing leaves one that compiles and means
//! something else. The second is the one a refactoring tool must never produce, so every
//! doubt here is answered with an error response and no edit at all.

use std::path::PathBuf;

use jr_db::{Db, ModuleSearchPaths, SourceFile};
use jr_hir::{ConstValue, FileHir, ItemKind};
use lsp_types::{
    DocumentHighlight, DocumentHighlightKind, DocumentSymbol, Location, PrepareRenameResponse,
    SymbolInformation, SymbolKind, TextEdit, WorkspaceEdit,
};

use crate::defs::{DefId, declaration_name, declaration_span, definition_at, references};
use crate::position::{Encoding, Positions};
use crate::render::{Decl, container_of};

/// Why a rename was refused.
///
/// Each variant is a message a user reads, so each says what to do rather than only what
/// went wrong — a refusal that reads like a bug is worse than no feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameRefusal {
    /// The cursor is not on anything renameable.
    NotRenameable,
    /// The new text is not a legal Jairs identifier.
    NotAnIdentifier(String),
    /// The new name is already declared where the rename would reach.
    Collision {
        /// The name that already exists.
        name: String,
        /// Where it already exists.
        file: PathBuf,
    },
    /// A file that must be edited does not parse.
    ///
    /// Names the file, because otherwise this reads as a bug in the rename rather than as a
    /// syntax error somewhere the user is not looking.
    UnparsedFile(PathBuf),
    /// The workspace file list was truncated, so the search cannot have been exhaustive.
    TruncatedWorkspace,
}

impl std::fmt::Display for RenameRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRenameable => write!(f, "there is nothing to rename here"),
            Self::NotAnIdentifier(text) => write!(
                f,
                "`{text}` is not a valid Jairs identifier, so nothing was renamed"
            ),
            Self::Collision { name, file } => write!(
                f,
                "`{name}` is already declared in {}, and renaming would silently shadow it \
                 rather than fail to compile — nothing was renamed",
                file.display()
            ),
            Self::UnparsedFile(file) => write!(
                f,
                "{} has syntax errors, and an edit computed from a partial parse could \
                 corrupt it — fix it first; nothing was renamed",
                file.display()
            ),
            Self::TruncatedWorkspace => write!(
                f,
                "the workspace has more files than this server will scan, so a rename \
                 cannot be proven complete — nothing was renamed"
            ),
        }
    }
}

/// Every reference to whatever the cursor is on.
#[must_use]
pub fn find_references(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    encoding: Encoding,
    position: lsp_types::Position,
    include_declaration: bool,
    workspace: &[PathBuf],
) -> Vec<Location> {
    let text = file.text(db);
    let index = jr_db::line_index(db, file);
    let positions = Positions::new(text.as_ref(), &index, encoding);
    let Some(def) = definition_at(db, file, search_paths, positions.offset(position)) else {
        return Vec::new();
    };

    references(db, search_paths, &def, &with_open_file(db, file, workspace))
        .into_iter()
        .filter(|found| include_declaration || !found.is_declaration)
        .filter_map(|found| location_of(db, encoding, &found.file, found.span))
        .collect()
}

/// Every occurrence in *this* file, for an editor's cursor-idle highlight.
///
/// Confined deliberately: a client sends this on every cursor move, and a workspace scan
/// per keystroke would be indefensible. The restriction is the feature.
#[must_use]
pub fn document_highlight(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    encoding: Encoding,
    position: lsp_types::Position,
) -> Vec<DocumentHighlight> {
    let text = file.text(db);
    let index = jr_db::line_index(db, file);
    let positions = Positions::new(text.as_ref(), &index, encoding);
    let Some(def) = definition_at(db, file, search_paths, positions.offset(position)) else {
        return Vec::new();
    };
    let here = vec![PathBuf::from(file.path(db).as_ref())];

    references(db, search_paths, &def, &here)
        .into_iter()
        .filter(|found| found.file == here[0])
        .map(|found| DocumentHighlight {
            range: positions.range(found.span),
            kind: Some(if found.is_declaration {
                DocumentHighlightKind::WRITE
            } else {
                DocumentHighlightKind::READ
            }),
        })
        .collect()
}

/// Whether the cursor is on something renameable, and what range would change.
///
/// Refuses *before* the user types a new name, which is the whole value of the request:
/// a keyword, a builtin type name and an unresolved name all look renameable in an editor
/// until the server says otherwise.
#[must_use]
pub fn prepare_rename(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    encoding: Encoding,
    position: lsp_types::Position,
) -> Option<PrepareRenameResponse> {
    let text = file.text(db);
    let index = jr_db::line_index(db, file);
    let positions = Positions::new(text.as_ref(), &index, encoding);
    let offset = positions.offset(position);
    let def = definition_at(db, file, search_paths, offset)?;
    let name = declaration_name(db, &def)?;
    let span = declaration_span(db, &def)?;

    // The range returned is the *occurrence under the cursor*, not the declaration's — a
    // client uses it to seed its input box, and seeding it from another file's span would
    // put the box in the wrong place.
    let here = references(
        db,
        search_paths,
        &def,
        &[PathBuf::from(file.path(db).as_ref())],
    );
    let at_cursor = here
        .iter()
        .find(|found| found.span.range.contains_inclusive(offset))
        .map_or(span, |found| found.span);

    Some(PrepareRenameResponse::RangeWithPlaceholder {
        range: positions.range(at_cursor),
        placeholder: db.interner().resolve(name).to_owned(),
    })
}

/// A workspace-wide rename, or a refusal.
///
/// Returns `Err` with a message the client shows and **no edit**. See [`RenameRefusal`] for
/// the five reasons, and ADR-0030 §3 for why each is a refusal rather than a warning.
pub fn rename(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    encoding: Encoding,
    position: lsp_types::Position,
    new_name: &str,
    workspace: &jr_db::WorkspaceFileList,
) -> Result<WorkspaceEdit, RenameRefusal> {
    if !is_identifier(new_name) {
        return Err(RenameRefusal::NotAnIdentifier(new_name.to_owned()));
    }

    let text = file.text(db);
    let index = jr_db::line_index(db, file);
    let positions = Positions::new(text.as_ref(), &index, encoding);
    let def = definition_at(db, file, search_paths, positions.offset(position))
        .ok_or(RenameRefusal::NotRenameable)?;

    // A file-local rename does not depend on the workspace being complete, so a truncated
    // list refuses only what it actually endangers.
    if !def.is_file_local() && workspace.truncated {
        return Err(RenameRefusal::TruncatedWorkspace);
    }

    let found = references(
        db,
        search_paths,
        &def,
        &with_open_file(db, file, &workspace.files),
    );

    // Every file about to be edited must parse. Checked over the files the edit touches
    // rather than the whole workspace: refusing because an unrelated file is broken would
    // make the feature unusable in a repository mid-edit.
    let mut touched: Vec<PathBuf> = found.iter().map(|r| r.file.clone()).collect();
    touched.sort();
    touched.dedup();
    for path in &touched {
        let Some(source) = db.source_file_for_path(path.to_string_lossy().as_ref()) else {
            continue;
        };
        if !jr_db::parse_file(db, source).diagnostics().is_empty() {
            return Err(RenameRefusal::UnparsedFile(path.clone()));
        }
    }

    if let Some(collision) = collides(db, &def, new_name, &touched) {
        return Err(RenameRefusal::Collision {
            name: new_name.to_owned(),
            file: collision,
        });
    }

    // Grouped by *path* rather than by `Uri`: clippy rightly refuses a `Uri` as a hash key
    // because it has interior mutability, and a key whose hash can change is a key that
    // loses its entry.
    let mut grouped: Vec<(PathBuf, Vec<TextEdit>)> = Vec::new();
    for reference in found {
        let Some(source) = db.source_file_for_path(reference.file.to_string_lossy().as_ref())
        else {
            continue;
        };
        let text = source.text(db);
        let index = jr_db::line_index(db, source);
        let range = Positions::new(text.as_ref(), &index, encoding).range(reference.span);
        let edit = TextEdit {
            range,
            new_text: new_name.to_owned(),
        };
        match grouped.iter_mut().find(|(path, _)| *path == reference.file) {
            Some((_, edits)) => edits.push(edit),
            None => grouped.push((reference.file.clone(), vec![edit])),
        }
    }

    // `WorkspaceEdit::changes` *is* a `HashMap<Uri, _>` in the protocol type, so the
    // lint cannot be designed around: `lsp-types` 0.97's `Uri` wraps `fluent_uri`, whose
    // interior `Cell` is a lazily-computed authority cache and takes no part in `Hash` or
    // `Eq`. Allowed here rather than crate-wide, so a genuinely mutable key elsewhere still
    // fails the build.
    #[allow(
        clippy::mutable_key_type,
        reason = "the protocol's own type; Uri's Cell is a cache, not part of its hash"
    )]
    let mut changes = std::collections::HashMap::new();
    for (path, edits) in grouped {
        if let Some(uri) = crate::uri::from_path(&path) {
            changes.insert(uri, edits);
        }
    }

    Ok(WorkspaceEdit {
        changes: Some(changes),
        ..WorkspaceEdit::default()
    })
}

/// The file in which `new_name` already means something the rename would collide with.
///
/// For an item: any file whose top-level scope already declares the name, among the files
/// the edit touches — because a flat import merge means a name declared in one of them
/// shadows or clashes with the renamed one. For a local or parameter: the declaring body's
/// own locals and its procedure's parameters.
fn collides(db: &dyn Db, def: &DefId, new_name: &str, touched: &[PathBuf]) -> Option<PathBuf> {
    let symbol = db.interner().get(new_name)?;

    match def {
        DefId::Item { .. } => {
            for path in touched {
                let source = db.source_file_for_path(path.to_string_lossy().as_ref())?;
                let hir = jr_db::file_hir(db, source);
                if hir.scope.get(symbol).is_some() {
                    return Some(path.clone());
                }
            }
            None
        }
        DefId::Param { file, proc, .. } => {
            let source = db.source_file_for_path(file.to_string_lossy().as_ref())?;
            let hir = jr_db::file_hir(db, source);
            let owner = hir.procs.get(proc.index())?;
            let clash = owner.params.iter().any(|p| p.name == symbol)
                || owner.body.is_some_and(|body| {
                    hir.bodies
                        .get(body.index())
                        .is_some_and(|b| b.locals.iter().any(|l| l.name == symbol))
                });
            clash.then(|| file.clone())
        }
        DefId::Local { file, body, .. } => {
            let source = db.source_file_for_path(file.to_string_lossy().as_ref())?;
            let hir = jr_db::file_hir(db, source);
            let owner = hir.bodies.get(body.index())?;
            owner
                .locals
                .iter()
                .any(|l| l.name == symbol)
                .then(|| file.clone())
        }
    }
}

/// The scan set, guaranteed to include the file being edited.
///
/// Discovery can legitimately miss the open file: a client may send no `workspaceFolders`,
/// or the user may open a scratch file outside the tree. Without this the scan would cover
/// zero files and `references` would return only the declaration — a confident wrong answer
/// rather than a visible failure, which is the shape of bug ADR-0029 §3 warns about.
fn with_open_file(db: &dyn Db, file: SourceFile, workspace: &[PathBuf]) -> Vec<PathBuf> {
    let here = PathBuf::from(file.path(db).as_ref());
    let mut files = workspace.to_vec();
    if !files.contains(&here) {
        files.push(here);
    }
    files
}

/// Whether `text` is a legal Jairs identifier.
///
/// Deliberately strict about the *first* character: `2x` lexes as a number followed by an
/// identifier, so accepting it would produce a file that no longer parses.
fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A file's outline.
///
/// Struct fields nest under their struct; parameters do **not** nest under a procedure,
/// because the signature `detail` already lists them and nesting makes an outline
/// unreadable (ADR-0030 §4).
#[must_use]
pub fn document_symbol(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    encoding: Encoding,
) -> Vec<DocumentSymbol> {
    let text = file.text(db);
    let index = jr_db::line_index(db, file);
    let positions = Positions::new(text.as_ref(), &index, encoding);
    let hir = jr_db::file_hir(db, file);
    let sigs = jr_db::file_signatures(db, file, search_paths).signatures;
    let docs = jr_db::file_docs(db, file);
    let container = container_of(file.path(db).as_ref());
    let pool = db.pool().lock().unwrap_or_else(|e| e.into_inner());
    let decl = Decl {
        hir: hir.as_ref(),
        sigs: sigs.as_ref(),
        docs: docs.as_ref(),
        consts: None,
        pool: &pool,
        interner: db.interner(),
        container: &container,
    };

    let mut out = Vec::new();
    for (index, item) in hir.items.iter().enumerate() {
        let Some(name) = item.name else { continue };
        let id = jr_hir::ItemId::from_usize(index);
        let kind = symbol_kind(&item.kind);

        #[expect(
            deprecated,
            reason = "lsp-types marks `deprecated` on DocumentSymbol as deprecated by the \
                      protocol, but the struct has no non-exhaustive constructor"
        )]
        let symbol = DocumentSymbol {
            name: db.interner().resolve(name).to_owned(),
            detail: decl.signature(id),
            kind,
            tags: None,
            deprecated: None,
            range: positions.range(item.span),
            selection_range: positions.range(item.name_span),
            children: struct_children(db, hir.as_ref(), &positions, &item.kind),
        };
        out.push(symbol);
    }
    out
}

/// The fields of a struct declaration, as child symbols.
fn struct_children(
    db: &dyn Db,
    hir: &FileHir,
    positions: &Positions<'_>,
    kind: &ItemKind,
) -> Option<Vec<DocumentSymbol>> {
    let ItemKind::Const {
        value: ConstValue::Struct(id),
    } = kind
    else {
        return None;
    };
    let fields = &hir.structs.get(id.index())?.fields;
    if fields.is_empty() {
        return None;
    }

    #[expect(
        deprecated,
        reason = "see `document_symbol`: the protocol deprecates the field, lsp-types keeps it"
    )]
    Some(
        fields
            .iter()
            .map(|field| DocumentSymbol {
                name: db.interner().resolve(field.name).to_owned(),
                detail: None,
                kind: SymbolKind::FIELD,
                tags: None,
                deprecated: None,
                range: positions.range(field.name_span),
                selection_range: positions.range(field.name_span),
                children: None,
            })
            .collect(),
    )
}

/// Every declaration in the workspace whose name contains `query`.
///
/// Proceeds on a truncated file list, unlike rename: a partial outline is still useful,
/// where a partial rename is a broken build (ADR-0029 §4).
#[must_use]
pub fn workspace_symbol(
    db: &dyn Db,
    search_paths: ModuleSearchPaths,
    encoding: Encoding,
    query: &str,
    workspace: &[PathBuf],
) -> Vec<SymbolInformation> {
    let query = query.to_lowercase();
    let mut out = Vec::new();

    for path in workspace {
        let Some(file) = db.source_file_for_path(path.to_string_lossy().as_ref()) else {
            continue;
        };
        let hir = jr_db::file_hir(db, file);
        let sigs = jr_db::file_signatures(db, file, search_paths).signatures;
        let docs = jr_db::file_docs(db, file);
        let container = container_of(path.to_string_lossy().as_ref());
        let text = file.text(db);
        let index = jr_db::line_index(db, file);
        let positions = Positions::new(text.as_ref(), &index, encoding);
        // No `Decl` here: `SymbolInformation` has no `detail` field, so rendering a
        // signature would be work whose result the protocol discards. `document_symbol`
        // does have one and does render.
        let _ = (&sigs, &docs);

        for item in &hir.items {
            let Some(name) = item.name else { continue };
            let text = db.interner().resolve(name);
            // Empty query means "everything", which is what a client sends to populate a
            // picker before the user types.
            if !query.is_empty() && !text.to_lowercase().contains(&query) {
                continue;
            }
            let Some(uri) = crate::uri::from_path(path) else {
                continue;
            };

            #[expect(
                deprecated,
                reason = "SymbolInformation is protocol-deprecated in favour of \
                          WorkspaceSymbol, but is what lsp-types' response type carries"
            )]
            out.push(SymbolInformation {
                name: text.to_owned(),
                kind: symbol_kind(&item.kind),
                tags: None,
                deprecated: None,
                location: Location {
                    uri,
                    range: positions.range(item.name_span),
                },
                container_name: Some(container.clone()),
            });
        }
    }
    out
}

/// The protocol's symbol kind for an item.
fn symbol_kind(kind: &ItemKind) -> SymbolKind {
    match kind {
        ItemKind::Const {
            value: ConstValue::Proc(_),
        } => SymbolKind::FUNCTION,
        ItemKind::Const {
            value: ConstValue::Struct(_),
        } => SymbolKind::STRUCT,
        ItemKind::Const {
            value: ConstValue::Expr(_),
        } => SymbolKind::CONSTANT,
        ItemKind::Var { .. } => SymbolKind::VARIABLE,
        ItemKind::Import { .. } => SymbolKind::MODULE,
        ItemKind::Run { .. } => SymbolKind::EVENT,
    }
}

/// A `Location` in an arbitrary file, using *that* file's line index.
///
/// Converting a span from one file with another file's lines produces a plausible wrong
/// location, which is worse than none — the same rule `goto_definition` follows.
fn location_of(
    db: &dyn Db,
    encoding: Encoding,
    path: &std::path::Path,
    span: jr_base::Span,
) -> Option<Location> {
    let file = db.source_file_for_path(path.to_string_lossy().as_ref())?;
    let text = file.text(db);
    let index = jr_db::line_index(db, file);
    Some(Location {
        uri: crate::uri::from_path(path)?,
        range: Positions::new(text.as_ref(), &index, encoding).range(span),
    })
}
