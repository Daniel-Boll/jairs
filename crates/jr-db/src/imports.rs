//! Which `#import`s a file does not use: the `unused_imports` query.
//!
//! # Why an unused import is a warning in Jairs when it is nothing in Jai
//!
//! [ADR-0031](../../../docs/adr/0031-code-actions-and-hints.md) §3, and it is a
//! language-design position rather than plumbing. Imports here are a **flat merge**
//! (ADR-0014): an unused import does not cost a line, it silently enlarges the name space
//! that every identifier in the file resolves against, and it can turn a later declaration
//! into an E0211 ambiguity from a module the file never uses. That makes it a correctness
//! hazard rather than untidiness, which is what earns it a code.
//!
//! A warning and not an error, because a file part-way through an edit legitimately has
//! one.
//!
//! # The trap this query exists to not fall into
//!
//! §2 of the same ADR. `ResolveMap` maps every `Expr::Name` to what it resolved to — and
//! **only** an `Expr::Name`. A type annotation is a `jr_hir::TypeRef::Name`, resolved
//! separately inside `jr-sema` and absent from the resolve map entirely. So this file:
//!
//! ```jr
//! #import "Shapes";
//!
//! main :: () {
//!     r: Rect;      // a TypeRef::Name — invisible to ResolveMap
//! }
//! ```
//!
//! has an import that a resolve-map-only check calls unused. That file is
//! `tests/corpus/imports/valid/001-import-directory-module.jr`, so the naive version of
//! this query would have shipped a warning telling the user to delete an import their
//! program needs, with a one-click quick fix beside it that breaks the build.
//!
//! The second half of the answer therefore comes from `jr-sema`, which records the module
//! each *type* name resolved from as it resolves it. Re-deriving that here would mean a
//! second copy of ADR-0014 §3's shadowing order, and a divergence between the two copies
//! would present as exactly the false warning above.
//!
//! # Why this is deliberately conservative
//!
//! An import is reported only when it contributes **no** name in either position. An
//! import whose names are all shadowed by this file's own declarations is *not* reported,
//! because "unused" would then depend on a subtlety that is not visible in the import
//! line. Over-reporting here is the harmful direction: the user acts on it.

use std::sync::Arc;

use jr_diag::{Diagnostic, Diagnostics};
use jr_hir::{ItemKind, Res};
use rustc_hash::FxHashSet;

use crate::{
    Db, SourceFile,
    module_loader::{ModuleSearchPaths, file_hir, resolved},
};

/// The diagnostic code for an import nothing in the file uses.
///
/// E0231 was the first free code when this query claimed it, and it is this project's first
/// diagnostic that is a **warning** rather than an error — so a consumer filtering by severity has
/// something to filter.
///
/// The workspace's range table is in `AGENTS.md`, and is deliberately not restated here: this comment
/// used to carry a copy of it, and the three copies (here, `AGENTS.md`, `jr-syntax/src/code.rs`) had
/// drifted apart by the time the audit at `354d900` looked. `crates/jr-cli/tests/codes.rs` now
/// enforces what the copies were trying to convey.
const E0231: &str = "E0231";

/// Every `#import` in a file that nothing in it uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnusedImports {
    /// The unused imports, in source order: the item, the module name, and the span of the
    /// whole `#import` declaration.
    ///
    /// The *whole* declaration rather than the path string, because that is what a "remove
    /// this import" edit has to delete. A span covering only `"Shapes"` would leave
    /// `#import ;` behind.
    pub imports: Vec<UnusedImport>,
}

/// One `#import` nothing uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnusedImport {
    /// The declaring item.
    pub item: jr_hir::ItemId,
    /// The module name, as written in `#import "Name"`.
    pub module: String,
    /// The span of the whole `#import` declaration, which is what removing it deletes.
    pub span: jr_base::Span,
}

