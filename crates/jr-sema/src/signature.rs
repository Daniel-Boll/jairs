//! The signature phase: typing declarations without checking bodies.

use jr_base::{FileId, Interner};
use jr_diag::{Diagnostic, Diagnostics};
use jr_hir::{ConstValue, ExprScope, FileHir, ItemId, ItemKind, ProcId, ResolveMap};
use jr_pool::{ContextKind, Item, Pool, PoolId};

use crate::check::bin_op_text;
use crate::code::{E0226, E0246};
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
    // Recorded so that an *imported* overload can become a `ProcRef` (ADR-0048 §5).
    ctx.sigs.set_file(file);

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
                // An overload is an ordinary procedure with two extra checks: the operator must be
                // one ADR-0048 §2 permits, and at least one operand must be declared in this file
                // (§3's orphan rule).
                ConstValue::Operator(proc, op) => {
                    let sig = self.proc_signature(proc);
                    self.check_operator_overload(op, &sig, proc, span);
                    Some(SigEntry {
                        ty: sig.ty,
                        type_value: None,
                        kind: SigKind::Operator,
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
                // The struct arm with one line changed — `union_type` rather than
                // `struct_type` — because everything else about a union's *signature* is a
                // struct's: nominal identity, a recorded type name, a field list resolved
                // after the type has an ID (ADR-0045 §5).
                ConstValue::Union(sid) => {
                    let ty = self.union_type(sid);
                    let interner = self.interner;
                    self.sigs
                        .insert_type_name(ty, interner.resolve(name).to_owned());
                    self.resolve_struct_body(sid, ty, span);
                    Some(SigEntry {
                        ty: PoolId::TYPE,
                        type_value: Some(ty),
                        kind: SigKind::Union,
                        item,
                    })
                }
                // The same shape as the struct arm, and for the same reasons: the type name is
                // recorded so a diagnostic can spell it, and the body is resolved *after* the
                // type has an ID (ADR-0041 §4).
                ConstValue::Enum(eid) => {
                    let ty = self.enum_type(eid);
                    let interner = self.interner;
                    self.sigs
                        .insert_type_name(ty, interner.resolve(name).to_owned());
                    self.resolve_enum_body(eid, span);
                    Some(SigEntry {
                        ty: PoolId::TYPE,
                        type_value: Some(ty),
                        kind: SigKind::Enum,
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
    /// Checks an overload and registers it, or reports why it cannot be one (ADR-0048 §2, §3).
    fn check_operator_overload(
        &mut self,
        op: jr_hir::BinOp,
        sig: &ProcSig,
        proc: ProcId,
        span: jr_base::Span,
    ) {
        // Exactly two operands. A one-parameter `operator -` would be unary negation, which
        // ADR-0048 §6 leaves out because it collides with the binary form's name.
        if sig.params.len() != 2 {
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "an overload of `{}` must take exactly two parameters, not {}",
                        bin_op_text(op),
                        sig.params.len()
                    ),
                )
                .with_code(E0246)
                .with_note("unary operator overloading is not supported (ADR-0048 §6)"),
            );
            return;
        }
        let (lhs, rhs) = (sig.params[0], sig.params[1]);
        if lhs == PoolId::ERROR || rhs == PoolId::ERROR {
            // A parameter type that did not resolve was already reported; registering the
            // overload under a poison type would make it unreachable anyway.
            return;
        }

        // ADR-0048 §2's permitted set. Each refusal names its own reason rather than sharing a
        // generic one, because "wrapping is about a machine representation" and "bitwise belongs
        // to `enum_flags`" are different facts and a reader can act on each.
        let refusal = match op {
            jr_hir::BinOp::Add
            | jr_hir::BinOp::Sub
            | jr_hir::BinOp::Mul
            | jr_hir::BinOp::Div
            | jr_hir::BinOp::Rem
            | jr_hir::BinOp::Eq
            | jr_hir::BinOp::Ne
            | jr_hir::BinOp::Lt
            | jr_hir::BinOp::Le
            | jr_hir::BinOp::Gt
            | jr_hir::BinOp::Ge => None,
            jr_hir::BinOp::WrapAdd | jr_hir::BinOp::WrapSub | jr_hir::BinOp::WrapMul => Some(
                "the wrapping operators mean \"wrap the machine integer at this width\" (ADR-0002), which has no meaning for a user type",
            ),
            jr_hir::BinOp::BitAnd
            | jr_hir::BinOp::BitOr
            | jr_hir::BinOp::BitXor
            | jr_hir::BinOp::Shl
            | jr_hir::BinOp::Shr => Some(
                "the bitwise operators are reserved for `enum_flags`, whose design is that `&` on a flags type yields the flags type (ADR-0043)",
            ),
            jr_hir::BinOp::And | jr_hir::BinOp::Or => Some(
                "`&&` and `||` are control flow rather than operators — MIR has no node for them, so an overload could not short-circuit",
            ),
        };
        if let Some(note) = refusal {
            self.diags.push(
                Diagnostic::error(span, format!("`{}` cannot be overloaded", bin_op_text(op)))
                    .with_code(E0246)
                    .with_note(note),
            );
            return;
        }

        // §3's orphan rule: at least one operand must be *declared in this file*. A `DeclId`
        // records exactly that (ADR-0015 §1), so this is a file comparison rather than anything
        // new — and a structural type (`*T`, `[N]T`, `[]T`) is declared nowhere and so never
        // satisfies it, which is what keeps view equality builtin-only (ADR-0044 §5).
        if !self.declared_here(lhs) && !self.declared_here(rhs) {
            let (lt, rt) = (self.describe(lhs), self.describe(rhs));
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "an overload of `{}` for `{lt}` and `{rt}` needs a type declared in this file",
                        bin_op_text(op)
                    ),
                )
                .with_code(E0246)
                .with_note(
                    "at least one operand must be a struct, union or enum declared here, so an `#import` cannot change what an operator means for types it does not own",
                ),
            );
            return;
        }

        if self.sigs.insert_operator(op, lhs, rhs, proc).is_err() {
            let (lt, rt) = (self.describe(lhs), self.describe(rhs));
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "`{}` is already overloaded for `{lt}` and `{rt}`",
                        bin_op_text(op)
                    ),
                )
                .with_code(E0246)
                .with_note(
                    "resolution is an exact match on both operand types, so two overloads \
                     for the same pair have no way to be told apart (ADR-0048 §4)",
                ),
            );
        }
    }

    /// Whether `ty` is a nominal type declared in *this* file (ADR-0048 §3).
    fn declared_here(&self, ty: PoolId) -> bool {
        match self.pool.item(ty) {
            Item::StructType { decl } | Item::UnionType { decl } | Item::EnumType { decl, .. } => {
                decl.file == self.file
            }
            _ => false,
        }
    }

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
