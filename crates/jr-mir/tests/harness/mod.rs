//! A miniature front end, so the MIR tests can run without `jr-db`.
//!
//! `jr-mir` is a pure function over HIR plus `jr-sema`'s output, exactly as
//! `jr-sema` is a pure function over HIR. That is what lets these tests drive
//! parse → lower → resolve → signatures → check → **mir** with no salsa
//! database, no filesystem and no module loader. `jr-sema`'s own harness makes
//! the same argument and this one is deliberately its sibling, extended by one
//! stage and by keeping the `ResolveMap`, which lowering needs and sema's
//! harness throws away.
//!
//! Every test binary in this crate includes this module and none of them uses
//! all of it, so unused helpers here are expected rather than dead.
#![allow(dead_code)]

use jr_base::{FileId, Interner};
use jr_diag::Diagnostics;
use jr_hir::{ConstValue, FileHir, ItemKind, ProcId, ResolveMap};
use jr_mir::{FileMir, MirBody, Poisoned};
use jr_pool::Pool;
use jr_sema::{FileSignatures, TypeMap};

/// One file, lowered all the way to MIR.
pub struct Lowered {
    /// The file's HIR.
    pub hir: FileHir,
    /// Its name resolution.
    pub resolve: ResolveMap,
    /// Its signatures.
    pub signatures: FileSignatures,
    /// Every type sema learned, from both phases.
    pub types: TypeMap,
    /// The MIR for every procedure with a body.
    pub mir: FileMir,
    /// Diagnostics from every phase before MIR.
    ///
    /// Kept separate so a test can prove MIR stayed silent about someone else's
    /// error — the same separation `jr-sema`'s harness keeps, and for the same
    /// reason. `jr-mir` raises no diagnostics at all, so there is deliberately no
    /// field for its own.
    pub earlier_diagnostics: Diagnostics,
}

impl Lowered {
    /// The [`ProcId`] of a named procedure.
    ///
    /// A procedure carries no name of its own (ADR-0012 makes procedures
    /// constants), so the name lives on the item whose `ItemKind::Const` holds it.
    pub fn proc_id(&self, interner: &Interner, name: &str) -> Option<ProcId> {
        let symbol = interner.get(name)?;
        self.hir.items.iter().find_map(|item| {
            let ItemKind::Const {
                value: ConstValue::Proc(proc),
            } = &item.kind
            else {
                return None;
            };
            (item.name == Some(symbol)).then_some(*proc)
        })
    }

    /// The outcome for a named procedure.
    pub fn outcome(&self, interner: &Interner, name: &str) -> &Result<MirBody, Poisoned> {
        let proc = self.proc_id(interner, name).expect("no such procedure");
        self.mir.get(proc).expect("the procedure has no body")
    }

    /// The MIR of a named procedure, asserting it lowered.
    pub fn body(&self, interner: &Interner, name: &str) -> &MirBody {
        match self.outcome(interner, name) {
            Ok(body) => body,
            Err(poison) => panic!("expected `{name}` to lower, but it was refused: {poison:?}"),
        }
    }

    /// Why a named procedure was refused, asserting it was.
    pub fn refusal(&self, interner: &Interner, name: &str) -> Poisoned {
        match self.outcome(interner, name) {
            Ok(_) => panic!("expected `{name}` to be refused, but it lowered"),
            Err(poison) => *poison,
        }
    }
}

/// The interner and pool shared by a test program.
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

    /// Runs the whole front end over one self-contained file.
    pub fn lower(&mut self, source: &str) -> Lowered {
        let file = FileId::from_usize(0);
        let parsed = jr_syntax::parse(source, file);
        let mut earlier = Diagnostics::new();
        earlier.extend(parsed.diagnostics().iter().cloned());

        let (hir, lower_diags) = jr_hir::lower_file(&parsed, file, &self.interner);
        earlier.extend(lower_diags.iter().cloned());

        let (resolve, resolve_diags) = jr_hir::resolve(&hir, &[], &self.interner);
        earlier.extend(resolve_diags.iter().cloned());

        let signatures =
            jr_sema::file_signatures(&hir, file, &resolve, &[], &mut self.pool, &self.interner);
        earlier.extend(signatures.diagnostics.iter().cloned());

        let checked = jr_sema::check_file(
            &hir,
            file,
            &resolve,
            &signatures.signatures,
            &[],
            &mut self.pool,
            &self.interner,
        );
        earlier.extend(checked.diagnostics.iter().cloned());

        let mut types = signatures.types;
        types.absorb(&checked.types);

        let mir = jr_mir::lower_file(
            &hir,
            &resolve,
            &types,
            &signatures.signatures,
            &self.interner,
            &mut self.pool,
        );

        Lowered {
            hir,
            resolve,
            signatures: signatures.signatures,
            types,
            mir,
            earlier_diagnostics: earlier,
        }
    }

    /// Lowers `source` and asserts every earlier phase stayed silent.
    ///
    /// Most tests want this: a refusal caused by a *parse* error would otherwise
    /// look exactly like a refusal caused by the rule under test.
    pub fn lower_clean(&mut self, source: &str) -> Lowered {
        let lowered = self.lower(source);
        assert!(
            lowered.earlier_diagnostics.is_empty(),
            "expected a clean program, got: {:?}",
            lowered
                .earlier_diagnostics
                .iter()
                .map(|d| format!("{:?} {}", d.code, d.message))
                .collect::<Vec<_>>()
        );
        lowered
    }

    /// A dump of every body, for eyeballing a failure.
    pub fn dump(&self, lowered: &Lowered) -> String {
        jr_mir::dump_file(
            &lowered.mir,
            &lowered.hir,
            &self.pool,
            &lowered.signatures,
            &self.interner,
        )
    }
}

impl Default for Program {
    fn default() -> Self {
        Self::new()
    }
}
