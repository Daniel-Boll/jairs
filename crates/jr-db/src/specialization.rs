//! Root-scoped polymorphic specialisation (ADR-0230).
//!
//! A call may demand a clone in another file, so no per-file query can own the complete answer. This
//! module prepares every reachable file, groups demands by template owner, expands owners to a bounded
//! global fixed point, then returns one coherent tuple per file. MIR consumers see only that tuple; the
//! owner grouping, clone numbering and cross-file redirects stay private here.

use std::sync::Arc;

use jr_diag::Diagnostics;
use jr_hir::{FileHir, ResolveMap};
use jr_sema::FileSignatures;

use crate::{
    Db, SourceFile,
    module_loader::{ModuleCatalog, file_hir, resolved},
    sema::{
        CallSite, CheckResult, ComptimeCallKey, Expansion, Instantiated, TypeCallKey,
        build_instantiation_site, checked, checked_expanded, comptime_call_sites, expand_round,
        file_signatures, type_call_sites,
    },
};

/// The front-end tuple from which an owner expansion starts.
#[derive(Clone)]
pub(crate) struct PreparedFile {
    pub(crate) hir: Arc<FileHir>,
    pub(crate) resolve: Arc<ResolveMap>,
    pub(crate) signatures: Arc<FileSignatures>,
    pub(crate) check: CheckResult,
    pub(crate) diagnostics: Diagnostics,
    pub(crate) inserted: bool,
}

/// One reachable file's part of a root-scoped specialisation plan.
#[derive(Clone)]
pub(crate) struct ProgramFileSpecialization {
    pub(crate) file: SourceFile,
    pub(crate) prepared: PreparedFile,
    pub(crate) instantiated: Option<Instantiated>,
    /// Redirects for calls in this file, including calls whose clone belongs to another file.
    pub(crate) redirects: Vec<(CallSite, jr_mir::ProcRef)>,
}

/// The complete specialisation answer for one root program.
pub(crate) struct ProgramSpecializations {
    active: bool,
    files: Vec<ProgramFileSpecialization>,
}

impl ProgramSpecializations {
    /// Whether a demand crossed a file boundary.
    ///
    /// The existing per-file path remains the compatibility path when it did not: it has years of
    /// coverage for local `$T`/`$N`, while this plan exists specifically for the ownership seam.
    pub(crate) fn active(&self) -> bool {
        self.active
    }

    pub(crate) fn file(&self, file: SourceFile) -> Option<&ProgramFileSpecialization> {
        self.files.iter().find(|entry| entry.file == file)
    }
}

struct PlanningFile {
    source: SourceFile,
    file_id: jr_base::FileId,
    prepared: PreparedFile,
    expansion: Option<Expansion>,
    type_keys: Vec<TypeCallKey>,
    type_sites: Vec<Option<jr_hir::InstantiationSite>>,
    comptime_keys: Vec<ComptimeCallKey>,
    comptime_sites: Vec<Option<jr_hir::InstantiationSite>>,
    comptime_values: Option<Arc<jr_mir::ConstValues>>,
}

impl PlanningFile {
    fn hir(&self) -> &FileHir {
        self.expansion
            .as_ref()
            .map_or(self.prepared.hir.as_ref(), |expanded| expanded.hir.as_ref())
    }

    fn check(&self) -> &CheckResult {
        self.expansion
            .as_ref()
            .map_or(&self.prepared.check, |expanded| &expanded.check)
    }
}

