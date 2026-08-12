//! Appending polymorphic-procedure instantiations to a `FileHir` (ADR-0082 §2).
//!
//! An instantiation is a **new procedure**: a clone of a `$T` template with its variable bound to a
//! concrete type. The compiler keys everything by `ProcId`, so the clone must be a real entry in the
//! `procs` arena with its own `Item`, body and parameter `TypeRef`s — after which the signature phase,
//! the checker and both back ends treat it as an ordinary procedure (ADR-0082 §3, §4).
//!
//! # Why a clone and not a substitution
//!
//! The clone's parameter and return `TypeRef`s are **left saying `$T`/`T`**; the binding to the concrete
//! type lives in [`crate::hir::FileHir::proc_bindings`], which the signature phase reads. This keeps `jr-hir` from
//! having to rewrite type syntax it cannot always express (an anonymous struct type has no `TypeRef`
//! spelling), and puts the substitution where resolved types already live (ADR-0082 §2). The clone is
//! therefore a *structural* copy — new arena indices — with no type rewriting at all.
//!
//! # Why the body's arenas copy wholesale
//!
//! A [`crate::hir::Body`] owns its `exprs`, `stmts`, `type_refs` and `locals` in its own arenas, indexed from 0, so
//! cloning one is a deep copy of self-contained vectors — no index remapping within the body. Only the
//! *procedure's* parameter and return `TypeRefId`s index the shared [`crate::hir::FileHir::type_refs`], so those are
//! copied into that arena and the ids remapped. Nothing else crosses a body boundary.

use jr_base::Symbol;
use jr_pool::{IntKind, Item as PoolItem, Pool, PoolId};

use crate::hir::{
    ConstValue, Expr, FileHir, Item, ItemKind, Literal, Param, ParamId, Proc, ProcId, Res, TypeRef,
    TypeRefId,
};
use crate::resolve::ExprScope;

/// One instantiation to append: the template procedure, its type bindings, and — for a comptime-value
/// template — the baked value of each `$N` parameter (ADR-0083 §2, ADR-0088 §3).
///
/// Two shapes carried in one struct, because the append is otherwise the same and a second variant would
/// duplicate the clone. `bindings` is the `$T` side (ADR-0083); `comptime_values` is the `$N` side, one
/// entry per procedure parameter in source order — `Some(value)` for a `$N` parameter that gets the
/// baked constant, `None` for an ordinary parameter that keeps its runtime slot. Empty
/// `comptime_values` means "no comptime parameters", the ordinary `$T`-only instantiation.
#[derive(Debug, Clone)]
pub struct Instantiation {
    /// The polymorphic template being instantiated.
    pub template: ProcId,
    /// Each type variable and the concrete type it is bound to, in the template's first-seen order.
    pub bindings: Vec<(Symbol, PoolId)>,
    /// For each of the template's parameters, `Some(value)` for a `$N` parameter baked to that value or
    /// `None` for a runtime one (ADR-0088 §3). Empty for a `$T`-only template (ordinary `$T` path).
    pub comptime_values: Vec<Option<PoolId>>,
    /// Where this instantiation was demanded, for a diagnostic's backtrace (ADR-0128).
    ///
    /// `None` when the caller has no site to offer — a `jr-hir` unit test appending an instantiation
    /// directly, for instance. The appender then records nothing, so a missing site costs a backtrace
    /// rather than producing a wrong one.
    pub site: Option<InstantiationSite>,
}

/// Where an instantiation was demanded, and how to describe it in a backtrace.
///
/// # Why the scope is kept beside the rendered frame
///
/// The [`frame`](Self::frame) alone gives one `note:` line. A **chain** — `main` calls `outer`, whose
/// body calls `inner` — needs to know which *body* the call sat in, so the walk can ask whether that
/// body's own procedure was itself an instantiation. [`ExprScope::Body`] carries exactly that, and
/// `check_file` already holds the `BodyId → ProcId` map, so keeping the scope turns one frame into a
/// full backtrace with no new bookkeeping.
#[derive(Debug, Clone)]
pub struct InstantiationSite {
    /// The rendered frame: the call's span, and a description like ``in instantiation of `f($T = bool)` ``.
    pub frame: jr_diag::InstantiationFrame,
    /// The expression arena the demanding call sat in, or `None` for a top-level one.
    pub called_from: Option<ExprScope>,
}

