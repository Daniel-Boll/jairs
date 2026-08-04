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
use jr_pool::PoolId;

use crate::hir::{ConstValue, FileHir, Item, ItemKind, Param, Proc, ProcId, TypeRef, TypeRefId};

/// One instantiation to append: the template procedure and the type its single `$T` binds to.
#[derive(Debug, Clone, Copy)]
pub struct Instantiation {
    /// The polymorphic template being instantiated.
    pub template: ProcId,
    /// The type its `$T` variable is bound to.
    pub bound: PoolId,
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
    instantiations: &[Instantiation],
) -> Vec<ProcId> {
    let mut new_ids = Vec::with_capacity(instantiations.len());
    for (n, inst) in instantiations.iter().enumerate() {
        new_ids.push(append_one(hir, interner, n, *inst));
    }
    new_ids
}

/// Appends one instantiation and returns its new `ProcId`.
fn append_one(
    hir: &mut FileHir,
    interner: &jr_base::Interner,
    n: usize,
    inst: Instantiation,
) -> ProcId {
    let template = hir.proc(inst.template).clone();

    // Copy the parameter and return `TypeRef`s into the shared arena, remapping their ids. A `$T`
    // parameter keeps its `TypeRef::Poly` — the binding, not a rewrite, is what makes it concrete.
    let params: Vec<Param> = template
        .params
        .iter()
        .map(|param| Param {
            name: param.name,
            name_span: param.name_span,
            ty: param.ty.map(|t| copy_type_ref(hir, t)),
            using: param.using,
            default: param.default,
        })
        .collect();
    let ret = template.ret.map(|t| copy_type_ref(hir, t));

    // Copy the body wholesale into a new arena slot, if the template has one.
    let body = template.body.map(|b| {
        let cloned = hir.body(b).clone();
        let id = crate::hir::BodyId::from_usize(hir.bodies.len());
        hir.bodies.push(cloned);
        id
    });

    let proc = Proc {
        params,
        c_call: template.c_call,
        no_abc: template.no_abc,
        ret,
        body,
        foreign: template.foreign.clone(),
        span: template.span,
        type_refs: Vec::new(),
    };
    let proc_id = ProcId::from_usize(hir.procs.len());
    hir.procs.push(proc);

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

    // Record the binding for the signature phase to resolve `$T`/`T` against.
    let var = sole_poly_var(hir, inst.template);
    if let Some(var) = var {
        hir.proc_bindings.push((proc_id, var, inst.bound));
    }

    proc_id
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

/// The single `$T` variable a template introduces, if it has exactly one (this sub-wave's slice).
fn sole_poly_var(hir: &FileHir, proc: ProcId) -> Option<Symbol> {
    let mut found: Option<Symbol> = None;
    for param in &hir.proc(proc).params {
        if let Some(ty) = param.ty
            && let TypeRef::Poly(v) = hir.type_refs[ty.index()]
        {
            found = Some(v);
        }
    }
    found
}
