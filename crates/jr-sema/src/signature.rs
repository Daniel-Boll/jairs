//! The signature phase: typing declarations without checking bodies.

use jr_base::{FileId, Interner};
use jr_diag::{Diagnostic, Diagnostics};
use jr_hir::{ConstValue, ExprScope, FileHir, ItemId, ItemKind, ProcId, ResolveMap};
use jr_pool::{ContextKind, Pool, PoolId};

use crate::code::E0226;
use crate::ctx::{Ctx, Mode};
use crate::map::TypeMap;
use crate::sigs::{FileSignatures, ProcSig, SigEntry, SigKind};

// ---------------------------------------------------------------------------
// Inputs and outputs
// ---------------------------------------------------------------------------

/// A module this file imports, as the signature phase needs to see it.
///
/// The signature phase takes imported **HIR**, not imported signatures. That is
/// not an oversight: ADR-0016 §5 requires signature computation to depend only on
/// another file's HIR, because `Cycle_A` and `Cycle_B` import each other and a
/// signature query that called another file's signature query would make the
/// dependency graph cyclic. The check phase, which may read signatures freely,
/// takes [`FileSignatures`] instead.
pub struct ImportedFile<'a> {
    /// The module name as written in `#import "Name"`.
    pub name: &'a str,
    /// The imported file's stable id, which is half of every nominal type's
    /// identity.
    pub file: FileId,
    /// The imported file's HIR.
    pub hir: &'a FileHir,
    /// The imported file's name resolution.
    pub resolve: &'a ResolveMap,
}