/// Appends one procedure per instantiation to `hir`, returning each instantiation's new `ProcId`
/// parallel to the input (ADR-0082 §2).
///
/// De-duplication is the caller's: it passes one `Instantiation` per **distinct** structural key
/// (ADR-0005), so this appends exactly that many procedures. The returned `ProcId`s let the caller
/// record the call → instantiation redirect.
///
/// Each appended procedure gets:
/// - a clone of the template's [`Proc`], with its parameter and return `TypeRef`s copied into
///   `hir.type_refs` and the ids remapped;
/// - a clone of the template's body (if any), appended to `hir.bodies`, its own arenas copied wholesale;
/// - an [`Item`] so the signature phase (which walks items) computes its signature;
/// - a `proc_bindings` entry so that phase resolves `$T`/`T` to the concrete type.
///
/// The `Item` gets a **synthetic, unexported** name and is **not** added to the file scope: the signature
/// phase computes a signature only for a *named* item (`item_signature` returns early otherwise), so the
/// instantiation needs a name — but it is reached only through the redirect, never by lookup, so it stays
/// out of `scope` to avoid shadowing. The interner supplies a fresh name per instantiation.
pub fn expand_instantiations(
    hir: &mut FileHir,
    interner: &jr_base::Interner,
    pool: &Pool,
    instantiations: &[Instantiation],
) -> Vec<ProcId> {
    let mut new_ids = Vec::with_capacity(instantiations.len());
    for (n, inst) in instantiations.iter().enumerate() {
        new_ids.push(append_one(hir, interner, pool, n, inst));
    }
    new_ids
}

