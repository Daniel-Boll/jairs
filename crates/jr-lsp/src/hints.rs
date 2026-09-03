//! Signature help and inlay hints: telling the user what the source does not say.
//!
//! # Why `locate` is not enough for signature help
//!
//! [ADR-0031](../../../docs/adr/0031-code-actions-and-hints.md) §6. [`locate()`](crate::locate())
//! returns the *innermost* expression containing the offset, which inside `add(2, |)` is
//! the argument — or nothing at all, when the cursor sits on whitespace between a comma and
//! the closing paren. Signature help needs the **enclosing call** and the index of the
//! argument the cursor is in, and neither falls out of a narrowing scan. Hence
//! [`enclosing_call`], which widens instead.
//!
//! # Why the active parameter is counted from spans
//!
//! The buffer usually does not parse mid-call — that is when help is wanted — but the
//! arguments already typed do, and lowering recorded their spans. Counting the arguments
//! that end before the cursor is therefore reliable exactly when a textual comma count
//! would be fooled by a comma inside a nested call.
//!
//! # Why there are only two kinds of inlay hint
//!
//! §7. A hint earns its place by showing something absent from the text: the inferred type
//! of a `:=`, and the *value* a `#run` produced. The second is the one nothing outside this
//! project can offer, and it is why hints are in this wave — it makes compile-time
//! execution visible, which `PLAN.md` §1.4 could previously only assert through a MIR
//! snapshot.
//!
//! A hint is never emitted for a type that renders `<unknown>`: it would be noise that
//! looks like a compiler bug, and no hint already means "nothing useful is known".

use jr_db::{Db, ModuleSearchPaths, SourceFile};
use jr_hir::{Expr, ExprId, ExprScope, FileHir, ItemKind};
use lsp_types::{
    InlayHint, InlayHintKind, InlayHintLabel, ParameterInformation, ParameterLabel, SignatureHelp,
    SignatureInformation,
};

use crate::position::{Encoding, Positions};
use crate::render::{Decl, container_of, type_name};

/// A call the cursor is inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnclosingCall {
    /// Which expression arena the ids index.
    pub scope: ExprScope,
    /// The call expression itself.
    pub call: ExprId,
    /// Which argument the cursor is in, counting from zero.
    ///
    /// Equal to the argument count when the cursor is past the last argument — a call being
    /// typed has a position for an argument that does not exist yet.
    pub argument: usize,
}

/// The innermost call whose span contains `offset`.
///
/// Widening rather than narrowing: `locate` would answer the argument, and the argument does
/// not know its own index. Ties on span width keep the *later* node for the same reason
/// `locate` does — lowering emits an inner expression after the outer one containing it.
#[must_use]
pub fn enclosing_call(hir: &FileHir, offset: jr_base::TextSize) -> Option<EnclosingCall> {
    let mut best: Option<(u32, EnclosingCall)> = None;

    let mut consider = |scope: ExprScope, exprs: &[Expr]| {
        for (index, expr) in exprs.iter().enumerate() {
            let Expr::Call { args, span, .. } = expr else {
                continue;
            };
            // Inclusive of the end so that a cursor on the closing paren still gets help;
            // `locate` is exclusive there because two token spans would tie, but a call's
            // span ends at `)` and nothing else claims that byte.
            if !(span.start() <= offset && offset <= span.end()) {
                continue;
            }
            let width = u32::from(span.end()) - u32::from(span.start());
            let argument = args
                .iter()
                .filter(|arg| arg_span(exprs, **arg).is_some_and(|span| span.end() <= offset))
                .count();
            let candidate = EnclosingCall {
                scope,
                call: ExprId::from_usize(index),
                argument,
            };
            if best.is_none_or(|(current, _)| width <= current) {
                best = Some((width, candidate));
            }
        }
    };

    consider(ExprScope::TopLevel, &hir.exprs);
    for (index, body) in hir.bodies.iter().enumerate() {
        consider(
            ExprScope::Body(jr_hir::BodyId::from_usize(index)),
            &body.exprs,
        );
    }

    best.map(|(_, call)| call)
}

/// One argument's span, from the same arena the call came from.
fn arg_span(exprs: &[Expr], id: ExprId) -> Option<jr_base::Span> {
    exprs.get(id.index()).map(Expr::span)
}