/// What the signature phase produces.
pub struct SignatureOutput {
    /// The file's signatures, for its own check phase and for its importers.
    pub signatures: FileSignatures,
    /// The types of the file-level expressions this phase typed.
    pub types: TypeMap,
    /// Diagnostics about declarations: unresolvable annotations, constants whose
    /// initialiser does not check, and constant cycles.
    pub diagnostics: Diagnostics,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Computes the signatures of one file.
///
/// This phase owns every **named** file-level declaration: it resolves parameter,
/// return, field and variable annotations, and it types constant initialisers.
/// The check phase deliberately does not revisit them, because two phases typing
/// the same expression means either duplicated diagnostics or a silent
/// disagreement about a constant's type.
///
/// # Imported names cost a recomputation
///
/// An imported name's type is obtained by computing that module's signatures
/// here, with an empty import list — so a name the *imported* file itself
/// imported is not visible, and resolves to poison. This is the price of ADR-0016
/// §5's rule that signatures may not call other signatures: the recursion is one
/// level deep and therefore terminates even for a module cycle. The cost is that
/// an imported module's signatures are computed once per importer instead of once
/// per module; interning is idempotent, so the only loss is time.
pub fn file_signatures(
    hir: &FileHir,
    file: FileId,
    resolve: &ResolveMap,
    imports: &[ImportedFile<'_>],
    pool: &mut Pool,
    interner: &Interner,
) -> SignatureOutput {
    // Shallow signatures for each import: same algorithm, no imports of their
    // own, diagnostics discarded because they belong to the other file.
    let shallow: Vec<(&str, FileSignatures)> = imports
        .iter()
        .map(|imported| {
            let output = file_signatures(
                imported.hir,
                imported.file,
                imported.resolve,
                &[],
                pool,
                interner,
            );
            (imported.name, output.signatures)
        })
        .collect();
    let import_refs: Vec<(&str, &FileSignatures)> =
        shallow.iter().map(|(name, sigs)| (*name, sigs)).collect();

    let mut ctx = Ctx::new(
        hir,
        file,
        resolve,
        interner,
        pool,
        import_refs,
        Mode::Signatures,
    );

    // Pass 1: give every named struct its type identity before any field type is
    // resolved. Without this, `Node :: struct { next: *Node; }` — and any pair of
    // structs that point at each other — would trip the constant-cycle guard.
    // ADR-0015 §1 is what makes this legal: a struct type's identity is its
    // declaration site, not its fields.
    for index in 0..hir.items.len() {
        let item_id = ItemId::from_usize(index);
        let item = hir.item(item_id);
        let Some(name) = item.name else { continue };
        let ItemKind::Const {
            value: ConstValue::Struct(sid),
        } = item.kind
        else {
            continue;
        };
        let ty = ctx.struct_type(sid);
        ctx.sigs
            .insert_type_name(ty, interner.resolve(name).to_owned());
        ctx.sigs.insert(
            name,
            SigEntry {
                ty: PoolId::TYPE,
                type_value: Some(ty),
                kind: SigKind::Struct,
                item: item_id,
            },
        );
    }

    // Pass 2: force every named item. Order does not matter — a constant may
    // refer to one declared later (ADR-0007) — because `item_signature` is
    // demand-driven and memoised.
    for index in 0..hir.items.len() {
        let _ = ctx.item_signature(ItemId::from_usize(index));
    }

    SignatureOutput {
        signatures: ctx.sigs,
        types: ctx.types,
        diagnostics: ctx.diags,
    }
}

// ---------------------------------------------------------------------------
// On-demand item signatures
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    /// Returns the signature of a named file-level item, computing it if needed.
    ///
    /// `None` for an unnamed item (`#import`, a top-level `#run`), which has no
    /// signature to return.
    pub(crate) fn item_signature(&mut self, item: ItemId) -> Option<SigEntry> {
        let hir = self.hir;
        let name = hir.item(item).name?;

        if self.finished.contains(&item) {
            return self.sigs.lookup(name);
        }
        if self.in_progress.contains(&item) {
            self.report_constant_cycle(item);
            return Some(SigEntry {
                ty: PoolId::ERROR,
                type_value: None,
                kind: SigKind::Const,
                item,
            });
        }

        self.in_progress.push(item);
        let entry = self.compute_item_signature(item);
        self.in_progress.pop();
        self.finished.insert(item);

        if let Some(entry) = entry {
            self.sigs.insert(name, entry);
        }
        entry
    }

    /// Computes one item's signature, assuming the cycle guard is already set.
    fn compute_item_signature(&mut self, item: ItemId) -> Option<SigEntry> {
        let hir = self.hir;
        let declaration = hir.item(item);
        let name = declaration.name?;
        let span = declaration.name_span;
        let kind = declaration.kind.clone();

        match kind {
            ItemKind::Const { value } => match value {
                ConstValue::Proc(proc) => {
                    let sig = self.proc_signature(proc);
                    Some(SigEntry {
                        ty: sig.ty,
                        type_value: None,
                        kind: SigKind::Proc,
                        item,
                    })
                }
                ConstValue::Struct(sid) => {
                    let ty = self.struct_type(sid);
                    let interner = self.interner;
                    self.sigs
                        .insert_type_name(ty, interner.resolve(name).to_owned());
                    self.resolve_struct_body(sid, ty, span);
                    Some(SigEntry {
                        ty: PoolId::TYPE,
                        type_value: Some(ty),
                        kind: SigKind::Struct,
                        item,
                    })
                }
                ConstValue::Expr(expr) => {
                    // No annotation exists on a `::` declaration, so the
                    // initialiser types itself and an untyped integer literal
                    // lands on the default (ADR-0016 §1).
                    let ty = self.check_expr(ExprScope::TopLevel, expr, None);
                    Some(SigEntry {
                        ty,
                        type_value: None,
                        kind: SigKind::Const,
                        item,
                    })
                }
            },
            ItemKind::Var { ty, init, uninit } => {
                let declared = ty.map(|id| self.resolve_type(ExprScope::TopLevel, id, span));
                let resolved = match (declared, init) {
                    (Some(annotation), Some(expr)) => {
                        self.check_expr(ExprScope::TopLevel, expr, Some(annotation));
                        annotation
                    }
                    // `x: T;` and `x: T = ---;` are both "declared, not
                    // initialised here"; whether reading it first is an error is
                    // wave W3's definite-assignment analysis, not a typing
                    // question.
                    (Some(annotation), None) => annotation,
                    (None, Some(expr)) => {
                        let inferred = self.check_expr(ExprScope::TopLevel, expr, None);
                        self.reject_void_binding(inferred, span)
                    }
                    // The parser requires an annotation or an initialiser, so
                    // this is error recovery: poison quietly.
                    (None, None) => PoolId::ERROR,
                };
                let _ = uninit;
                Some(SigEntry {
                    ty: resolved,
                    type_value: None,
                    kind: SigKind::Var,
                    item,
                })
            }
            // Neither has a name, so neither has a signature. `#run` is typed by
            // the check phase, which is where unnamed items are handled.
            ItemKind::Import { .. } | ItemKind::Run { .. } => None,
        }
    }

    /// Resolves a procedure's signature and records it.
    fn proc_signature(&mut self, proc: ProcId) -> ProcSig {
        let hir = self.hir;
        let declaration = hir.proc(proc).clone();

        let mut params = Vec::with_capacity(declaration.params.len());
        for param in &declaration.params {
            let ty = match param.ty {
                // Parameter types live in `FileHir::type_refs`, not in
                // `Proc::type_refs`, which is always empty.
                Some(id) => self.resolve_type(ExprScope::TopLevel, id, param.name_span),
                None => PoolId::ERROR,
            };
            params.push(ty);
        }

        let ret = match declaration.ret {
            Some(id) => self.resolve_type(ExprScope::TopLevel, id, declaration.span),
            // Omitting the arrow means `void`, which is a real type (ADR-0015 §3)
            // so that a procedure type's return field is total.
            None => PoolId::VOID,
        };

        // Every `#foreign` procedure is implicitly `#c_call` (ADR-0001), and the
        // context kind is part of the type's identity — a function pointer of one
        // kind must never satisfy the other.
        let context = if declaration.foreign.is_some() {
            ContextKind::CCall
        } else {
            ContextKind::Jairs
        };

        let ty = self.pool.proc_type(params.clone(), ret, context);
        let sig = ProcSig { params, ret, ty };
        self.sigs.insert_proc(proc, sig.clone());
        if let Some(library) = self.foreign_library_of(&declaration) {
            self.sigs.insert_foreign_library(proc, library);
        }
        sig
    }

    /// Resolves a `#foreign` procedure's library operand to an interned library.
    ///
    /// `write :: (...) #foreign libc "write"` names the *constant* `libc`, not the
    /// library `"c"`. Getting from one to the other means finding the item that
    /// constant declares and reading the `#system_library` directive out of it.
    ///
    /// `None` for a procedure that is not `#foreign`, that named no library, or
    /// whose operand does not resolve to a `#system_library` declaration. The last
    /// case is already an E0225 from the check phase, and returning `None` here
    /// means a consumer refuses to guess rather than inventing a library name.
    ///
    /// ADR-0019 §4 put this here. It used to be done twice — once in this crate to
    /// check the operand denotes a library, and once in `jr-vm` to make the call —
    /// and the native back end would have been a third. The answer is interned in
    /// the pool so that all three read one resolution.
    fn foreign_library_of(&mut self, declaration: &jr_hir::Proc) -> Option<PoolId> {
        let name = declaration.foreign.as_ref()?.library?;
        let hir = self.hir;
        let item = hir.scope.get(name)?;
        let ItemKind::Const {
            value: ConstValue::Expr(expr),
        } = &hir.items.get(item.index())?.kind
        else {
            return None;
        };
        let jr_hir::Expr::Directive {
            name: directive,
            arg,
            span: _,
        } = hir.exprs.get(expr.index())?
        else {
            return None;
        };
        let interner = self.interner;
        if interner.resolve(*directive) != "system_library" {
            return None;
        }
        let arg = arg.clone()?;
        Some(self.pool.foreign_library_value(&arg))
    }

    /// Reports a constant whose type depends on itself.
    fn report_constant_cycle(&mut self, item: ItemId) {
        let hir = self.hir;
        let interner = self.interner;
        let declaration = hir.item(item);
        let name = declaration.name.map_or_else(
            || "<unnamed>".to_owned(),
            |n| interner.resolve(n).to_owned(),
        );
        self.diags.push(
            Diagnostic::error(
                declaration.name_span,
                format!("the type of `{name}` depends on itself"),
            )
            .with_code(E0226)
            .with_note("a constant's type comes from its value, so a cycle has no type")
            .with_help("annotate one of the declarations in the cycle with an explicit type"),
        );
    }
}