/// Appends one instantiation and returns its new `ProcId`.
fn append_one(
    hir: &mut FileHir,
    interner: &jr_base::Interner,
    pool: &Pool,
    n: usize,
    inst: &Instantiation,
) -> ProcId {
    let template = hir.proc(inst.template).clone();

    // For a comptime-value instantiation (ADR-0088 §3), the clone **drops** each `$N` parameter — the
    // caller passes no value for it — and its body's references to that parameter are rewritten to a
    // literal. `keep_map[i]` is `Some(new_index)` for a runtime parameter kept at that position in the
    // clone's parameter list, or `None` for a dropped comptime one. An empty `comptime_values` (the
    // ordinary `$T`-only path) keeps every parameter and leaves the map identity.
    let comptime_baked = !inst.comptime_values.is_empty();
    let mut keep_map: Vec<Option<u32>> = Vec::with_capacity(template.params.len());
    let mut next_kept: u32 = 0;
    for i in 0..template.params.len() {
        let dropped = inst.comptime_values.get(i).and_then(|v| *v).is_some();
        if dropped {
            keep_map.push(None);
        } else {
            keep_map.push(Some(next_kept));
            next_kept += 1;
        }
    }

    // Copy the parameter and return `TypeRef`s into the shared arena, remapping their ids. A `$T`
    // parameter keeps its `TypeRef::Poly` — the binding, not a rewrite, is what makes it concrete.
    // A `$N` (dropped) parameter is skipped, and the clone's `comptime` flag is cleared on every kept
    // parameter (an instantiation is ordinary code — its `$N`s no longer exist).
    let params: Vec<Param> = template
        .params
        .iter()
        .enumerate()
        .filter(|(i, _)| keep_map[*i].is_some())
        .map(|(_, param)| Param {
            name: param.name,
            name_span: param.name_span,
            ty: param.ty.map(|t| copy_type_ref(hir, t)),
            using: param.using,
            comptime: false,
            default: param.default,
        })
        .collect();
    let ret = template.ret.map(|t| copy_type_ref(hir, t));

    // Copy the body wholesale into a new arena slot, if the template has one. Then rewrite it: a
    // reference to a *dropped* comptime parameter becomes a `Literal` of its baked value; a reference
    // to a *kept* runtime parameter has its `Res::Param` index remapped through `keep_map`.
    let body = template.body.map(|b| {
        let mut cloned = hir.body(b).clone();
        if comptime_baked {
            let expr_count = cloned.exprs.len();
            for expr_index in 0..expr_count {
                if let Expr::Name { res, span, .. } = &cloned.exprs[expr_index] {
                    let (new_expr, replaced_span) = match res {
                        Res::Param(pid) => {
                            let i = pid.index();
                            match keep_map.get(i).copied().flatten() {
                                None => {
                                    // Dropped: bake the value as a literal.
                                    let value = inst.comptime_values[i]
                                        .expect("keep_map[i] is None ⇒ comptime_values[i] is Some");
                                    let lit = literal_from_value(pool, value);
                                    (Expr::Literal(lit, *span), *span)
                                }
                                Some(new_i) => {
                                    // Kept: remap the parameter index. Name and span unchanged.
                                    let (name, span_copy) = match &cloned.exprs[expr_index] {
                                        Expr::Name { name, span, .. } => (*name, *span),
                                        _ => unreachable!(),
                                    };
                                    (
                                        Expr::Name {
                                            name,
                                            span: span_copy,
                                            res: Res::Param(ParamId::from_usize(new_i as usize)),
                                        },
                                        span_copy,
                                    )
                                }
                            }
                        }
                        _ => continue,
                    };
                    cloned.exprs[expr_index] = new_expr;
                    // `expr_spans` is parallel; keep it aligned even though the span did not change,
                    // to make the invariant local to this rewrite rather than an unstated assumption.
                    if cloned.expr_spans.len() > expr_index {
                        cloned.expr_spans[expr_index] = replaced_span;
                    }
                }
            }
        }
        let id = crate::hir::BodyId::from_usize(hir.bodies.len());
        hir.bodies.push(cloned);
        id
    });

    let proc = Proc {
        params,
        c_call: template.c_call,
        no_abc: template.no_abc,
        expand: template.expand,
        modify: template.modify,
        notes: template.notes.clone(),
        ret,
        body,
        foreign: template.foreign.clone(),
        span: template.span,
        type_refs: Vec::new(),
    };
    let proc_id = ProcId::from_usize(hir.procs.len());
    hir.procs.push(proc);

    // Recorded here because this is the first point at which the clone's `ProcId` exists, and the site
    // has to be keyed on the clone rather than the template: two instantiations of one template were
    // demanded from different call sites, and attributing a diagnostic to the template's own span would
    // name the code the reader did not write (ADR-0128 §2).
    if let Some(site) = inst.site.clone() {
        hir.instantiation_sites.push((proc_id, site));
    }

    // A **synthetic, unexported** name so the signature phase computes this procedure's signature — it
    // returns early for an unnamed item. The name cannot collide with a source name (a `$` is not a valid
    // identifier character), and the item is deliberately **not** added to `hir.scope`, so the name never
    // resolves: the instantiation is reached only through the MIR redirect (ADR-0082 §2, fork 4).
    let synthetic = interner.intern(&format!("$inst{n}"));
    hir.items.push(Item {
        name: Some(synthetic),
        exported: false,
        span: template.span,
        name_span: template.span,
        kind: ItemKind::Const {
            value: ConstValue::Proc(proc_id),
        },
    });

    // Record a binding per variable for the signature phase to resolve each `$T`/`T` against
    // (ADR-0083 §2). Empty for a `$N`-only instantiation, so this loop is a no-op there.
    for (var, ty) in &inst.bindings {
        hir.proc_bindings.push((proc_id, *var, *ty));
    }

    // **Clone the `#modify` predicate for this instantiation** (ADR-0094 §2). The template lowered it once
    // as a synthetic no-parameter `bool` procedure; the clone gets *this* instantiation's `proc_bindings`,
    // so `type_info(T)` inside it describes the bound type (ADR-0092 §1). Cloned rather than shared because
    // two instantiations of one template must evaluate the predicate against *different* bindings — sharing
    // one procedure would evaluate it once and apply the answer to both, which is the wrong answer for at
    // least one of them.
    if let Some(pred) = template.modify {
        let cloned = clone_predicate(hir, interner, pool, n, pred, inst);
        hir.modify_predicates.push((proc_id, cloned));
    }

    // Record each **baked comptime value** under this instantiation's `ProcId` and the parameter's *name*
    // (ADR-0089 §1). The body's uses were already rewritten to literals above; this is for a use in a
    // *type* — `buf: [N]s64` — which sema resolves by name and cannot see a literal for. The value-side
    // counterpart of `proc_bindings`, read the same way.
    for (i, value) in inst.comptime_values.iter().enumerate() {
        if let (Some(value), Some(param)) = (value, template.params.get(i)) {
            hir.param_values.push((proc_id, param.name, *value));
        }
    }

    proc_id
}

