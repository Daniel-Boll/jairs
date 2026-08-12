//! The signature phase: typing declarations without checking bodies.

use jr_base::{FileId, Interner, Span};
use jr_diag::{Diagnostic, Diagnostics};
use jr_hir::{
    ConstValue, Expr, ExprScope, FileHir, ItemId, ItemKind, Literal, ProcId, ResolveMap, TypeRef,
};
use jr_pool::{ContextKind, Item, Pool, PoolId};

use crate::check::bin_op_text;
use crate::code::{E0204, E0214, E0226, E0246, E0252, E0255};
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

    // The imported HIRs the *signature* phase already has, for the reason ADR-0117 §1 gives — this phase has
    // always had them through `ImportedFile`, and `Ctx` now carries them so the check phase can too.
    let imported_hirs: Vec<(jr_base::FileId, &FileHir)> =
        imports.iter().map(|i| (i.file, i.hir)).collect();

    let mut ctx = Ctx::new(
        hir,
        file,
        resolve,
        interner,
        pool,
        import_refs,
        imported_hirs,
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
                    // Recorded so an *importer* can refuse a cross-file macro call (ADR-0091 §3): a macro
                    // is spliced from its own file's source text, which does not cross the boundary, and an
                    // importer has these signatures rather than this file's HIR.
                    if self.hir.procs.get(proc.index()).is_some_and(|p| p.expand) {
                        self.sigs.insert_macro(name);
                    }
                    // Recorded for the same reason one level over (ADR-0104 §2): cross-file *instantiation*
                    // is deferred, so an importer must be able to recognise an imported template in order to
                    // refuse the call with E0268 — which it could not, because a `$T` parameter's type is
                    // `PoolId::ERROR` and `ERROR` matches anything, so the call type-checked and the missing
                    // instantiation leaked out of an engine as an internal error.
                    if sig.is_template() {
                        self.sigs.insert_template(name);
                    }
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
                    // A parameterised `struct($T) { … }` is a **template**, not a type: its field type
                    // `T` has no meaning until an instantiation binds it (ADR-0085 §3). Bind its
                    // variables to `ERROR` around the body resolution so a bare `T` resolves quietly
                    // rather than reporting E0212 — the same discipline a polymorphic procedure's
                    // template uses (ADR-0081 §1). Each `Box(s64)` reference resolves its own fields
                    // under real bindings, keyed on the instance, so this template entry's fields are
                    // never read.
                    let poly_vars = self.hir.struct_def(sid).poly_vars.clone();
                    for &var in &poly_vars {
                        self.type_bindings.insert(var, PoolId::ERROR);
                    }
                    self.resolve_struct_body(sid, ty, span);
                    for &var in &poly_vars {
                        self.type_bindings.remove(&var);
                    }
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
                // The same shape a third time (ADR-0068 §1): a variant's cases are a field list, so
                // the body resolution and the recorded name are a union's. `SigKind::Variant` is what
                // makes a consumer able to tell them apart without a second lookup.
                ConstValue::Variant(sid) => {
                    let ty = self.variant_type(sid);
                    let interner = self.interner;
                    self.sigs
                        .insert_type_name(ty, interner.resolve(name).to_owned());
                    self.resolve_struct_body(sid, ty, span);
                    Some(SigEntry {
                        ty: PoolId::TYPE,
                        type_value: Some(ty),
                        kind: SigKind::Variant,
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
                    // A `::` initialiser is the one position that may **name a type**: `T :: Point;`
                    // binds a type value (ADR-0071 §1, §2). Recorded before typing it, the same
                    // mechanism a field-access receiver uses, so the `Name` arm's E0261 refusal skips
                    // this expression while still typing it.
                    self.type_position.insert((ExprScope::TopLevel, expr));
                    // No annotation exists on a `::` declaration, so the
                    // initialiser types itself and an untyped integer literal
                    // lands on the default (ADR-0016 §1).
                    let ty = self.check_expr(ExprScope::TopLevel, expr, None);
                    // **A type-valued constant is an alias**, and its entry carries the type it
                    // denotes rather than only `PoolId::TYPE` — which is what makes `T` usable in a
                    // type annotation, since `resolve_type_name` reads exactly this field. Taken from
                    // the *aliased* name's own entry, so the alias is one lookup rather than a second
                    // resolution of what `Point` means (ADR-0071 §2).
                    //
                    // One level only: `B :: A` where `A :: Point` leaves `type_value` `None`, because a
                    // chain needs a fixpoint and a cycle check (§5) — the same line ADR-0070 §4 drew
                    // for an array length.
                    let type_value = (ty == PoolId::TYPE)
                        .then(|| self.aliased_type(ExprScope::TopLevel, expr))
                        .flatten();
                    Some(SigEntry {
                        ty,
                        type_value,
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
                    // definite assignment, not a typing question. It is checked in
                    // `jr-mir` over the CFG (E0245), not deferred to a wave — this
                    // comment named W3 for waves after W3 shipped it.
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
            Item::StructType { decl, .. }
            | Item::UnionType { decl, .. }
            | Item::EnumType { decl, .. } => decl.file == self.file,
            _ => false,
        }
    }

    /// Interns a parameter's default value, refusing anything but a literal (ADR-0053 §2).
    ///
    /// **No const-eval.** The literal is read directly out of the HIR and interned against the
    /// parameter's type. A default that could be `SIZE` would make a *signature* depend on a
    /// constant's value, and that constant's type depends on signatures — the cycle ADR-0018 §3's
    /// ordering exists to prevent, and the same shape ADR-0039 §3a records for an array length.
    ///
    /// The refusal names what would be needed rather than saying "unsupported", because a reader who
    /// writes `= SIZE` will try `= 2 + 3` next unless they learn the rule.
    /// The polymorphic type variables a parameter list introduces, in first-seen order (ADR-0081 §1).
    ///
    /// Walks each parameter's `TypeRef` tree for [`TypeRef::Poly`] — `$T` may be nested (`*$T`, `[]$T`),
    /// so it recurses. First-seen order and de-duplication mean `swap :: (a: $T, b: $T)` yields one
    /// variable `T`, not two, which is ADR-0081 §4's "one `$T` used across several positions" case.
    fn collect_poly_vars(&self, params: &[jr_hir::Param]) -> Vec<jr_base::Symbol> {
        let mut vars = Vec::new();
        for param in params {
            if let Some(id) = param.ty {
                self.collect_poly_in_type(ExprScope::TopLevel, id, &mut vars);
            }
        }
        vars
    }

    /// Adds the `$T` variables reachable from one `TypeRef` to `vars`, de-duplicated, in first-seen order.
    fn collect_poly_in_type(
        &self,
        scope: ExprScope,
        id: jr_hir::TypeRefId,
        vars: &mut Vec<jr_base::Symbol>,
    ) {
        match self.type_ref(scope, id) {
            TypeRef::Poly(sym) => {
                if !vars.contains(&sym) {
                    vars.push(sym);
                }
            }
            TypeRef::Pointer(inner) => self.collect_poly_in_type(scope, inner, vars),
            TypeRef::Array { elem, .. } | TypeRef::View { elem } => {
                self.collect_poly_in_type(scope, elem, vars);
            }
            // A `$T` inside a proc-pointer or results type is not part of this sub-wave's one-`$T` slice;
            // it is left for the sub-wave that generalises, and reaching one resolves to `ERROR` rather
            // than binding — which refuses the signature rather than half-supporting it (ADR-0081 §4).
            // A `$T` inside a parameterised type reference — `f :: (b: Box($T))` — is nested inference
            // through a nominal type, deferred with the rest of that step (ADR-0085 §5). So `Apply` does
            // not bind here this sub-wave; a `Box(s64)` parameter is an ordinary concrete type.
            TypeRef::Proc { .. }
            | TypeRef::Results(_)
            | TypeRef::Name(_)
            | TypeRef::Apply { .. }
            | TypeRef::Struct(_)
            | TypeRef::Union(_)
            | TypeRef::Variant(_)
            | TypeRef::Enum(_)
            | TypeRef::Error => {}
        }
    }

    fn param_default(
        &mut self,
        param: &jr_hir::Param,
        ty: PoolId,
        foreign: bool,
    ) -> Option<PoolId> {
        let expr = param.default?;
        let span = param.name_span;
        // A `#foreign` procedure's parameters are the C function's, and Jairs does not control its
        // call sites — a default would be a Jairs-side fiction the FFI boundary cannot honour
        // (ADR-0053 §4).
        if foreign {
            self.diags.push(
                Diagnostic::error(span, "a `#foreign` parameter cannot have a default value")
                    .with_code(E0252)
                    .with_note("the C function's own call sites are not Jairs's to fill in"),
            );
            return None;
        }
        let Expr::Literal(literal, lit_span) = self.hir.expr(expr).clone() else {
            self.diags.push(
                Diagnostic::error(span, "a default value must be a literal")
                    .with_code(E0252)
                    .with_note(
                        "constants are evaluated after signatures are resolved, so a signature cannot depend on one",
                    )
                    .with_help("write the value directly, as in `= 10`"),
            );
            return None;
        };
        // Checked against the parameter's own type through the *existing* literal fit rule
        // (ADR-0016 §1, ADR-0038), so `b: u8 = 300` is the established E0204 rather than a new code.
        self.intern_default(&literal, ty, lit_span)
    }

    /// Interns a literal as a value of type `ty`, reporting a mismatch.
    ///
    /// Deliberately *not* `check_literal`, which answers a **type**: a default needs the interned
    /// **value**, because `ProcSig::defaults` holds one and a call site fills it in without
    /// re-reading the HIR. The fit check is `IntKind`'s, the same one every integer literal gets.
    fn intern_default(&mut self, literal: &Literal, ty: PoolId, span: Span) -> Option<PoolId> {
        match literal {
            Literal::Bool(value) => {
                self.expect_default(ty, PoolId::BOOL, span)?;
                Some(self.pool.bool_value(*value))
            }
            Literal::Str(text) => {
                self.expect_default(ty, PoolId::STRING, span)?;
                Some(self.pool.str_value(text))
            }
            Literal::Int { value, .. } => {
                let kind = jr_pool::IntKind::of(self.pool, ty)?;
                // The *same* range check every integer literal gets (ADR-0038): checked against the
                // type's range rather than its maximum magnitude, so `-128` fits `s8`.
                let Ok(bits) = kind.check(*value, "a default value") else {
                    let text = self.describe(ty);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("the default value does not fit in `{text}`"),
                        )
                        .with_code(E0204),
                    );
                    return None;
                };
                Some(self.pool.int_value(ty, bits))
            }
            Literal::Float { bits, .. } => {
                let kind = jr_pool::FloatKind::of(self.pool, ty)?;
                Some(
                    self.pool
                        .float_value(ty, kind.encode(f64::from_bits(*bits))),
                )
            }
            // `null` as a default (ADR-0060 §5): a literal, so it is admissible, and it interns to
            // the zero pointer of the parameter's type — the same value `check_null_literal`
            // produces. The parameter's type must be a pointer, checked the way `expect_default`
            // checks the other kinds: a `p: s64 = null` default is the same category error a
            // `p: s64 = 1.5` default is.
            Literal::Null => {
                if self.pointee(ty).is_none() {
                    let want = self.describe(ty);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("a `null` default cannot be used for a `{want}` parameter"),
                        )
                        .with_code(E0214),
                    );
                    return None;
                }
                Some(self.pool.int_value(ty, 0))
            }
        }
    }

    /// Reports a default whose literal kind does not match its parameter's type.
    fn expect_default(&mut self, declared: PoolId, actual: PoolId, span: Span) -> Option<()> {
        if declared == actual {
            return Some(());
        }
        let want = self.describe(declared);
        let got = self.describe(actual);
        self.diags.push(
            Diagnostic::error(
                span,
                format!("a default of type `{got}` cannot be used for a `{want}` parameter"),
            )
            .with_code(E0214),
        );
        None
    }

    fn proc_signature(&mut self, proc: ProcId) -> ProcSig {
        let hir = self.hir;
        let declaration = hir.proc(proc).clone();

        let mut params = Vec::with_capacity(declaration.params.len());
        let mut names = Vec::with_capacity(declaration.params.len());
        let mut defaults = Vec::with_capacity(declaration.params.len());
        let foreign = declaration.foreign.is_some();

        // **Bind the signature's `$T` variables first** (ADR-0081 §1), so a bare `T` in a later parameter
        // or the return type resolves to the variable rather than reporting E0212. Each is bound to
        // `PoolId::ERROR` — the "not concrete" placeholder — because the *template* has no concrete type
        // for it; a call supplies one at instantiation. The names are collected into `poly_vars`, in
        // first-seen order, so a consumer knows the signature is a template. The bindings are cleared at
        // the end of this function, so they never leak into another signature.
        // An **instantiation** has a `proc_bindings` entry, which makes it *concrete*: its `poly_vars` is
        // empty, so it is lowered and declared like any other procedure (ADR-0082 §2). Only a *template*
        // — a `$T` proc with no binding — keeps its variables here.
        let is_instantiation = self.hir.proc_bindings.iter().any(|(p, _, _)| *p == proc);
        let poly_vars = if is_instantiation {
            Vec::new()
        } else {
            self.collect_poly_vars(&declaration.params)
        };
        // Bind whatever variables the params introduce — for a template, to `ERROR`; for an
        // instantiation, `poly_vars` is empty but its params still mention `$T`, so bind those from
        // `proc_bindings` too, else the concrete signature would resolve `$T` to `ERROR`.
        let vars_to_bind = self.collect_poly_vars(&declaration.params);
        for &var in &vars_to_bind {
            // An **instantiation** binds its variable to a concrete type (ADR-0082 §2), carried on the
            // expanded HIR's `proc_bindings`; the *template* binds to `ERROR`. Reading the concrete
            // binding here is what makes an instantiation's signature — and, downstream, its body —
            // resolve `$T`/`T` to the real type and be checked against it.
            let bound = self
                .hir
                .proc_bindings
                .iter()
                .find(|(p, v, _)| *p == proc && *v == var)
                .map_or(PoolId::ERROR, |(_, _, ty)| *ty);
            self.type_bindings.insert(var, bound);
        }

        // The **baked comptime values** of this procedure's `$N` parameters, if it is an instantiation
        // (ADR-0089 §1). Set for the same window `type_bindings` is, so an array length naming a `$N`
        // parameter — `buf: [N]s64` — resolves while this signature and its body are resolved. Empty for
        // an ordinary procedure and for a template, so nothing else changes.
        let values_to_bind: Vec<(jr_base::Symbol, PoolId)> = self
            .hir
            .param_values
            .iter()
            .filter(|(p, _, _)| *p == proc)
            .map(|(_, name, value)| (*name, *value))
            .collect();
        for (name, value) in &values_to_bind {
            self.value_bindings.insert(*name, *value);
        }
        // The **names** of this procedure's comptime parameters, whether or not values are known
        // (ADR-0089 §2). A *template* has names and no values, and that is exactly the case where an
        // array length naming one must be withheld rather than refused.
        let comptime_names: Vec<jr_base::Symbol> = declaration
            .params
            .iter()
            .filter(|p| p.comptime)
            .map(|p| p.name)
            .collect();
        for name in &comptime_names {
            self.comptime_param_names.insert(*name);
        }

        let mut comptime_params = Vec::with_capacity(declaration.params.len());
        for param in &declaration.params {
            let ty = match param.ty {
                // Parameter types live in `FileHir::type_refs`, not in
                // `Proc::type_refs`, which is always empty.
                Some(id) => self.resolve_type(ExprScope::TopLevel, id, param.name_span),
                None => PoolId::ERROR,
            };
            names.push(param.name);
            defaults.push(self.param_default(param, ty, foreign));
            // A `$N` parameter's *type* is ordinary and resolved above; the mark only affects when its
            // value is known (ADR-0087 §1), so its `ty` is real — which is what lets the body check.
            comptime_params.push(param.comptime);
            params.push(ty);
        }

        let ret = match declaration.ret {
            Some(id) => self.resolve_type(ExprScope::TopLevel, id, declaration.span),
            // Omitting the arrow means `void`, which is a real type (ADR-0015 §3)
            // so that a procedure type's return field is total.
            None => PoolId::VOID,
        };

        // Every `#foreign` procedure is implicitly `#c_call` (ADR-0001), and the context kind is
        // part of the type's identity — a function pointer of one kind must never satisfy the other.
        //
        // **Both** conditions, not just `foreign`. This read `foreign.is_some()` alone, which was
        // correct when written: `#c_call` was unparseable, so `#foreign` was the only route to
        // `CCall`. ADR-0057 made the directive real and left this behind, so an explicit
        // `raw :: () #c_call { }` interned as `ContextKind::Jairs` — its *type* claiming a context
        // its ABI does not take. Nothing reads the kind for the ABI yet (lowering and both back
        // ends ask the HIR flags directly), which is exactly why it was invisible: a wrong answer
        // waiting for the first consumer, which is the shape of a function-pointer type check.
        let context = if declaration.foreign.is_some() || declaration.c_call {
            ContextKind::CCall
        } else {
            ContextKind::Jairs
        };

        // `#no_abc` on a `#foreign` declaration is refused (ADR-0058 §3). A procedure with no body
        // has no index in it to leave unchecked, so the directive could only ever be a word that
        // does nothing — and a directive that is silently ignored is worse than one that is
        // rejected, because nothing tells the writer their intent did not land.
        //
        // Here rather than in the check phase because it is a property of the *declaration*: it
        // needs no types, no body and no expression context.
        if declaration.no_abc && declaration.foreign.is_some() {
            self.diags.push(
                Diagnostic::error(
                    declaration.span,
                    "`#no_abc` is not allowed on a `#foreign` procedure",
                )
                .with_code(E0255)
                .with_note("a `#foreign` procedure has no body, so it has no index to check")
                .with_help("remove the `#no_abc`"),
            );
        }

        // Clear this signature's bindings before returning, so they never leak into the next signature
        // computed on the same context (ADR-0081 §1: the map is empty outside a polymorphic signature).
        for &var in &vars_to_bind {
            self.type_bindings.remove(&var);
        }
        // Clear this signature's comptime-value bindings, exactly as the type bindings above are cleared:
        // two instantiations of one template share the parameter name `N` with different values, so
        // leaving one set would give the next signature the wrong length. The *body* gets its own seeding
        // in `check_file`'s body loop (ADR-0089 §1).
        for (name, _) in &values_to_bind {
            self.value_bindings.remove(name);
        }
        for name in &comptime_names {
            self.comptime_param_names.remove(name);
        }

        let ty = self.pool.proc_type(params.clone(), ret, context);
        let sig = ProcSig {
            params,
            names,
            defaults,
            ret,
            poly_vars,
            comptime_params,
            ty,
        };
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
