//! A miniature front end, so the sema tests can run without `jr-db`.
//!
//! `jr-sema` is a pure function over HIR, exactly like `jr-hir` is a pure
//! function over a syntax tree. That is what lets these tests drive
//! parse → lower → resolve → signatures → check with no salsa database, no
//! filesystem, and no module loader — and it is worth keeping, because a test
//! that needs a database to type `1 + 1` is a test nobody writes.
//!
//! Every test binary in this crate includes this module, and none of them uses
//! all of it, so unused helpers here are expected rather than dead.
#![allow(dead_code)]

use jr_base::{FileId, Interner};
use jr_diag::Diagnostics;
use jr_hir::{FileHir, ItemScope, ResolveMap};
use jr_pool::{Pool, PoolId};
use jr_sema::{FileSignatures, ImportedFile, TypeMap};

/// One analysed file: everything a test might want to assert about.
pub struct Analysis {
    /// The file's HIR.
    pub hir: FileHir,
    /// Its signatures.
    pub signatures: FileSignatures,
    /// Every type sema learned, from both phases.
    pub types: TypeMap,
    /// Diagnostics from sema **only** — never from lexing, parsing, lowering, or
    /// resolution, so that a test can prove sema stayed quiet about someone
    /// else's error.
    pub sema_diagnostics: Diagnostics,
    /// Diagnostics from every earlier phase.
    pub earlier_diagnostics: Diagnostics,
}

impl Analysis {
    /// The codes sema reported, in order.
    pub fn codes(&self) -> Vec<&'static str> {
        self.sema_diagnostics
            .iter()
            .filter_map(|diag| diag.code)
            .collect()
    }

    /// Asserts sema reported nothing at all.
    pub fn assert_silent(&self) {
        assert!(
            self.sema_diagnostics.is_empty(),
            "expected sema to stay silent, got: {:?}",
            self.sema_diagnostics
                .iter()
                .map(|d| format!("{:?} {}", d.code, d.message))
                .collect::<Vec<_>>()
        );
    }

    /// The type of a named file-level declaration.
    pub fn type_of(&self, interner: &Interner, name: &str) -> Option<PoolId> {
        let symbol = interner.get(name)?;
        self.signatures.lookup(symbol).map(|entry| entry.ty)
    }
}

/// The interner, pool, and analyses of a whole test program.
pub struct Program {
    /// The shared interner: symbols must be comparable across files.
    pub interner: Interner,
    /// The shared pool: type identity is only meaningful within one.
    pub pool: Pool,
}

impl Program {
    /// Creates an empty program.
    pub fn new() -> Self {
        Self {
            interner: Interner::new(),
            pool: Pool::new(),
        }
    }

    /// Analyses a single self-contained file.
    pub fn analyse(&mut self, source: &str) -> Analysis {
        self.analyse_with_imports(source, FileId::from_usize(0), &[], &[])
    }

    /// Analyses a file that imports already-analysed modules.
    ///
    /// `modules` carries each module's name, id, HIR and resolution — what the
    /// signature phase needs (ADR-0016 §5) — and `module_signatures` carries the
    /// signatures the check phase needs.
    pub fn analyse_with_imports(
        &mut self,
        source: &str,
        file: FileId,
        modules: &[(&str, FileId, &FileHir, &ResolveMap)],
        module_signatures: &[(&str, &FileSignatures)],
    ) -> Analysis {
        let parsed = jr_syntax::parse(source, file);
        let mut earlier = Diagnostics::new();
        earlier.extend(parsed.diagnostics().iter().cloned());

        let (hir, lower_diags) = jr_hir::lower_file(&parsed, file, &self.interner);
        earlier.extend(lower_diags.iter().cloned());

        // Owned, because `export_scope` now *computes* a filtered scope rather than returning a
        // borrow of the file's own (ADR-0054 §3) — one definition of what a module exports.
        let owned_exports: Vec<(&str, ItemScope)> = modules
            .iter()
            .map(|(name, _, module_hir, _)| (*name, module_hir.export_scope()))
            .collect();
        let exports: Vec<(&str, &ItemScope)> = owned_exports
            .iter()
            .map(|(name, scope)| (*name, scope))
            .collect();
        let (resolve, resolve_diags) = jr_hir::resolve(&hir, &exports, &self.interner);
        earlier.extend(resolve_diags.iter().cloned());

        let imports: Vec<ImportedFile<'_>> = modules
            .iter()
            .map(|(name, id, module_hir, module_resolve)| ImportedFile {
                name,
                file: *id,
                hir: module_hir,
                resolve: module_resolve,
            })
            .collect();

        let signatures = jr_sema::file_signatures(
            &hir,
            file,
            &resolve,
            &imports,
            &mut self.pool,
            &self.interner,
        );

        let checked = jr_sema::check_file(
            &hir,
            file,
            &resolve,
            &signatures.signatures,
            module_signatures,
            // No imported HIRs: an imported *parameterised* struct is out of this harness's scope (ADR-0117 §1).
            &[],
            &mut self.pool,
            &self.interner,
        );

        let mut sema_diagnostics = signatures.diagnostics;
        sema_diagnostics.extend(checked.diagnostics.into_vec());

        let mut types = signatures.types;
        types.absorb(&checked.types);

        Analysis {
            hir,
            signatures: signatures.signatures,
            types,
            sema_diagnostics,
            earlier_diagnostics: earlier,
        }
    }

    /// Analyses a file and returns its resolution as well, so it can be imported.
    pub fn analyse_module(
        &mut self,
        source: &str,
        file: FileId,
    ) -> (FileHir, ResolveMap, FileSignatures) {
        let parsed = jr_syntax::parse(source, file);
        let (hir, _) = jr_hir::lower_file(&parsed, file, &self.interner);
        let (resolve, _) = jr_hir::resolve(&hir, &[], &self.interner);
        let signatures =
            jr_sema::file_signatures(&hir, file, &resolve, &[], &mut self.pool, &self.interner);
        (hir, resolve, signatures.signatures)
    }
}

impl Default for Program {
    fn default() -> Self {
        Self::new()
    }
}