/// Plans every owner clone and caller redirect reachable from `root`.
#[salsa::tracked(returns(clone), no_eq)]
pub(crate) fn program_specializations(
    db: &dyn Db,
    root: SourceFile,
    catalog: ModuleCatalog,
) -> Arc<ProgramSpecializations> {
    let mut files: Vec<PlanningFile> = crate::run::reachable_files(db, root, catalog)
        .into_iter()
        .map(|source| {
            let prepared = prepare_file(db, source, catalog);
            let comptime_values = (!prepared.inserted)
                .then(|| crate::consts::file_consts(db, source, catalog).values);
            PlanningFile {
                source,
                file_id: crate::queries::resolve_file_id(db, source),
                prepared,
                expansion: None,
                type_keys: Vec::new(),
                type_sites: Vec::new(),
                comptime_keys: Vec::new(),
                comptime_sites: Vec::new(),
                comptime_values,
            }
        })
        .collect();

    let mut active = false;
    let mut converged = false;
    let mut last_fresh = vec![false; files.len()];

    for _ in 0..crate::sema::MAX_INSTANTIATION_ROUNDS {
        let type_demands: Vec<(usize, CallSite, TypeCallKey)> = files
            .iter()
            .enumerate()
            .flat_map(|(caller, file)| {
                type_call_sites(file.check())
                    .into_iter()
                    .map(move |(call, key)| (caller, call, key))
            })
            .collect();
        let comptime_demands: Vec<(usize, CallSite, ComptimeCallKey)> = files
            .iter()
            .enumerate()
            .flat_map(|(caller, file)| {
                file.comptime_values
                    .as_deref()
                    .map(|values| {
                        comptime_call_sites(file.check(), values, file.file_id)
                            .into_iter()
                            .map(move |(call, key)| (caller, call, key))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .collect();

        let mut fresh = false;
        last_fresh.fill(false);

        for (caller, call, key) in type_demands {
            let Some(owner) = files
                .iter()
                .position(|candidate| candidate.file_id == key.0.file)
            else {
                // A direct imported procedure's file is reachable by construction. If a poisoned import
                // violates that invariant, leave the call unredirected so ordinary diagnostics remain the
                // cause rather than inventing a clone in an unrelated file.
                continue;
            };
            active |= caller != owner;
            if files[owner].type_keys.contains(&key) {
                continue;
            }

            let site = {
                let caller_hir = files[caller].hir();
                let owner_hir = files[owner].prepared.hir.as_ref();
                let owner_signatures = files[owner].prepared.signatures.as_ref();
                let vars = owner_signatures
                    .proc_sig(key.0.proc)
                    .map(|signature| signature.poly_vars.clone())
                    .unwrap_or_default();
                let bindings: Vec<_> = vars.into_iter().zip(key.1.iter().copied()).collect();
                let pool = crate::sema::read_pool(db);
                build_instantiation_site(
                    caller_hir,
                    owner_hir,
                    db.interner(),
                    owner_signatures,
                    &pool,
                    key.0.proc,
                    &bindings,
                    Some(call),
                )
            };
            files[owner].type_keys.push(key);
            files[owner].type_sites.push(site);
            last_fresh[owner] = true;
            fresh = true;
        }

        for (owner, call, key) in comptime_demands {
            if files[owner].comptime_keys.contains(&key) {
                continue;
            }
            let site = {
                let file = &files[owner];
                let vars = file
                    .prepared
                    .signatures
                    .proc_sig(key.0.proc)
                    .map(|signature| signature.poly_vars.clone())
                    .unwrap_or_default();
                let bindings: Vec<_> = vars.into_iter().zip(key.1.iter().copied()).collect();
                let pool = crate::sema::read_pool(db);
                build_instantiation_site(
                    file.hir(),
                    file.prepared.hir.as_ref(),
                    db.interner(),
                    file.prepared.signatures.as_ref(),
                    &pool,
                    key.0.proc,
                    &bindings,
                    Some(call),
                )
            };
            files[owner].comptime_keys.push(key);
            files[owner].comptime_sites.push(site);
            last_fresh[owner] = true;
            fresh = true;
        }

        if !fresh {
            converged = true;
            break;
        }

        for file in &mut files {
            if file.type_keys.is_empty() && file.comptime_keys.is_empty() {
                continue;
            }
            file.expansion = Some(expand_round(
                db,
                file.source,
                catalog,
                file.prepared.hir.as_ref(),
                &file.type_keys,
                &file.comptime_keys,
                &file.type_sites,
                &file.comptime_sites,
            ));
        }
    }

    // A key's index is its clone's index in the owner expansion. The vectors are deterministic:
    // reachable-file order, then each check's sorted call-site order.
    let type_targets: Vec<(TypeCallKey, jr_mir::ProcRef)> = files
        .iter()
        .flat_map(|owner| {
            owner
                .expansion
                .as_ref()
                .into_iter()
                .flat_map(move |expansion| {
                    owner.type_keys.iter().enumerate().map(move |(index, key)| {
                        (
                            key.clone(),
                            jr_mir::ProcRef::new(owner.file_id, expansion.new_ids[index]),
                        )
                    })
                })
        })
        .collect();

    let mut planned = Vec::with_capacity(files.len());
    for (index, mut file) in files.into_iter().enumerate() {
        let check = file
            .expansion
            .as_ref()
            .map_or(&file.prepared.check, |expanded| &expanded.check);
        let mut redirects = Vec::new();
        for (call, key) in type_call_sites(check) {
            if let Some((_, target)) = type_targets.iter().find(|(candidate, _)| candidate == &key)
            {
                redirects.push((call, *target));
            }
        }

        let mut comptime_masks = Vec::new();
        if let (Some(expansion), Some(values)) =
            (file.expansion.as_ref(), file.comptime_values.as_deref())
        {
            for (call, key) in comptime_call_sites(check, values, file.file_id) {
                let Some(key_index) = file
                    .comptime_keys
                    .iter()
                    .position(|candidate| candidate == &key)
                else {
                    continue;
                };
                redirects.push((
                    call,
                    jr_mir::ProcRef::new(
                        file.file_id,
                        expansion.new_ids[expansion.comptime_start + key_index],
                    ),
                ));
                let mask = file
                    .prepared
                    .signatures
                    .proc_sig(key.0.proc)
                    .map(|signature| signature.comptime_params.clone())
                    .unwrap_or_default();
                comptime_masks.push((call, mask));
            }
        }

        let instantiated = file.expansion.take().map(|expansion| {
            let mut diagnostics = expansion.diagnostics;
            if !converged && last_fresh.get(index).copied().unwrap_or(false) {
                diagnostics.push(
                    jr_diag::Diagnostic::error(
                        jr_base::Span::from_offsets(file.file_id, 0, 0),
                        format!(
                            "instantiation did not settle after {} rounds: this program produces an \
                             unbounded family of instantiations",
                            crate::sema::MAX_INSTANTIATION_ROUNDS
                        ),
                    )
                    .with_code(crate::sema::E0280)
                    .with_help(
                        "a polymorphic procedure instantiating itself at a new type each round cannot \
                         terminate; give the recursion a concrete type",
                    ),
                );
            }
            Instantiated {
                hir: expansion.hir,
                resolve: expansion.resolve,
                signatures: expansion.signatures,
                check: expansion.check,
                diagnostics,
                redirects: redirects.clone(),
                comptime_masks,
                body_scopes: expansion.body_scopes,
            }
        });

        planned.push(ProgramFileSpecialization {
            file: file.source,
            prepared: file.prepared,
            instantiated,
            redirects,
        });
    }

    Arc::new(ProgramSpecializations {
        active,
        files: planned,
    })
}

fn prepare_file(db: &dyn Db, file: SourceFile, catalog: ModuleCatalog) -> PreparedFile {
    let operands = crate::consts::insert_operands(db, file, catalog);
    if operands.is_empty() {
        return PreparedFile {
            hir: file_hir(db, file),
            resolve: resolved(db, file, catalog).map,
            signatures: file_signatures(db, file, catalog).signatures,
            check: checked(db, file, catalog),
            diagnostics: Diagnostics::new(),
            inserted: false,
        };
    }

    let parse = crate::parse_file(db, file);
    let file_id = crate::queries::resolve_file_id(db, file);
    let (hir, lower_diagnostics) =
        jr_hir::lower_file_with_inserts(&parse, file_id, db.interner(), operands.as_ref());
    let hir = Arc::new(hir);
    let (resolve, check, diagnostics, signatures) =
        checked_expanded(db, file, catalog, hir.as_ref());
    let mut ordered = lower_diagnostics;
    ordered.extend(diagnostics.iter().cloned());
    PreparedFile {
        hir,
        resolve,
        signatures,
        check,
        diagnostics: ordered,
        inserted: true,
    }
}