impl UnusedImports {
    /// Whether every import in the file is used.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.imports.is_empty()
    }

    /// How many imports are unused.
    #[must_use]
    pub fn len(&self) -> usize {
        self.imports.len()
    }

    /// One warning per unused import.
    ///
    /// Built here rather than at the call site so that `file_diagnostics` and a code action
    /// cannot disagree about the wording — the same reason ADR-0028 §1 keeps one renderer.
    #[must_use]
    pub fn diagnostics(&self) -> Diagnostics {
        let mut out = Diagnostics::new();
        for unused in &self.imports {
            out.push(
                Diagnostic::warning(
                    unused.span,
                    format!(
                        "unused import: nothing in this file uses `{}`",
                        unused.module
                    ),
                )
                .with_code(E0231)
                .with_help("remove it, or use a name it provides"),
            );
        }
        out
    }
}

/// Which of a file's `#import`s nothing in it uses.
///
/// Reads both halves of "used": `ResolveMap`'s `Res::Imported` for expression positions,
/// and `jr-sema`'s record of type-name resolutions for annotation positions. See the module
/// docs for why the second half is not optional.
///
/// A module that could not be found is **never** reported, because E0210 already says so
/// and an import that failed to resolve provides nothing by definition — calling it unused
/// would be a second complaint about one problem, and the wrong one to act on.
#[salsa::tracked(returns(clone))]
pub fn unused_imports(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
) -> Arc<UnusedImports> {
    let hir = file_hir(db, file);
    let resolve = resolved(db, file, search_paths).map;

    // Expression positions: every `Res::Imported` names the `#import` item it came through,
    // which is more direct than matching on the module name.
    let mut used_items: FxHashSet<jr_hir::ItemId> = FxHashSet::default();
    for res in resolve.resolutions.values() {
        if let Res::Imported(import, _) = res {
            used_items.insert(*import);
        }
    }

    // Type positions: `jr-sema` recorded the module name, so this half matches by name.
    // `checked` rather than `file_signatures` because a *local*'s annotation is resolved by
    // the check phase, and a local is exactly the case that motivated this (`r: Rect;`).
    let mut used_modules: FxHashSet<String> = FxHashSet::default();
    let signatures = crate::sema::file_signatures(db, file, search_paths);
    for module in signatures.signatures.modules_used_in_type_position() {
        used_modules.insert(module.to_owned());
    }
    let checked = crate::sema::checked(db, file, search_paths);
    for module in checked.type_name_imports.iter() {
        used_modules.insert(module.clone());
    }

    let own_path = file.path(db);
    let mut imports = Vec::new();
    // Keyed on `(path, alias)`: a bare and an aliased import of one module are two different
    // requests (ADR-0179 §2), so neither makes the other redundant.
    let mut seen: FxHashSet<(&str, Option<jr_base::Symbol>)> = FxHashSet::default();

    for (index, item) in hir.items.iter().enumerate() {
        let ItemKind::Import { path, alias, .. } = &item.kind else {
            continue;
        };
        let id = jr_hir::ItemId::from_usize(index);

        // A duplicate import is idempotent (ADR-0014 §6), and only the *first* occurrence
        // can ever carry a resolution — the import index dedupes by module name before
        // recording one. So a second `#import "Colors";` is reported whether or not the
        // name is used, which is right: it is the line that does nothing.
        let duplicate = !seen.insert((path.as_str(), *alias));

        // A module that does not resolve is E0210's business, not this query's.
        let lookup = crate::module_loader::module_file(db, search_paths, Arc::from(path.as_str()));
        let Some(found) = lookup.found else { continue };
        // A self-import provides nothing and is already skipped everywhere else
        // (ADR-0014 §6); reporting it as unused would be technically true and useless.
        if found.to_string_lossy().as_ref() == own_path.as_ref() {
            continue;
        }

        if !duplicate && (used_items.contains(&id) || used_modules.contains(path.as_str())) {
            continue;
        }

        imports.push(UnusedImport {
            item: id,
            module: path.clone(),
            span: item.span,
        });
    }

    Arc::new(UnusedImports { imports })
}
