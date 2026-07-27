//! Finding a declaration, and everywhere it is used.
//!
//! # Why a definition is not identified by its name
//!
//! [ADR-0030](../../../docs/adr/0030-references-and-rename.md) §1. Jairs imports are a
//! **flat merge** (ADR-0014): there are no qualified paths, so an imported `print` is
//! spelled `print` and nothing about the spelling says which file declared it. Matching by
//! name would therefore conflate two modules' declarations — and the corpus already
//! declares the same names in more than one file. A [`DefId`] names the declaration site.
//!
//! # Why every answer here is a scan
//!
//! `resolved`'s `ResolveMap` maps a use to its declaration. Nothing maps a declaration to
//! its uses; there is no reverse index, and ADR-0030's consequences say plainly that
//! building one means invalidating one, which no measurement yet justifies. So a
//! workspace-wide search loads each file and walks its map.
//!
//! The cost is real and stated: the first such request parses the workspace.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use jr_base::{Span, Symbol};
use jr_db::{Db, ModuleSearchPaths, SourceFile};
use jr_hir::{Expr, ExprScope, FileHir, ItemKind, Res};

/// Which declaration a search is about.
///
/// Carries a path rather than a `SourceFile` so that it can be compared across snapshots
/// and printed in a test failure. That also means it is not a salsa key, which is why
/// these searches are functions rather than tracked queries (ADR-0030 consequences).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefId {
    /// A file-level declaration: procedure, struct, constant, variable or import.
    Item {
        /// The declaring file.
        file: PathBuf,
        /// Which item.
        item: jr_hir::ItemId,
    },
    /// A procedure parameter.
    Param {
        /// The declaring file.
        file: PathBuf,
        /// The declaring procedure.
        proc: jr_hir::ProcId,
        /// Which parameter.
        param: jr_hir::ParamId,
    },
    /// A local variable.
    Local {
        /// The declaring file.
        file: PathBuf,
        /// The declaring body.
        body: jr_hir::BodyId,
        /// Which local.
        local: jr_hir::LocalId,
    },
}

impl DefId {
    /// The file that declares this.
    #[must_use]
    pub fn file(&self) -> &Path {
        match self {
            Self::Item { file, .. } | Self::Param { file, .. } | Self::Local { file, .. } => file,
        }
    }

    /// Whether a search for this can be confined to one file.
    ///
    /// A parameter and a local can only be referenced inside the body that declares them —
    /// `jr-hir` cannot express a reference to another file's local — so scanning the
    /// workspace for one would be pure waste. An item can be imported, so it cannot.
    #[must_use]
    pub fn is_file_local(&self) -> bool {
        match self {
            Self::Item { .. } => false,
            Self::Param { .. } | Self::Local { .. } => true,
        }
    }
}

/// One occurrence of a name that refers to a definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// The file the occurrence is in.
    pub file: PathBuf,
    /// The span of the name token.
    pub span: Span,
    /// `true` when this occurrence *is* the declaration rather than a use of it.
    ///
    /// Kept apart because `references` takes `includeDeclaration` as a parameter, and
    /// because a rename must edit the declaration exactly once however many uses share its
    /// line.
    pub is_declaration: bool,
}

/// The definition the cursor is on, whether it sits on a use or on the declaration itself.
///
/// Tries the resolution first and the declaration site second, in the order and for the
/// reason ADR-0028 §4 fixed for hover: where a name is *used*, following it to what it
/// means is the better answer.
#[must_use]
pub fn definition_at(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    offset: jr_base::TextSize,
) -> Option<DefId> {
    let hir = jr_db::file_hir(db, file);
    let path = PathBuf::from(file.path(db).as_ref());

    if let Some(found) = crate::locate(hir.as_ref(), offset) {
        let res = jr_db::resolved(db, file, search_paths)
            .map
            .get(found.scope, found.expr)?;
        return match res {
            Res::Item(item) => Some(DefId::Item { file: path, item }),
            Res::Imported(import, name) => {
                imported_def(db, hir.as_ref(), search_paths, import, name)
            }
            Res::Param(param) => {
                let ExprScope::Body(body) = found.scope else {
                    return None;
                };
                let proc = owner_of(hir.as_ref(), body)?;
                Some(DefId::Param {
                    file: path,
                    proc,
                    param,
                })
            }
            Res::Local(local) => {
                let ExprScope::Body(body) = found.scope else {
                    return None;
                };
                Some(DefId::Local {
                    file: path,
                    body,
                    local,
                })
            }
            Res::Error => None,
        };
    }

    match crate::locate_declaration(hir.as_ref(), offset)? {
        crate::DeclSite::Item(item) => Some(DefId::Item { file: path, item }),
        crate::DeclSite::Param { proc, param } => Some(DefId::Param {
            file: path,
            proc,
            param,
        }),
        crate::DeclSite::Local { body, local } => Some(DefId::Local {
            file: path,
            body,
            local,
        }),
    }
}

/// The declaring file's `DefId` for a name brought in by an `#import`.
fn imported_def(
    db: &dyn Db,
    hir: &FileHir,
    search_paths: ModuleSearchPaths,
    import: jr_hir::ItemId,
    name: Symbol,
) -> Option<DefId> {
    let ItemKind::Import { path, .. } = &hir.items.get(import.index())?.kind else {
        return None;
    };
    let found = jr_db::module_file(db, search_paths, Arc::from(path.as_str())).found?;
    let module = db.source_file_for_path(found.to_string_lossy().as_ref())?;
    let other = jr_db::file_hir(db, module);
    let item = other.scope.get(name)?;
    Some(DefId::Item { file: found, item })
}