/// Decodes a `PoolId` holding a compile-time value into a HIR literal, for baking a `$N` parameter's
/// value into an instantiation's body (ADR-0088 §3).
///
/// Handles `IntValue` and `BoolValue`, which are the only comptime-value shapes this sub-wave admits —
/// a `$N: s64` or `$N: bool` parameter. Any other `Item` is a compiler bug the caller should have
/// refused earlier; it lowers here to a poisoned integer literal rather than panicking, so the
/// instantiation still checks (as an error) rather than crashing the compile.
fn literal_from_value(pool: &Pool, value: PoolId) -> Literal {
    match *pool.item(value) {
        PoolItem::IntValue { ty, bits } => {
            // Decode against the value's integer type — the value-side counterpart of the type-side
            // `IntKind::from` a signature already resolves. Every integer *type* the pool holds is an
            // `IntType`, so this arm covers `s8`..`s64` and `u8`..`u64` uniformly; anything else is a
            // compiler bug the caller should have refused.
            let value = match *pool.item(ty) {
                PoolItem::IntType {
                    signed,
                    bits: width,
                } => IntKind {
                    signed,
                    bits: width,
                }
                .decode(bits),
                _ => i128::from(bits as i64),
            };
            Literal::Int {
                value,
                radix: 10,
                overflowed: false,
            }
        }
        PoolItem::BoolValue(v) => Literal::Bool(v),
        _ => Literal::Int {
            value: 0,
            radix: 10,
            overflowed: true,
        },
    }
}

/// Copies a `TypeRef` (and, recursively, the ones it points at) into `hir.type_refs`, returning the new
/// id. The parameter/return types index the *shared* arena, so an instantiation needs its own copies to
/// avoid two procedures aliasing one `TypeRefId` — harmless today (the syntax is identical) but a trap if
/// a later pass ever mutated one.
fn copy_type_ref(hir: &mut FileHir, id: TypeRefId) -> TypeRefId {
    let cloned = match hir.type_refs[id.index()].clone() {
        TypeRef::Pointer(inner) => TypeRef::Pointer(copy_type_ref(hir, inner)),
        TypeRef::Array {
            elem,
            len,
            len_name,
            len_span,
        } => TypeRef::Array {
            elem: copy_type_ref(hir, elem),
            len,
            len_name,
            len_span,
        },
        TypeRef::View { elem } => TypeRef::View {
            elem: copy_type_ref(hir, elem),
        },
        // A `$T`, a name, an inline aggregate or an error copies as itself: none references another entry
        // in this arena (an inline struct/enum indexes its own arena, unchanged by the copy).
        other => other,
    };
    let new_id = TypeRefId::from_usize(hir.type_refs.len());
    hir.type_refs.push(cloned);
    new_id
}

/// Clones a `#modify` predicate for one instantiation, binding it to that instantiation's types
/// (ADR-0094 §2).
///
/// The predicate is an ordinary synthetic procedure the template already lowered, so this is the same
/// structural copy [`append_one`] makes: its body's arenas copy wholesale, its return `TypeRef` is copied
/// into the shared arena, and it gets a named synthetic item so the signature phase computes its signature.
/// What differs is only which `proc_bindings` it carries.
fn clone_predicate(
    hir: &mut FileHir,
    interner: &jr_base::Interner,
    _pool: &Pool,
    n: usize,
    pred: ProcId,
    inst: &Instantiation,
) -> ProcId {
    let template = hir.proc(pred).clone();
    let ret = template.ret.map(|t| copy_type_ref(hir, t));
    let body = template.body.map(|b| {
        let cloned = hir.body(b).clone();
        let id = crate::hir::BodyId::from_usize(hir.bodies.len());
        hir.bodies.push(cloned);
        id
    });
    let proc = Proc {
        params: Vec::new(),
        c_call: false,
        no_abc: false,
        expand: false,
        modify: None,
        notes: Vec::new(),
        ret,
        body,
        foreign: None,
        span: template.span,
        type_refs: Vec::new(),
    };
    let proc_id = ProcId::from_usize(hir.procs.len());
    hir.procs.push(proc);

    let synthetic = interner.intern(&format!("$modinst{n}"));
    hir.items.push(Item {
        name: Some(synthetic),
        exported: false,
        span: template.span,
        name_span: template.span,
        kind: ItemKind::Const {
            value: ConstValue::Proc(proc_id),
        },
    });
    for (var, ty) in &inst.bindings {
        hir.proc_bindings.push((proc_id, *var, *ty));
    }
    proc_id
}