/// The signature of the call the cursor is inside, with the active parameter marked.
///
/// `None` when the cursor is not in a call, or the callee does not resolve to a procedure
/// this file can see. The signature text comes from [`Decl`], so it cannot disagree with the
/// hover card or the completion item for the same procedure (ADR-0028 §1).
#[must_use]
pub fn signature_help(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    encoding: Encoding,
    position: lsp_types::Position,
) -> Option<SignatureHelp> {
    let text = file.text(db);
    let index = jr_db::line_index(db, file);
    let positions = Positions::new(text.as_ref(), &index, encoding);
    let offset = positions.offset(position);

    let hir = jr_db::file_hir(db, file);
    let found = enclosing_call(hir.as_ref(), offset)?;

    let (callee, arity) = callee_of(hir.as_ref(), found)?;
    let resolve = jr_db::resolved(db, file, search_paths).map;
    let res = resolve.get(found.scope, callee)?;

    // Every query before the pool lock: a query locks the pool itself and the mutex is not
    // reentrant, which is the self-deadlock the completion wave found the hard way.
    let (target, item) = match res {
        jr_hir::Res::Item(item) => (file, item),
        jr_hir::Res::Imported(import, name) => {
            let module = imported_module(db, hir.as_ref(), search_paths, import)?;
            let other = jr_db::file_hir(db, module);
            (module, other.scope.get(name)?)
        }
        // A local, a parameter or an unresolved name: Jairs-0 has no procedure values, so
        // there is no signature to show rather than a signature this cannot find.
        // A promoted name joins them: a field cannot hold a procedure in Jairs, so a promoted
        // callee has no signature to show either.
        jr_hir::Res::Local(_)
        | jr_hir::Res::Param(_)
        | jr_hir::Res::Promoted { .. }
        | jr_hir::Res::Error => return None,
    };

    let hir = jr_db::file_hir(db, target);
    let sigs = jr_db::file_signatures(db, target, search_paths).signatures;
    let docs = jr_db::file_docs(db, target);
    let container = container_of(target.path(db).as_ref());
    let proc = proc_of(hir.as_ref(), item)?;
    let params = hir.procs.get(proc.index())?.params.clone();

    let pool = db.read_pool();
    let label = Decl {
        hir: hir.as_ref(),
        sigs: sigs.as_ref(),
        docs: docs.as_ref(),
        consts: None,
        pool: &pool,
        interner: db.interner(),
        container: &container,
    }
    .signature(item)?;

    let parameters: Vec<ParameterInformation> = params
        .iter()
        .enumerate()
        .map(|(i, param)| {
            let ty = sigs
                .proc_sig(proc)
                .and_then(|sig| sig.params.get(i).copied())
                .map(|ty| type_name(&pool, sigs.as_ref(), ty));
            ParameterInformation {
                // A string rather than an offset pair: the offsets would have to index the
                // label this module just built, and a mismatch there highlights the wrong
                // text. A client resolves the string against the label itself.
                label: ParameterLabel::Simple(match ty {
                    Some(ty) => format!("{}: {ty}", db.interner().resolve(param.name)),
                    None => db.interner().resolve(param.name).to_owned(),
                }),
                documentation: None,
            }
        })
        .collect();
    drop(pool);

    // Clamped rather than left out of range: a call with too many arguments still
    // highlights something, where an out-of-range index makes a client highlight nothing at
    // the moment the user most needs to see which parameter they overran.
    let active = found
        .argument
        .min(parameters.len().saturating_sub(1))
        .min(arity);

    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label,
            documentation: None,
            parameters: Some(parameters),
            active_parameter: Some(active as u32),
        }],
        active_signature: Some(0),
        active_parameter: Some(active as u32),
    })
}

/// The callee expression of a call, and how many arguments it was given.
fn callee_of(hir: &FileHir, found: EnclosingCall) -> Option<(ExprId, usize)> {
    let exprs = match found.scope {
        ExprScope::TopLevel => &hir.exprs,
        ExprScope::Body(body) => &hir.bodies.get(body.index())?.exprs,
    };
    match exprs.get(found.call.index())? {
        Expr::Call { callee, args, .. } => Some((*callee, args.len())),
        _ => None,
    }
}

/// The procedure a named item declares.
fn proc_of(hir: &FileHir, item: jr_hir::ItemId) -> Option<jr_hir::ProcId> {
    match &hir.items.get(item.index())?.kind {
        ItemKind::Const {
            value: jr_hir::ConstValue::Proc(proc),
        } => Some(*proc),
        _ => None,
    }
}