/// Every reference to `def`, across the files given.
///
/// The caller supplies the file set, because who decides it differs per capability:
/// `documentHighlight` passes one file, `references` and `rename` pass the workspace. A
/// file that is not already in the database is loaded from disk; one that cannot be read is
/// skipped, because an unreadable file is not a reason to answer nothing.
#[must_use]
pub fn references(
    db: &dyn Db,
    search_paths: ModuleSearchPaths,
    def: &DefId,
    files: &[PathBuf],
) -> Vec<Reference> {
    let mut out = Vec::new();

    // The declaration itself, which no `ResolveMap` contains: it maps *uses*.
    if let Some(span) = declaration_span(db, def) {
        out.push(Reference {
            file: def.file().to_path_buf(),
            span,
            is_declaration: true,
        });
    }

    let scope: Vec<PathBuf> = if def.is_file_local() {
        vec![def.file().to_path_buf()]
    } else {
        files.to_vec()
    };

    for path in scope {
        let Some(file) = db.source_file_for_path(path.to_string_lossy().as_ref()) else {
            continue;
        };
        let hir = jr_db::file_hir(db, file);
        let resolve = jr_db::resolved(db, file, search_paths).map;
        let here = PathBuf::from(file.path(db).as_ref());

        collect(
            db,
            search_paths,
            def,
            hir.as_ref(),
            &resolve,
            ExprScope::TopLevel,
            &hir.exprs,
            &here,
            &mut out,
        );
        for (index, body) in hir.bodies.iter().enumerate() {
            collect(
                db,
                search_paths,
                def,
                hir.as_ref(),
                &resolve,
                ExprScope::Body(jr_hir::BodyId::from_usize(index)),
                &body.exprs,
                &here,
                &mut out,
            );
        }
    }

    // Two paths can reach the same occurrence — a declaration that is also its own first
    // use does not happen in Jairs, but a file listed twice by a caller would. Cheaper to
    // be idempotent here than to require it of three callers.
    out.sort_by(|a, b| {
        (a.file.clone(), a.span.range.start()).cmp(&(b.file.clone(), b.span.range.start()))
    });
    out.dedup_by(|a, b| a.file == b.file && a.span == b.span);
    out
}

/// Walks one expression arena, pushing every name that resolves to `def`.
#[allow(clippy::too_many_arguments, reason = "a scan needs its whole context")]
fn collect(
    db: &dyn Db,
    search_paths: ModuleSearchPaths,
    def: &DefId,
    hir: &FileHir,
    resolve: &jr_hir::ResolveMap,
    scope: ExprScope,
    exprs: &[Expr],
    file: &Path,
    out: &mut Vec<Reference>,
) {
    for (index, expr) in exprs.iter().enumerate() {
        let Expr::Name { span, .. } = expr else {
            continue;
        };
        let id = jr_hir::ExprId::from_usize(index);
        let Some(res) = resolve.get(scope, id) else {
            continue;
        };
        let found = match res {
            Res::Item(item) => Some(DefId::Item {
                file: file.to_path_buf(),
                item,
            }),
            Res::Imported(import, name) => imported_def(db, hir, search_paths, import, name),
            Res::Param(param) => match scope {
                ExprScope::Body(body) => owner_of(hir, body).map(|proc| DefId::Param {
                    file: file.to_path_buf(),
                    proc,
                    param,
                }),
                ExprScope::TopLevel => None,
            },
            Res::Local(local) => match scope {
                ExprScope::Body(body) => Some(DefId::Local {
                    file: file.to_path_buf(),
                    body,
                    local,
                }),
                ExprScope::TopLevel => None,
            },
            Res::Error => None,
        };
        if found.as_ref() == Some(def) {
            out.push(Reference {
                file: file.to_path_buf(),
                span: *span,
                is_declaration: false,
            });
        }
    }
}

/// The span of the name token that declares `def`.
#[must_use]
pub fn declaration_span(db: &dyn Db, def: &DefId) -> Option<Span> {
    let file = db.source_file_for_path(def.file().to_string_lossy().as_ref())?;
    let hir = jr_db::file_hir(db, file);
    match def {
        DefId::Item { item, .. } => crate::locate::item_name_span(hir.as_ref(), *item),
        DefId::Param { proc, param, .. } => {
            crate::locate::param_name_span(hir.as_ref(), *proc, *param)
        }
        DefId::Local { body, local, .. } => {
            crate::locate::local_name_span(hir.bodies.get(body.index())?, *local)
        }
    }
}

/// The name `def` is declared with.
#[must_use]
pub fn declaration_name(db: &dyn Db, def: &DefId) -> Option<Symbol> {
    let file = db.source_file_for_path(def.file().to_string_lossy().as_ref())?;
    let hir = jr_db::file_hir(db, file);
    match def {
        DefId::Item { item, .. } => hir.items.get(item.index())?.name,
        DefId::Param { proc, param, .. } => {
            Some(hir.procs.get(proc.index())?.params.get(param.index())?.name)
        }
        DefId::Local { body, local, .. } => Some(
            hir.bodies
                .get(body.index())?
                .locals
                .get(local.index())?
                .name,
        ),
    }
}

/// The procedure whose body is `body`.
fn owner_of(hir: &FileHir, body: jr_hir::BodyId) -> Option<jr_hir::ProcId> {
    hir.procs
        .iter()
        .position(|proc| proc.body == Some(body))
        .map(jr_hir::ProcId::from_usize)
}