/// The loaded file an `#import` item names.
fn imported_module(
    db: &dyn Db,
    hir: &FileHir,
    search_paths: ModuleSearchPaths,
    import: jr_hir::ItemId,
) -> Option<SourceFile> {
    let ItemKind::Import { path, .. } = &hir.items.get(import.index())?.kind else {
        return None;
    };
    let lookup = jr_db::module_file(db, search_paths, std::sync::Arc::from(path.as_str()));
    let found = lookup.found?;
    db.source_file_for_path(found.to_string_lossy().as_ref())
}

/// Inferred types on `:=`, and the values `#run` produced.
///
/// Computed for the requested range, as the protocol intends, so that a large file does not
/// render its whole body on every scroll.
#[must_use]
pub fn inlay_hints(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    encoding: Encoding,
    range: lsp_types::Range,
) -> Vec<InlayHint> {
    let text = file.text(db);
    let index = jr_db::line_index(db, file);
    let positions = Positions::new(text.as_ref(), &index, encoding);
    let hir = jr_db::file_hir(db, file);

    let first = range.start.line;
    let last = range.end.line;
    let in_range = |span: jr_base::Span| {
        let line = positions.range(span).start.line;
        first <= line && line <= last
    };

    // Every query before the pool lock (see `signature_help`).
    let types = jr_db::checked(db, file, search_paths).types;
    let sigs = jr_db::file_signatures(db, file, search_paths).signatures;
    let consts = jr_db::file_consts(db, file, search_paths).values;
    let docs = jr_db::file_docs(db, file);
    let container = container_of(file.path(db).as_ref());

    let mut out = Vec::new();
    let pool = db.read_pool();

    // `:=` locals. A local with an explicit annotation is skipped: the type is already on
    // screen, and repeating it is noise.
    for (body_index, body) in hir.bodies.iter().enumerate() {
        let body_id = jr_hir::BodyId::from_usize(body_index);
        for (local_index, local) in body.locals.iter().enumerate() {
            if local.ty.is_some() || !in_range(local.name_span) {
                continue;
            }
            let Some(ty) = types.local_type(body_id, jr_hir::LocalId::from_usize(local_index))
            else {
                continue;
            };
            let rendered = type_name(&pool, sigs.as_ref(), ty);
            if rendered == "<unknown>" {
                continue;
            }
            out.push(InlayHint {
                position: positions.range(local.name_span).end,
                label: InlayHintLabel::String(format!(": {rendered}")),
                kind: Some(InlayHintKind::TYPE),
                text_edits: None,
                tooltip: None,
                padding_left: Some(false),
                padding_right: Some(false),
                data: None,
            });
        }
    }

    // `#run` values, and file-level constants whose value came from one. The value is what
    // nothing else can show: `COMPUTED :: #run add(2, 3)` says nothing about `5` in its
    // text, and the fold happened in the bytecode VM (ADR-0018 §3).
    let decl = Decl {
        hir: hir.as_ref(),
        sigs: sigs.as_ref(),
        docs: docs.as_ref(),
        consts: Some(consts.as_ref()),
        pool: &pool,
        interner: db.interner(),
        container: &container,
    };
    for (index, item) in hir.items.iter().enumerate() {
        let id = jr_hir::ItemId::from_usize(index);
        if !in_range(item.span) || !is_run_constant(hir.as_ref(), id) {
            continue;
        }
        let Some(value) = decl.value_of(id) else {
            continue;
        };
        out.push(InlayHint {
            position: positions.range(item.span).end,
            label: InlayHintLabel::String(format!(" = {value}")),
            kind: Some(InlayHintKind::TYPE),
            text_edits: None,
            tooltip: None,
            padding_left: Some(false),
            padding_right: Some(false),
            data: None,
        });
    }

    out
}

/// Whether an item is a constant whose value is a `#run`.
///
/// Restricted to `#run` on purpose: a hint on `FOUR :: 4` would restate the text, and the
/// whole rule for a hint is that it shows what the source does not say.
fn is_run_constant(hir: &FileHir, item: jr_hir::ItemId) -> bool {
    let Some(item) = hir.items.get(item.index()) else {
        return false;
    };
    let ItemKind::Const {
        value: jr_hir::ConstValue::Expr { expr, .. },
    } = &item.kind
    else {
        return false;
    };
    matches!(hir.exprs.get(expr.index()), Some(Expr::Run(_, _)))
}
