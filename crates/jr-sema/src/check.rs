//! The check phase: typing expressions, statements, and bodies.
//!
//! # What this phase owns
//!
//! Everything the signature phase does not: procedure bodies, the unnamed
//! file-level items (`#run`), and the `#foreign` library operand. It deliberately
//! does **not** re-type named file-level declarations — those were typed while
//! computing signatures, and typing them twice would either double every
//! diagnostic or, worse, reach a different answer.
//!
//! # Poison propagation is mandatory, not polite
//!
//! `jr_db::file_diagnostics` does not gate later phases on earlier ones: a file
//! that failed to parse is still lowered, resolved, and checked. Without poison
//! propagation every parse error would arrive here as an invented type error, and
//! the recovery quality the parser was built for would be undone by the checker.
//! So [`PoolId::ERROR`] flows through silently, and so do `TypeRef::Error`,
//! `Expr::Error` and `Res::Error`.
//!
//! # Two things the corpus needs that no ADR states
//!
//! - **`string` has `.data` and `.count`.** ADR-0004 fixes the layout as
//!   `{data: *u8, count: s64}` and says the fields are directly accessible;
//!   `valid/021` and `modules/Basic/module.jr` both rely on it. They are treated
//!   as pseudo-fields of the builtin rather than by making `string` a struct,
//!   because ADR-0015 §2 says a user struct of that shape is a *different* type.
//! - **Field access auto-dereferences.** `valid/015` writes `pp := *origin;
//!   pp.x = 1;`, so `.` looks through any number of pointers, and the result is
//!   assignable because a dereference always has an address.

use jr_base::{Interner, Span, Symbol, TextRange};
use jr_diag::{Diagnostic, Diagnostics};
use jr_hir::{
    AssignOp, BinOp, BodyId, Expr, ExprId, ExprScope, FileHir, ItemKind, Literal, ProcId, Res,
    ResolveMap, Stmt, StmtId, UnOp,
};
use jr_pool::{Item, Pool, PoolId};
use rustc_hash::FxHashMap;

use crate::code::{
    E0204, E0214, E0215, E0216, E0217, E0218, E0219, E0220, E0221, E0222, E0223, E0224, E0225,
    E0232, E0234, E0235, E0236, E0238, E0239, E0241, E0242, E0243, E0244, E0247, E0251, E0252,
    E0254, E0256, E0257, E0258, E0259, E0260, E0261, E0265, E0266,
};
use crate::ctx::{BodyEnv, Ctx, Mode};
use crate::map::TypeMap;
use crate::sigs::{FileSignatures, ProcSig, SigKind};

/// How a `Type_Info` field's type is checked.
#[derive(Debug, Clone, Copy)]
enum TypeInfoField {
    /// It must be exactly this type.
    Exact(PoolId),
    /// It must be *some* enum, checked by shape because an enum's `PoolId` depends on where it is
    /// declared — `Type_Info_Kind` lives beside `Type_Info` in `Basic`, so the compiler cannot name
    /// its id in advance.
    Enum,
}

/// The fields the compiler expects `Basic`'s `Type_Info` to have, in order (ADR-0075 §2).
///
/// **This is the contract with `modules/Basic`.** ADR-0075 §2 declares `Type_Info` in Jairs so it is
/// *spellable* — no compiler-declared type is — and this list is what stops that choice from becoming a
/// silent wrong offset: `type_info_struct` checks the declaration against it and raises E0265 on any
/// mismatch, so editing the struct produces a diagnostic rather than a wrong read.
///
/// Keep it in step with `Type_Info` in `modules/Basic/module.jr`.
const TYPE_INFO_FIELDS: &[(&str, TypeInfoField)] = &[
    ("kind", TypeInfoField::Enum),
    ("name", TypeInfoField::Exact(PoolId::STRING)),
    ("size", TypeInfoField::Exact(PoolId::S64)),
    ("alignment", TypeInfoField::Exact(PoolId::S64)),
];

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// One resolved argument position (ADR-0053 §1).
///
/// A `Vec<ArgSlot>` per call replaces the source-order argument list, so `jr-mir` lowers a call
/// without knowing what a parameter name is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgSlot {
    /// An argument the call site wrote, positionally or by name.
    Given(ExprId),
    /// A parameter's default value, already interned (ADR-0053 §2).
    ///
    /// A `PoolId` rather than an `ExprId` because the default belongs to the *declaration*, not to
    /// this call — so there is no expression in this body to point at, and MIR emits it as a
    /// constant operand directly.
    Default(PoolId),
}

/// What the check phase produces.
pub struct CheckOutput {
    /// The type of every expression and local the checker reached.
    pub types: TypeMap,
    /// Diagnostics about bodies, `#run` items, and foreign bindings.
    pub diagnostics: Diagnostics,
    /// Modules a *local* annotation named a type from.
    ///
    /// The signature phase records the same thing for file-level annotations, on
    /// `FileSignatures`. This phase needs its own channel because a local's annotation is
    /// resolved *here* and `FileSignatures` is an input to this phase rather than an
    /// output of it — so a record made on `Ctx::sigs` during a check is discarded when the
    /// context is dropped.
    ///
    /// Which is not hypothetical: `r: Rect;` in
    /// `tests/corpus/imports/valid/001-import-directory-module.jr` is a local, so without
    /// this field ADR-0031 §2's whole point would be defeated for exactly the file that
    /// motivated it.
    pub type_name_imports: Vec<String>,
    /// Which overload each operator expression resolved to (ADR-0048 §5).
    ///
    /// Keyed on `(ExprScope, ExprId)` for the reason `TypeMap` is: an `ExprId` is **not** unique
    /// within a file, because `FileHir::exprs` and every `Body::exprs` start at 0. A bare
    /// `ExprId` key silently collides and the last writer wins, which is a real bug that was
    /// found and fixed in `jr-hir`'s `ResolveMap`.
    ///
    /// The value is `(FileId, ProcId)` rather than a `ProcId`: an imported overload lives in
    /// another file's arena, so the file is what makes the pair a `ProcRef` at lowering time —
    /// the same shape ADR-0018 §5 chose for a cross-file callee.
    ///
    /// Recorded rather than recomputed so that `jr-mir` never re-runs resolution. Two
    /// implementations of one rule are two chances to disagree, which is why `jr-mir` reads
    /// `TypeMap` instead of typing expressions itself.
    pub operator_calls: FxHashMap<(ExprScope, ExprId), (jr_base::FileId, ProcId)>,
    /// The positional argument list of every call that used a named argument or a default
    /// (ADR-0053 §1).
    ///
    /// **Absent for an all-positional call with no defaults**, so the common path pays nothing and
    /// `jr-mir` falls back to the source order — which for such a call is already correct.
    pub filled_calls: FxHashMap<(ExprScope, ExprId), Vec<ArgSlot>>,
    /// The type each `type_info(T)` call describes (ADR-0075 §2).
    ///
    /// Recorded because a *type* is not an operand: nothing in the expression tree carries a `PoolId`,
    /// so lowering could not recover the argument by looking at the call. This is the same reason
    /// `operator_calls` is recorded rather than recomputed — one pass decides, and `jr-mir` reads.
    pub type_info_calls: FxHashMap<(ExprScope, ExprId), PoolId>,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Type-checks one file against its own and its imports' signatures.
///
/// `signatures` must be the output of [`file_signatures`](crate::file_signatures)
/// for this same file, and `imports` must carry one entry per `#import`, keyed by
/// the module name as written. A missing import is not an error here — name
/// resolution already reported it — so its names simply resolve to poison.
pub fn check_file(
    hir: &FileHir,
    file: jr_base::FileId,
    resolve: &ResolveMap,
    signatures: &FileSignatures,
    imports: &[(&str, &FileSignatures)],
    pool: &mut Pool,
    interner: &Interner,
) -> CheckOutput {
    // Struct field lists live in the pool, keyed by declaration. Recording them
    // explicitly — rather than trusting that some earlier phase interned them —
    // is what keeps this function callable on its own in a test.
    signatures.record_in(pool);
    for (_, imported) in imports {
        imported.record_in(pool);
    }

    let mut ctx = Ctx::new(
        hir,
        file,
        resolve,
        interner,
        pool,
        imports.to_vec(),
        Mode::Check,
    );
    ctx.sigs = signatures.clone();
    ctx.sigs.set_file(file);

    // Unnamed items. A named item's initialiser was typed by the signature
    // phase; a top-level `#run` has no name and so has no signature.
    for index in 0..hir.items.len() {
        let item = hir.item(jr_hir::ItemId::from_usize(index));
        if let ItemKind::Run { expr } = item.kind {
            ctx.check_expr(ExprScope::TopLevel, expr, None);
        }
    }

    // `Body` has no back-pointer to its `Proc`, so the mapping is recovered by
    // scanning. Without it a `Res::Param` could not be typed at all.
    let mut owner: FxHashMap<BodyId, ProcId> = FxHashMap::default();
    for (index, proc) in hir.procs.iter().enumerate() {
        if let Some(body) = proc.body {
            owner.insert(body, ProcId::from_usize(index));
        }
    }

    for index in 0..hir.procs.len() {
        ctx.check_foreign_binding(ProcId::from_usize(index));
    }

    for index in 0..hir.bodies.len() {
        let body = BodyId::from_usize(index);
        let sig = owner
            .get(&body)
            .and_then(|proc| ctx.sigs.proc_sig(*proc))
            .cloned();
        let (params, ret) = match sig {
            Some(sig) => (sig.params, sig.ret),
            // A body whose procedure has no signature only happens after an
            // error; poison rather than guessing `void`, which would make every
            // `return x;` in it wrong.
            None => (Vec::new(), PoolId::ERROR),
        };
        ctx.body = Some(BodyEnv {
            id: body,
            params,
            ret,
        });
        let root = hir.body(body).root;
        ctx.check_stmt(body, root);
        ctx.body = None;
    }

    // Collected before `ctx.sigs` is dropped. It started as a clone of the file's
    // signatures, so an entry the *signature* phase recorded is in here too; the union is
    // taken by the consumer rather than filtered here, because a module named in both
    // positions is used either way.
    let type_name_imports: Vec<String> = ctx
        .sigs
        .modules_used_in_type_position()
        .map(ToOwned::to_owned)
        .collect();

    CheckOutput {
        types: ctx.types,
        diagnostics: ctx.diags,
        type_name_imports,
        operator_calls: ctx.operator_calls,
        filled_calls: ctx.filled_calls,
        type_info_calls: ctx.type_info_calls,
    }
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    /// Checks one statement of a body.
    pub(crate) fn check_stmt(&mut self, body: BodyId, stmt: StmtId) {
        let scope = ExprScope::Body(body);
        let hir = self.hir;
        let statement = hir
            .body(body)
            .stmts
            .get(stmt.index())
            .cloned()
            .unwrap_or(Stmt::Error(self.nowhere()));

        match statement {
            Stmt::Block(stmts, _) => {
                for inner in stmts {
                    self.check_stmt(body, inner);
                }
            }
            Stmt::Local(local, _) => self.check_local(body, local),
            Stmt::LocalTuple {
                targets,
                call,
                span,
            } => self.check_local_tuple(body, &targets, call, span),
            Stmt::AssignTuple {
                targets,
                call,
                span,
            } => self.check_assign_tuple(scope, &targets, call, span),
            // Declared but never constructed by lowering; a nested item would be
            // E0207 long before it reached here.
            Stmt::Item(_, _) => {}
            Stmt::Expr(expr, _) => {
                // A discarded result is fine — `zero();` is a statement in
                // `valid/017` — so there is no expectation to impose.
                self.check_expr(scope, expr, None);
            }
            Stmt::Assign { lhs, op, rhs, span } => self.check_assign(scope, lhs, op, rhs, span),
            Stmt::If {
                cond, then, else_, ..
            } => {
                self.check_condition(scope, cond, "if");
                self.check_stmt(body, then);
                if let Some(branch) = else_ {
                    self.check_stmt(body, branch);
                }
            }
            Stmt::While {
                cond,
                body: loop_body,
                ..
            } => {
                self.check_condition(scope, cond, "while");
                self.check_stmt(body, loop_body);
            }
            Stmt::Return(expr, span) => self.check_return(scope, expr, span),
            Stmt::ReturnTuple(exprs, span) => self.check_return_tuple(scope, &exprs, span),
            Stmt::For {
                value,
                index,
                iterable,
                reverse: _,
                body: loop_body,
                label: _,
                span,
            } => self.check_for(body, value, index, &iterable, loop_body, span),
            // The deferred statement is checked once, where it was written. `jr-mir` duplicates
            // its lowering across exit paths, not its typing (ADR-0049 §3) — so a type error in a
            // `defer` is reported once rather than once per way out of the scope.
            Stmt::Defer(inner, _) => self.check_stmt(body, inner),
            // `push_context { … }` copies the context on entry (ADR-0063), so it needs one to copy.
            // A `#c_call` procedure has none, and this is the same refusal as `context` itself there
            // — E0254, reused because it means exactly "this needs a context and there isn't one"
            // (ADR-0063 §4). The message names `push_context` so the diagnostic points at what was
            // written. The block is checked regardless, so a body error inside it is still reported.
            Stmt::Switch { value, arms, span } => self.check_switch(body, value, &arms, span),
            // An `#insert`'s statements are checked **as if written here** (ADR-0072 §1) — no scope, no
            // separate environment, so a local the insert declares is in `self.locals` for the statements
            // after it. Nothing here can tell they came from a string, which is the evidence lowering put
            // them in the enclosing body rather than in a nested one.
            Stmt::Insert {
                stmts,
                operand,
                span: _,
            } => {
                // A **computed** operand is checked as an expression **expecting `string`** (ADR-0073 §1),
                // so a non-`string` operand is an ordinary type mismatch at its own span rather than a
                // bespoke refusal. Nothing here evaluates it — that is the operand pre-pass's job — but
                // checking it means the error a reader sees is about *their* expression. `None` for a
                // literal insert, whose text is already lowered into `stmts`.
                if let Some(op) = operand {
                    self.check_expr(scope, op, Some(PoolId::STRING));
                }
                for inner in stmts {
                    self.check_stmt(body, inner);
                }
            }
            Stmt::PushContext(inner, span) => {
                if self.body_is_c_call(body) {
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            "`push_context` is not available in a `#c_call` procedure",
                        )
                        .with_code(E0254)
                        .with_note(
                            "a `#c_call` procedure receives no implicit context to copy (ADR-0057 §3)",
                        )
                        .with_help("remove the `#c_call`, or manage the resource explicitly"),
                    );
                }
                self.check_stmt(body, inner);
            }
            // A label names a *loop*, not a value, so there is nothing to type. Whether the label
            // exists is `jr-mir`'s question, because its loop stack is the only place a loop's
            // identity lives (ADR-0049 §2).
            Stmt::Break(_, _) | Stmt::Continue(_, _) | Stmt::Error(_) => {}
        }
    }

    /// Types a `for` loop and records its variables' types (ADR-0049 §1).
    ///
    /// Three iterable shapes and no more: an array, a view, or a range. The *element* type is what
    /// the value variable gets; the index variable is always `s64`, because that is the type
    /// `.count` has (ADR-0004) and an index that disagreed with the length would need a conversion
    /// to compare with it.
    fn check_for(
        &mut self,
        body: BodyId,
        value: jr_hir::LocalId,
        index: Option<jr_hir::LocalId>,
        iterable: &jr_hir::ForIterable,
        loop_body: jr_hir::StmtId,
        span: Span,
    ) {
        let scope = ExprScope::Body(body);
        let element = match iterable {
            jr_hir::ForIterable::Sequence(expr) => {
                let mut seq = self.check_expr(scope, *expr, None);
                // Auto-deref, matching `check_index` and `check_slice`: `p: *[4]u8` iterates
                // through the pointer. The same loop in all three, so they cannot disagree about
                // how many levels.
                while let Some(inner) = self.pointee(seq) {
                    seq = inner;
                }
                match self.pool.item(seq) {
                    Item::ArrayType { elem, .. } | Item::ViewType { elem } => *elem,
                    _ => {
                        if seq != PoolId::ERROR {
                            let text = self.describe(seq);
                            self.diags.push(
                                Diagnostic::error(
                                    span,
                                    format!("cannot iterate over a value of type `{text}`"),
                                )
                                .with_code(E0247)
                                .with_note(
                                    "a `for` iterates a fixed-size array `[N]T`, a view `[]T`,                                      or a range `a..b`",
                                )
                                .with_help(
                                    "a user type cannot be iterated yet — that needs the                                      iteration protocol wave W5's macros unlock",
                                ),
                            );
                        }
                        PoolId::ERROR
                    }
                }
            }
            // Both ends are context-typed as `s64`, which is what makes `for i: 0..n` an `s64`
            // loop rather than an unconstrained one — the same context ADR-0039 §5 gives an index.
            jr_hir::ForIterable::Range { start, end } => {
                let s = self.check_expr(scope, *start, Some(PoolId::S64));
                let e = self.check_expr(scope, *end, Some(PoolId::S64));
                // An end that is not an integer is the mistake worth naming: `0..buf` reads as an
                // iteration and is a range over something with no ordering.
                for (ty, which) in [(s, "start"), (e, "end")] {
                    if ty != PoolId::ERROR && self.int_info(ty).is_none() {
                        let text = self.describe(ty);
                        self.diags.push(
                            Diagnostic::error(
                                span,
                                format!("the {which} of a range must be an integer, not `{text}`"),
                            )
                            .with_code(E0247),
                        );
                    }
                }
                PoolId::S64
            }
        };

        self.locals.insert((body, value), element);
        self.types.set_local(body, value, element);
        if let Some(index) = index {
            self.locals.insert((body, index), PoolId::S64);
            self.types.set_local(body, index, PoolId::S64);
        }
        self.check_stmt(body, loop_body);
    }

    /// Checks a local declaration and records the local's type.
    /// Checks `q, ok := f();` (ADR-0052 §2).
    ///
    /// Each target's type is the corresponding *result* type, so the locals are typed from the
    /// call rather than from an annotation — a destructuring declaration has no place to write one.
    fn check_local_tuple(
        &mut self,
        body: BodyId,
        targets: &[Option<jr_hir::LocalId>],
        call: ExprId,
        span: Span,
    ) {
        let scope = ExprScope::Body(body);
        let results = self.destructured_results(scope, call, targets.len(), span);
        for (index, target) in targets.iter().enumerate() {
            // A discard is typed as nothing, because it declares nothing.
            let Some(local) = target else { continue };
            let ty = results.get(index).copied().unwrap_or(PoolId::ERROR);
            self.locals.insert((body, *local), ty);
            self.types.set_local(body, *local, ty);
        }
    }

    /// Checks `q, ok = f();` (ADR-0052 §2).
    ///
    /// Each present target must be an assignable place whose type accepts the matching result, so
    /// this reuses `expect` and `is_place` rather than inventing a second assignability rule — two
    /// rules would be two chances to disagree about what `=` means.
    fn check_assign_tuple(
        &mut self,
        scope: ExprScope,
        targets: &[Option<ExprId>],
        call: ExprId,
        span: Span,
    ) {
        let results = self.destructured_results(scope, call, targets.len(), span);
        for (index, target) in targets.iter().enumerate() {
            let Some(target) = target else { continue };
            let target_ty = self.check_expr(scope, *target, None);
            if !self.is_place(scope, *target) {
                let text = self.describe(target_ty);
                self.diags.push(
                    Diagnostic::error(span, format!("cannot assign to this `{text}` target"))
                        .with_code(E0251)
                        .with_note("each target of a destructuring assignment must be assignable"),
                );
                continue;
            }
            if let Some(result) = results.get(index).copied() {
                self.expect(Some(target_ty), result, span);
            }
        }
    }

    /// The result types a destructuring statement's right-hand side produces, checking arity.
    ///
    /// Returns one type per *target* so the caller can index it positionally; a mismatch yields
    /// `PoolId::ERROR` entries, which propagate without inventing a second diagnostic per target.
    ///
    /// This is the one place arity is decided (ADR-0052 §2). Both statement forms ask it, so they
    /// cannot disagree about how many results a call has.
    fn destructured_results(
        &mut self,
        scope: ExprScope,
        call: ExprId,
        want: usize,
        span: Span,
    ) -> Vec<PoolId> {
        let ty = self.check_expr(scope, call, None);
        if ty == PoolId::ERROR {
            return vec![PoolId::ERROR; want];
        }
        let Some(elems) = self.pool.results_elems(ty).map(<[PoolId]>::to_vec) else {
            // One result, or none: a destructuring statement is the wrong form. Named precisely,
            // because "expected 2 values" without saying what it *does* return sends the reader
            // looking for a call site problem.
            let text = self.describe(ty);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("this call returns one value of type `{text}`, not {want}"),
                )
                .with_code(E0251)
                .with_note("a destructuring statement needs a procedure returning several values")
                .with_note("for a single result, write `x := f();`"),
            );
            return vec![PoolId::ERROR; want];
        };
        if elems.len() != want {
            let text = self.describe(ty);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "this call returns {} values, but {want} {} named",
                        elems.len(),
                        if want == 1 { "is" } else { "are" }
                    ),
                )
                .with_code(E0251)
                .with_note(format!("it returns `{text}`"))
                .with_note(
                    "the counts must match exactly; write `_` to discard a result you do not want",
                ),
            );
            return vec![PoolId::ERROR; want];
        }
        elems
    }

    fn check_local(&mut self, body: BodyId, local: jr_hir::LocalId) {
        let scope = ExprScope::Body(body);
        let hir = self.hir;
        let declaration = hir.body(body).local(local).clone();

        // A local's annotation is the one type reference that lives in
        // `Body::type_refs` rather than `FileHir::type_refs`.
        let declared = declaration
            .ty
            .map(|id| self.resolve_type(scope, id, declaration.name_span));

        let ty = match (declared, declaration.init) {
            (Some(annotation), Some(init)) => {
                self.check_expr(scope, init, Some(annotation));
                annotation
            }
            (Some(annotation), None) => annotation,
            (None, Some(init)) => {
                let inferred = self.check_expr(scope, init, None);
                self.reject_void_binding(inferred, declaration.span)
            }
            (None, None) => PoolId::ERROR,
        };

        self.locals.insert((body, local), ty);
        self.types.set_local(body, local, ty);
    }

    /// Checks an assignment.
    fn check_assign(
        &mut self,
        scope: ExprScope,
        lhs: ExprId,
        op: AssignOp,
        rhs: ExprId,
        span: Span,
    ) {
        // Type the target first: `is_place` consults the receiver's type when
        // deciding whether a field access auto-dereferences.
        let target = self.check_expr(scope, lhs, None);
        if !self.is_place(scope, lhs) {
            self.diags.push(
                Diagnostic::error(span, "cannot assign to this expression")
                    .with_code(E0220)
                    .with_note("only variables, fields, and dereferences can be assigned to")
                    .with_help("a `::` declaration is a constant; use `:=` or `: T` for something assignable"),
            );
        }

        let compound = match op {
            AssignOp::Assign => false,
            AssignOp::AddAssign
            | AssignOp::SubAssign
            | AssignOp::MulAssign
            | AssignOp::DivAssign
            | AssignOp::RemAssign
            | AssignOp::WrapAddAssign
            | AssignOp::WrapSubAssign
            | AssignOp::WrapMulAssign
            | AssignOp::BitAndAssign
            | AssignOp::BitOrAssign
            | AssignOp::BitXorAssign
            | AssignOp::ShlAssign
            | AssignOp::ShrAssign => true,
        };

        if compound && target != PoolId::ERROR && self.int_info(target).is_none() {
            let text = self.describe(target);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("compound assignment is not supported for `{text}`"),
                )
                .with_code(E0223),
            );
        }

        self.check_expr(scope, rhs, Some(target));
    }

    /// Checks the condition of an `if` or `while`.
    fn check_condition(&mut self, scope: ExprScope, cond: ExprId, keyword: &str) {
        // Checked without an expectation so that the diagnostic can be about the
        // condition rather than a generic mismatch.
        let ty = self.check_expr(scope, cond, None);
        if ty == PoolId::ERROR || ty == PoolId::BOOL {
            return;
        }
        // The condition's own span, not the statement's: pointing at three lines
        // of `if … { … }` to say "this is not a bool" is not pointing at anything.
        let span = self.expr_of(scope, cond).span();
        let text = self.describe(ty);
        self.diags.push(
            Diagnostic::error(
                span,
                format!("the condition of `{keyword}` must be `bool`, found `{text}`"),
            )
            .with_code(E0222)
            .with_note("Jairs has no implicit conversion to `bool`"),
        );
    }

    /// Checks a `return`.
    ///
    /// Whether every path through a non-`void` procedure actually returns is a
    /// control-flow question, not a typing one, and is not answered here.
    /// Checks `return a, b;` against the procedure's declared results (ADR-0052 §1).
    ///
    /// Each expression is checked against its *positional* result type, so a mismatch names the
    /// position rather than the whole tuple — which is what makes a two-result procedure returning
    /// `(bool, s64)` by mistake report the swap rather than "expected `(s64, bool)`".
    fn check_return_tuple(&mut self, scope: ExprScope, exprs: &[ExprId], span: Span) {
        let ret = self.body.as_ref().map_or(PoolId::ERROR, |body| body.ret);
        let Some(elems) = self.pool.results_elems(ret).map(<[PoolId]>::to_vec) else {
            // The procedure declares one result (or none) and this `return` gives several. Checked
            // here rather than left to `expect`, because a results aggregate has no type to unify
            // with a scalar and the generic mismatch would name an internal type.
            for expr in exprs {
                self.check_expr(scope, *expr, None);
            }
            let text = self.describe(ret);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "this `return` gives {} values, but the procedure returns `{text}`",
                        exprs.len()
                    ),
                )
                .with_code(E0251)
                .with_note("declare several results as `-> (T, U)` to return several values"),
            );
            return;
        };
        if elems.len() != exprs.len() {
            for expr in exprs {
                self.check_expr(scope, *expr, None);
            }
            let text = self.describe(ret);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "this `return` gives {} values, but `{text}` declares {}",
                        exprs.len(),
                        elems.len()
                    ),
                )
                .with_code(E0251),
            );
            return;
        }
        for (expr, want) in exprs.iter().zip(elems) {
            self.check_expr(scope, *expr, Some(want));
        }
    }

    fn check_return(&mut self, scope: ExprScope, expr: Option<ExprId>, span: Span) {
        let ret = self.body.as_ref().map_or(PoolId::ERROR, |body| body.ret);
        match expr {
            Some(value) => {
                if ret == PoolId::VOID {
                    self.check_expr(scope, value, None);
                    self.diags.push(
                        Diagnostic::error(span, "this procedure returns nothing")
                            .with_code(E0224)
                            .with_help("write `return;`, or give the procedure a `-> T`"),
                    );
                } else {
                    self.check_expr(scope, value, Some(ret));
                }
            }
            None => {
                if ret != PoolId::VOID && ret != PoolId::ERROR {
                    let text = self.describe(ret);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!(
                                "this procedure returns `{text}`, but this `return` has no value"
                            ),
                        )
                        .with_code(E0224),
                    );
                }
            }
        }
    }

    /// Rejects binding the result of a procedure that returns nothing.
    ///
    /// ADR-0016 §2. The alternative — a `void`-typed local — costs one comparison
    /// here and propagates meaningless locals into MIR, the mid-end, and both
    /// backends forever.
    pub(crate) fn reject_void_binding(&mut self, ty: PoolId, span: Span) -> PoolId {
        // **A results aggregate is not storable** (ADR-0052 §4). `q := divide(7, 2)` binds *the
        // whole aggregate*, which would make a results type spellable as a variable's type through
        // the back door — and every tuple question ADR-0052 §1 declined to answer would follow.
        // Refused here because this is the one place a binding's inferred type is judged, so the
        // same rule covers a local, a `:=` and anything else that infers.
        if self.pool.results_elems(ty).is_some() {
            let text = self.describe(ty);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("cannot bind `{text}`: a multi-result call needs one name per result"),
                )
                .with_code(E0251)
                .with_note("several results are not a value, so there is nothing to store")
                .with_help("write `a, b := f();`, naming every result, or `_` to discard one"),
            );
            return PoolId::ERROR;
        }
        if ty != PoolId::VOID {
            return ty;
        }
        self.diags.push(
            Diagnostic::error(
                span,
                "cannot bind the result of a procedure that returns nothing",
            )
            .with_code(E0217)
            .with_note("the call has no value to bind")
            .with_help("call it as a statement instead, without the `:=`"),
        );
        PoolId::ERROR
    }

    /// Checks that a `#foreign` procedure's library operand really is a library.
    ///
    /// ADR-0016 §3 exists for this check: before it, `ForeignInfo::library` was a
    /// bare symbol that nothing resolved, which left the whole FFI boundary
    /// untyped — and ADR-0006 puts a libffi bridge inside the comptime VM, so a
    /// mis-declared binding can reach the host machine during compilation.
    fn check_foreign_binding(&mut self, proc: ProcId) {
        let hir = self.hir;
        let Some(info) = hir.proc(proc).foreign.clone() else {
            return;
        };
        let Some(library) = info.library else {
            return;
        };

        let interner = self.interner;
        let name = interner.resolve(library);
        match self.lookup_value_name(library) {
            Some(entry) => {
                if entry.ty != PoolId::FOREIGN_LIBRARY && entry.ty != PoolId::ERROR {
                    let text = self.describe(entry.ty);
                    self.diags.push(
                        Diagnostic::error(
                            info.span,
                            format!("`{name}` is not a foreign library: it is `{text}`"),
                        )
                        .with_code(E0225)
                        .with_help("declare it with `#system_library`"),
                    );
                }
            }
            None => {
                self.diags.push(
                    Diagnostic::error(info.span, format!("unknown foreign library `{name}`"))
                        .with_code(E0225)
                        .with_help(format!(
                            "declare it first, e.g. `{name} :: #system_library \"c\";`"
                        )),
                );
            }
        }
    }

    /// Looks a value name up in this file, then in the imported modules.
    fn lookup_value_name(&mut self, name: Symbol) -> Option<crate::sigs::SigEntry> {
        if let Some(item) = self.hir.scope.get(name) {
            return self.entry_for_item(item);
        }
        self.imports.iter().find_map(|(_, sigs)| sigs.lookup(name))
    }
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    /// Types one expression, imposing `expected` on it where that is meaningful.
    ///
    /// `expected` is what makes ADR-0016 §1 work: an integer literal has no type
    /// of its own, so the context is the only thing that can give it one.
    pub(crate) fn check_expr(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        expected: Option<PoolId>,
    ) -> PoolId {
        let expr = self.expr_of(scope, id);
        let ty = match expr {
            // `context` is a `*Context` — passed by pointer so a callee's writes reach *its* callees
            // (ADR-0057 §2). Typed as the pointer rather than the struct, so `context.allocator` goes
            // through the same auto-deref `p.x` already does and needs no special field rule.
            Expr::Context(span) => {
                let ty = self.context_expr_type(scope, span);
                self.expect(expected, ty, span)
            }
            Expr::Literal(literal, span) => self.check_literal(&literal, expected, span),
            Expr::Name { span, res, .. } => {
                let res = self.resolve.get(scope, id).unwrap_or(res);
                // A **`#foreign` procedure taken as a value** is refused (E0256, ADR-0059 §5): its
                // type is `ContextKind::CCall` and the VM reaches it through libffi rather than a
                // `ProcRef`, so an indirect call to one is a second mechanism this wave does not
                // build. Caught here, in value position — a *direct* call routes through
                // `type_of_callee`, which does not refuse, so `write(…)` stays legal.
                if self.is_foreign_proc(&res) && !self.call_position.contains(&(scope, id)) {
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            "a `#foreign` procedure cannot be used as a value yet",
                        )
                        .with_code(E0256)
                        .with_note(
                            "an indirect call to a foreign procedure needs machinery a later wave adds",
                        ),
                    );
                    return PoolId::ERROR;
                }
                let ty = self.type_of_name(res);
                // **A type used where a runtime value is expected is refused** (E0261, ADR-0071 §3).
                // Before this, `t := Point;` type-checked cleanly and both engines exited 0, lowering
                // to `s0: type` and `v1: type = undef` — a slot of a type with *no runtime layout*
                // (`LayoutError::ComptimeOnly`) holding a placeholder that is a legitimate value. That
                // is this project's first named failure mode, invisible to the verifier and to
                // ADR-0017 §4's poison gate alike.
                //
                // Refused here rather than in lowering for ADR-0039 §3a's reason: rejecting a
                // construct is a semantic judgement, and a lowering refusal reports a
                // compiler-internal message for a program that looks well-formed.
                //
                // **Silent when the context is already poisoned**, which is `expect`'s rule and not a
                // politeness: `file_diagnostics` does not gate later phases on earlier ones, so
                // `n: nosuchtype = Point;` would otherwise report E0212 *and* E0261 for one mistake.
                // Checked here rather than left to `expect`, because this arm returns before reaching
                // it — the refusal has to know the same thing `expect` knows.
                if ty == PoolId::TYPE
                    && expected != Some(PoolId::ERROR)
                    && !self.type_is_allowed_here(scope, id)
                {
                    self.reject_type_as_value(span);
                    return PoolId::ERROR;
                }
                self.expect(expected, ty, span)
            }
            Expr::Binary { op, lhs, rhs, span } => {
                // `id` is threaded so that a resolved overload can be recorded against *this*
                // expression: `jr-mir` looks it up by the same key rather than re-resolving
                // (ADR-0048 §5).
                self.check_binary(scope, id, op, lhs, rhs, expected, span)
            }
            Expr::Unary { op, operand, span } => {
                self.check_unary(scope, op, operand, expected, span)
            }
            Expr::Call {
                callee,
                args,
                arg_names,
                span,
            } => {
                let ty = self.check_call(scope, id, callee, &args, &arg_names, span);
                self.expect(expected, ty, span)
            }
            Expr::Field {
                receiver,
                name,
                name_span,
                span,
            } => {
                let ty = self.check_field(scope, receiver, name, name_span);
                self.expect(expected, ty, span)
            }
            Expr::Index {
                base,
                index,
                index_span,
                span,
            } => {
                let ty = self.check_index(scope, base, index, index_span, span);
                self.expect(expected, ty, span)
            }
            Expr::Slice { base, span } => {
                let ty = self.check_slice(scope, base, span);
                self.expect(expected, ty, span)
            }
            // Both take `expected` **directly** rather than through `expect`: the context is
            // the input to typing them, not a constraint on the answer, so passing it on to
            // `expect` afterwards would compare the type against itself (ADR-0046 §1).
            Expr::Autocast { operand, span } => self.check_autocast(scope, operand, expected, span),
            Expr::Member {
                name,
                name_span,
                span,
            } => self.check_bare_member(name, name_span, expected, span),
            Expr::Deref(pointer, span) => {
                let ty = self.check_deref(scope, pointer, span);
                self.expect(expected, ty, span)
            }
            // `---` in an initialiser never reaches here: lowering records it as
            // a flag on the declaration. Anywhere else it has no type of its own,
            // so it takes the context's and stays quiet.
            Expr::Uninit(_) => expected.unwrap_or(PoolId::ERROR),
            Expr::Cast { ty, operand, span } => {
                let ty = self.check_cast(scope, ty, operand, span);
                self.expect(expected, ty, span)
            }
            // ADR-0016 §4: `#run e` has the type of `e` and is not folded. The
            // value arrives when the VM does.
            Expr::Run(inner, _) => self.check_expr(scope, inner, expected),
            Expr::Directive { name, arg, span } => {
                self.check_directive(name, arg.as_deref(), expected, span)
            }
            Expr::Error(_) => PoolId::ERROR,
        };
        self.types.set_expr(scope, id, ty);
        ty
    }

    /// Types `cast(T, x)` (ADR-0037 §2).
    ///
    /// The result type is always `T`, whatever the operand turns out to be — that is what
    /// makes a cast a cast rather than a checked conversion.
    ///
    /// # Why the operand is checked *against* the target
    ///
    /// Because that is what gives a literal operand the comptime fit check for free. Passing
    /// `T` as the operand's `expected` makes `cast(u8, 300)` take exactly the path
    /// `x: u8 = 300;` already takes and raise the same E0204 about the same source text
    /// (ADR-0016 §1). Nothing here re-implements the range test.
    ///
    /// A *runtime* operand takes the other branch: `expected` would demand equality, and a
    /// cast exists precisely to convert between unequal types. So a non-literal operand is
    /// typed with no expectation and then only its *kind* is checked.
    fn check_cast(
        &mut self,
        scope: ExprScope,
        ty: jr_hir::TypeRefId,
        operand: ExprId,
        span: Span,
    ) -> PoolId {
        let target = self.resolve_type(scope, ty, span);
        if target == PoolId::ERROR {
            // The target did not resolve, which E0212 already reported. Still type the
            // operand, so an error inside it is not swallowed by the outer one.
            self.check_expr(scope, operand, None);
            return PoolId::ERROR;
        }

        // A literal operand is context-typed by the target, which is where the comptime fit
        // check comes from. `is_untyped_literal` is the same predicate binary arithmetic uses.
        //
        // A *float* target and an untyped **integer** literal is the one case this shortcut
        // gets wrong: `cast(float64, 1)` would context-type `1` as a `float64`, and
        // `check_int_literal` then reports "expected `float64`, found an integer literal" —
        // a mismatch inside a cast, which is precisely the conversion the user asked for. So
        // the shortcut applies only when the literal and the target belong to the same
        // family, and a cross-family literal falls through to the ordinary path below where
        // it is typed on its own and converted (ADR-0040 §3).
        if self.is_untyped_literal(scope, operand)
            && !self.literal_crosses_families(scope, operand, target)
        {
            self.check_expr(scope, operand, Some(target));
            return target;
        }

        let from = self.check_expr(scope, operand, None);
        if from == PoolId::ERROR {
            return target;
        }

        // Four directions now: int→int, int→float, float→int, float→float (ADR-0040 §3). A
        // pointer is deliberately in none of them, so casting one is still refused rather
        // than becoming pointer arithmetic by the back door.
        // An enum casts **to** a numeric type but not from one: `cast(s64, c)` is how the
        // number is obtained (ADR-0041 §3, §6), while `cast(Colour, 1)` would manufacture a
        // value that may name no member at all — which is the hole a nominal type exists to
        // close. Asymmetric on purpose, and stated here because the symmetry is tempting.
        let from_numeric = self.is_numeric(from) || self.is_enum(from);
        let to_numeric = self.is_numeric(target);
        if !from_numeric || !to_numeric {
            let (from_text, to_text) = (self.describe(from), self.describe(target));
            self.diags.push(
                Diagnostic::error(span, format!("cannot cast `{from_text}` to `{to_text}`"))
                    .with_code(E0232)
                    .with_note(
                        "`cast` converts between numeric types — integers and floats — and \
                         from an enum to one",
                    ),
            );
        }
        target
    }

    /// Types `xx expr` (ADR-0046 §2).
    ///
    /// The conversion rule is **ADR-0037 §2's, unchanged**: `xx` is legal exactly where `cast`
    /// is legal and nowhere else. That equivalence is the design — a reader can always
    /// mechanically recover the `cast` — so this deliberately delegates rather than
    /// re-implementing a looser test.
    fn check_autocast(
        &mut self,
        scope: ExprScope,
        operand: ExprId,
        expected: Option<PoolId>,
        span: Span,
    ) -> PoolId {
        // No context means no target, and there is deliberately no fallback: a defaulted `xx`
        // would silently convert to a type nobody wrote (ADR-0046 §1).
        let Some(target) = expected else {
            // The operand is still typed, so an error inside it is not swallowed by this one.
            self.check_expr(scope, operand, None);
            self.diags.push(
                Diagnostic::error(span, "the target type of `xx` cannot be inferred here")
                    .with_code(E0242)
                    .with_note(
                        "`xx` takes its target type from the context — an annotation, a \
                         parameter, or the other side of a comparison",
                    )
                    .with_help("write the conversion explicitly, e.g. `cast(u8, x)`"),
            );
            return PoolId::ERROR;
        };
        if target == PoolId::ERROR {
            self.check_expr(scope, operand, None);
            return PoolId::ERROR;
        }

        // An untyped literal already takes the context's type, so `xx` adds nothing here and
        // would *hide* E0204's fit check. Reported before the operand is typed against the
        // target, because typing it that way is exactly what the `xx` is redundantly asking for.
        if self.is_untyped_literal(scope, operand) {
            let text = self.describe(target);
            self.diags.push(
                Diagnostic::error(span, "`xx` on a literal has no effect")
                    .with_code(E0243)
                    .with_note(format!(
                        "a literal already takes its type from the context, which here is \
                         `{text}`"
                    ))
                    .with_help("remove the `xx`"),
            );
            self.check_expr(scope, operand, Some(target));
            return target;
        }

        let from = self.check_expr(scope, operand, None);
        if from == PoolId::ERROR {
            return target;
        }
        // The same pair of predicates `check_cast` applies, so the two cannot drift: numeric to
        // numeric, or an enum to a numeric type but never the reverse (ADR-0041 §3).
        let from_ok = self.is_numeric(from) || self.is_enum(from);
        let to_ok = self.is_numeric(target);
        if !from_ok || !to_ok {
            let (from_text, to_text) = (self.describe(from), self.describe(target));
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("cannot convert `{from_text}` to `{to_text}` with `xx`"),
                )
                .with_code(E0232)
                .with_note(
                    "`xx` converts between numeric types — integers and floats — and from an \
                     enum to one, exactly as `cast` does",
                ),
            );
            return target;
        }
        target
    }

    /// Types a bare `.RED` (ADR-0046 §3, executing ADR-0041 §2's plan).
    ///
    /// Takes no `ExprScope`: a bare member names no scope, which is the whole point — the
    /// namespace comes from the context type rather than from anywhere a name could be looked
    /// up.
    fn check_bare_member(
        &mut self,
        name: Symbol,
        name_span: Span,
        expected: Option<PoolId>,
        span: Span,
    ) -> PoolId {
        let Some(target) = expected else {
            self.diags.push(
                Diagnostic::error(
                    span,
                    "the enum a bare `.` member belongs to cannot be inferred here",
                )
                .with_code(E0244)
                .with_note(
                    "a bare member takes its enum from the context — an annotation, a \
                         parameter, or the other side of a comparison",
                )
                .with_help("name the enum, e.g. `Colour.RED`"),
            );
            return PoolId::ERROR;
        };
        if target == PoolId::ERROR {
            return PoolId::ERROR;
        }

        // **A bare member against a `variant` names one of its cases** (ADR-0068 §5). The same idea
        // ADR-0046 built this for — the context supplies the namespace the source omitted — with a
        // variant's case list as the namespace instead of an enum's members. Handled before the enum
        // gate below so that a `switch v { case .i; … }` resolves rather than being told it needs an
        // enum, and the *type* is the variant, because that is what the arm is compared against.
        if let Item::VariantType { decl } = *self.pool.item(target) {
            let known = self
                .pool
                .struct_fields(decl)
                .is_some_and(|cases| cases.iter().any(|case| case.name == name));
            if known {
                return target;
            }
            let text = self.interner.resolve(name).to_owned();
            let ty_text = self.describe(target);
            self.diags.push(
                Diagnostic::error(name_span, format!("`{ty_text}` has no case `{text}`"))
                    .with_code(E0244),
            );
            return PoolId::ERROR;
        }

        // A context that is neither an enum nor a variant is a *different* problem from having none,
        // so it gets its own wording with the type named — conflating them would misdirect the reader
        // (ADR-0046 §4).
        let Item::EnumType { decl, flags } = *self.pool.item(target) else {
            let text = self.describe(target);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("expected `{text}`, and a bare `.` member needs an enum"),
                )
                .with_code(E0244)
                .with_note("a bare member is only meaningful where the context type is an enum"),
            );
            return target;
        };

        // The same lookup and the same suggestion the qualified form uses (ADR-0041 §4), so the
        // two spellings cannot disagree about which members exist.
        let known = self
            .pool
            .enum_members(decl)
            .is_some_and(|members| members.iter().any(|m| m.name == name));
        if known {
            return target;
        }
        let interner = self.interner;
        let text = interner.resolve(name).to_owned();
        self.no_such_member(decl, flags, &text, name_span);
        PoolId::ERROR
    }

    /// Whether `ty` is an enum type.
    fn is_enum(&self, ty: PoolId) -> bool {
        matches!(self.pool.item(ty), Item::EnumType { .. })
    }

    /// Whether `ty` is an integer or a float type.
    fn is_numeric(&self, ty: PoolId) -> bool {
        self.int_info(ty).is_some() || jr_pool::FloatKind::of(self.pool, ty).is_some()
    }

    /// Whether an untyped literal operand and a cast target are in different numeric families.
    ///
    /// `cast(float64, 1)` and `cast(s64, 1.5)` are both legal conversions, and both would be
    /// reported as type mismatches if the literal were context-typed by the target. This is
    /// what keeps the context-typing shortcut from swallowing the very conversion `cast`
    /// exists to express.
    fn literal_crosses_families(
        &mut self,
        scope: ExprScope,
        operand: ExprId,
        target: PoolId,
    ) -> bool {
        let target_is_float = jr_pool::FloatKind::of(self.pool, target).is_some();
        let operand_is_float = self.untyped_literal_is_float(scope, operand);
        target_is_float != operand_is_float
    }

    /// Whether an untyped literal expression is built from *float* literals.
    ///
    /// Mirrors `is_untyped_literal`'s recursion, because `-1.5` and `1.5 + 2.5` are untyped
    /// float expressions just as `-1` and `1 + 2` are untyped integer ones.
    fn untyped_literal_is_float(&self, scope: ExprScope, id: ExprId) -> bool {
        match self.expr_of(scope, id) {
            Expr::Literal(literal, _) => match literal {
                Literal::Float { .. } => true,
                Literal::Int { .. } | Literal::Str(_) | Literal::Bool(_) | Literal::Null => false,
            },
            Expr::Unary { operand, .. } => self.untyped_literal_is_float(scope, operand),
            Expr::Binary { lhs, .. } => self.untyped_literal_is_float(scope, lhs),
            Expr::Run(inner, _) => self.untyped_literal_is_float(scope, inner),
            _ => false,
        }
    }

    /// Types a literal.
    fn check_literal(&mut self, literal: &Literal, expected: Option<PoolId>, span: Span) -> PoolId {
        match literal {
            Literal::Bool(_) => self.expect(expected, PoolId::BOOL, span),
            Literal::Str(_) => self.expect(expected, PoolId::STRING, span),
            Literal::Int { value, .. } => self.check_int_literal(*value, expected, span),
            Literal::Float { .. } => self.check_float_literal(expected, span),
            Literal::Null => self.check_null_literal(expected, span),
        }
    }

    /// Types `null` against its context (ADR-0060 §1).
    ///
    /// `null` has no intrinsic type and takes its context's, exactly as an integer literal does —
    /// but unlike an integer there is **no default**: a bare `null` with no context is E0257,
    /// because there is no one pointer type to fall back to. The context must be a *pointer* type;
    /// `n: s64 = null` is the same E0257, the literal being fine and the context wrong for it.
    fn check_null_literal(&mut self, expected: Option<PoolId>, span: Span) -> PoolId {
        match expected {
            Some(want) if want == PoolId::ERROR => PoolId::ERROR,
            Some(want) if self.pointee(want).is_some() => want,
            Some(want) => {
                let text = self.describe(want);
                self.diags.push(
                    Diagnostic::error(
                        span,
                        format!("mismatched types: expected `{text}`, found `null`"),
                    )
                    .with_code(E0257)
                    .with_note("`null` is a pointer, so its context must be a pointer type"),
                );
                PoolId::ERROR
            }
            None => {
                self.diags.push(
                    Diagnostic::error(span, "`null` needs a pointer type from its context")
                        .with_code(E0257)
                        .with_note(
                            "unlike an integer literal, `null` has no default type — annotate the                              binding or call, e.g. `p: *u8 = null`",
                        ),
                );
                PoolId::ERROR
            }
        }
    }

    /// Types a float literal against its context (ADR-0040 §5).
    ///
    /// The same shape as [`Ctx::check_int_literal`] and one rule shorter: a float literal
    /// takes its context's type, defaults to `float64`, and — unlike an integer literal —
    /// **has no fit check**. There is nothing to check: ADR-0040 §1 makes an out-of-range
    /// value saturate to `inf`, so every float literal has an answer in every float type,
    /// where `x: u8 = 300;` has none and is E0204.
    fn check_float_literal(&mut self, expected: Option<PoolId>, span: Span) -> PoolId {
        let default = self.pool.intern(Item::FloatType { bits: 64 });
        match expected {
            None => default,
            Some(want) if want == PoolId::ERROR => PoolId::ERROR,
            Some(want) => {
                if jr_pool::FloatKind::of(self.pool, want).is_none() {
                    // Deliberately not "expected `s64`, found `float64`": the literal has no
                    // intrinsic type, so naming one would be inventing it. The phrasing
                    // matches `check_int_literal`'s for the mirror-image mistake.
                    let text = self.describe(want);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("mismatched types: expected `{text}`, found a float literal"),
                        )
                        .with_code(E0214),
                    );
                    return want;
                }
                want
            }
        }
    }

    /// Types an integer literal against its context (ADR-0016 §1).
    ///
    /// The literal has no intrinsic type. It takes the context's, defaults to
    /// `s64` when there is none, and must fit whichever type it ends up with.
    /// Note what this means for diagnostics: the *contextual* type is the only
    /// one worth naming, because the literal has no other.
    fn check_int_literal(&mut self, value: i128, expected: Option<PoolId>, span: Span) -> PoolId {
        let target = match expected {
            None => PoolId::S64,
            Some(want) => {
                if want == PoolId::ERROR {
                    return PoolId::ERROR;
                }
                if self.int_info(want).is_none() {
                    let text = self.describe(want);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!(
                                "mismatched types: expected `{text}`, found an integer literal"
                            ),
                        )
                        .with_code(E0214),
                    );
                    return want;
                }
                want
            }
        };

        if let Some((signed, bits)) = self.int_info(target)
            && !literal_fits(signed, bits, value)
        {
            let text = self.describe(target);
            self.diags.push(
                Diagnostic::error(span, format!("integer literal does not fit `{text}`"))
                    .with_code(E0204)
                    .with_note(format!(
                        "an integer literal takes its type from its context, which here is `{text}`"
                    ))
                    .with_note(format!(
                        "the range of `{text}` is {}",
                        int_range(signed, bits)
                    )),
            );
        }
        target
    }

    /// The type of a `context` expression, refusing it where there is no context (ADR-0057 §3).
    ///
    /// Two refusals, both E0254 and each with its own note: a `#c_call` procedure receives none by
    /// definition, and file scope has no call to have carried one.
    fn context_expr_type(&mut self, scope: ExprScope, span: Span) -> PoolId {
        match scope {
            ExprScope::TopLevel => {
                self.diags.push(
                    Diagnostic::error(span, "`context` is not available at file scope")
                        .with_code(E0254)
                        .with_note(
                            "a constant's value is computed before any call, so no context has been passed",
                        ),
                );
                PoolId::ERROR
            }
            ExprScope::Body(body) => {
                if self.body_is_c_call(body) {
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            "`context` is not available in a `#c_call` procedure",
                        )
                        .with_code(E0254)
                        .with_note("a `#c_call` procedure receives no implicit context (ADR-0001)")
                        .with_help("remove the `#c_call`, or pass what is needed explicitly"),
                    );
                    return PoolId::ERROR;
                }
                self.pool.context_pointer()
            }
        }
    }

    /// Whether the procedure owning `body` is `#c_call` (ADR-0057 §3).
    ///
    /// Reads `Proc::c_call` *or* `foreign`, because ADR-0001 makes every `#foreign` procedure
    /// implicitly `#c_call` and sema is where that implication already lives — asking only the flag
    /// would let a `#foreign` procedure mention `context`.
    fn body_is_c_call(&self, body: BodyId) -> bool {
        self.hir
            .procs
            .iter()
            .find(|proc| proc.body == Some(body))
            .is_some_and(|proc| proc.c_call || proc.foreign.is_some())
    }

    /// Types a name reference from its resolution.
    /// Whether `res` names a `#foreign` procedure (ADR-0059 §5).
    ///
    /// A same-file item only: a cross-file procedure value resolves to `Res::Imported` and is
    /// refused earlier for a different reason (ADR-0059 §1), so this need not chase imports.
    fn is_foreign_proc(&mut self, res: &Res) -> bool {
        match res {
            Res::Item(item) => self
                .hir
                .items
                .get(item.index())
                .and_then(|it| match &it.kind {
                    jr_hir::ItemKind::Const {
                        value: jr_hir::ConstValue::Proc(proc),
                    } => self.hir.procs.get(proc.index()),
                    _ => None,
                })
                .is_some_and(|proc| proc.foreign.is_some()),
            // An **imported** `#foreign` procedure, asked of its *type* rather than the other
            // file's HIR (ADR-0062 §3). `ContextKind::CCall` is exactly what `#foreign` means
            // (ADR-0001), and the type is what this file already has — chasing the declaration
            // across the module boundary would be a second answer to the same question.
            //
            // Without this arm an imported `#foreign` procedure assigned into a proc-pointer field
            // reported "expected `(s64) -> *u8`, found `(s64) -> *u8`" — identical text, because the
            // two types differ only in the invisible `ContextKind`. A message a reader cannot act on.
            Res::Imported(import, name) => {
                let ty = self
                    .entry_for_import(*import, *name)
                    .map_or(PoolId::ERROR, |entry| entry.ty);
                matches!(
                    self.pool.item(ty),
                    Item::ProcType {
                        context: jr_pool::ContextKind::CCall,
                        ..
                    }
                )
            }
            Res::Local(_) | Res::Param(_) | Res::Promoted { .. } | Res::Error => false,
        }
    }

    /// Whether a type is a legal thing to name at this expression (ADR-0071 §3).
    ///
    /// A lookup in `type_position`, which the two positions that accept one populate: a field
    /// access's receiver and a `::` constant's initialiser. See that field's documentation for why
    /// this is an allowlist.
    fn type_is_allowed_here(&self, scope: ExprScope, id: ExprId) -> bool {
        self.type_position.contains(&(scope, id))
    }

    /// Reports a type used where a runtime value was expected (E0261, ADR-0071 §3).
    ///
    /// The note names the positions that *do* accept a type rather than naming a type the reader
    /// could annotate with, because `Type` is deliberately not spellable (ADR-0071 §1) — a help line
    /// suggesting an annotation would name something the parser rejects. "Cannot be stored" without
    /// saying where it *can* go is a diagnostic a reader cannot act on.
    fn reject_type_as_value(&mut self, span: Span) {
        self.diags.push(
            Diagnostic::error(span, "a type is a compile-time value, not a runtime one")
                .with_code(E0261)
                .with_note("a type has no runtime representation, so there is nothing to store")
                .with_help(
                    "bind it with `::`, e.g. `T :: Point;`, or write it as a type annotation",
                ),
        );
    }

    fn type_of_name(&mut self, res: Res) -> PoolId {
        match res {
            Res::Local(local) => self
                .body
                .as_ref()
                .and_then(|body| self.locals.get(&(body.id, local)).copied())
                .unwrap_or(PoolId::ERROR),
            // A `ParamId` indexes the enclosing `Proc::params`, which is why the
            // body has to know which procedure it belongs to.
            Res::Param(param) => self
                .body
                .as_ref()
                .and_then(|body| body.params.get(param.index()).copied())
                .unwrap_or(PoolId::ERROR),
            Res::Item(item) => self
                .entry_for_item(item)
                .map_or(PoolId::ERROR, |entry| entry.ty),
            Res::Imported(import, name) => self
                .entry_for_import(import, name)
                .map_or(PoolId::ERROR, |entry| entry.ty),
            // A promoted name is the *base's* type, then a field of it (ADR-0050 §2). The base is
            // itself a `Res`, so this recurses: a name promoted through an embedded field is a
            // chain, and typing it one level would silently give the wrong type for the
            // transitive case ADR-0050 §4 promises.
            Res::Promoted { base, field } => {
                let base_ty = self.type_of_name((*base).clone());
                self.promoted_field_type(base_ty, field)
            }
            Res::Error => PoolId::ERROR,
        }
    }

    /// The type of `name` found through a `using`-embedded field of the struct `decl`.
    ///
    /// Searches breadth-first over the embedded bases, so a shallower embedding wins — which
    /// matters when two levels both provide a name and is the same "nearer declaration shadows"
    /// rule the direct-field check above uses.
    ///
    /// Returns `None` when nothing provides it, leaving the caller to raise E0218 with its
    /// near-name suggestion (ADR-0031 §1) rather than duplicating that diagnostic here.
    fn embedded_field_type(&mut self, decl: jr_pool::DeclId, name: Symbol) -> Option<PoolId> {
        // A cycle is impossible — a struct cannot contain itself by value, and the recursive-type
        // refusal already covers it (ADR-0050 §4) — but the depth bound is kept anyway, because a
        // malformed pool would otherwise loop forever inside the compiler rather than report.
        let mut frontier: Vec<jr_pool::DeclId> = vec![decl];
        for _ in 0..16u32 {
            let mut next = Vec::new();
            for current in frontier.drain(..) {
                let fields = match self.pool.struct_fields(current) {
                    Some(fields) => fields.to_vec(),
                    None => continue,
                };
                for field in &fields {
                    if !field.using {
                        continue;
                    }
                    let mut base_ty = field.ty;
                    while let Some(inner) = self.pointee(base_ty) {
                        base_ty = inner;
                    }
                    let Item::StructType { decl: inner_decl } = self.pool.item(base_ty) else {
                        continue;
                    };
                    let inner_decl = *inner_decl;
                    if let Some(found) = self
                        .pool
                        .struct_fields(inner_decl)
                        .and_then(|fs| fs.iter().find(|f| f.name == name).map(|f| f.ty))
                    {
                        return Some(found);
                    }
                    next.push(inner_decl);
                }
            }
            if next.is_empty() {
                return None;
            }
            frontier = next;
        }
        None
    }

    /// The type of `field` within `base_ty`, for a `using`-promoted name.
    ///
    /// Auto-derefs, so `using p: *Point` types `x` as `Point`'s `x` — matching the auto-deref
    /// `p.x` already does, because the two spellings must agree (ADR-0050 §1).
    ///
    /// Raises no diagnostic: resolution built the promotion from the struct's *own* field list, so
    /// a field that does not exist here means the two disagree, which is a compiler bug rather than
    /// a program error. `PoolId::ERROR` propagates without inventing a message that would point at
    /// the user's code for our mistake.
    fn promoted_field_type(&mut self, base_ty: PoolId, field: Symbol) -> PoolId {
        let mut ty = base_ty;
        while let Some(inner) = self.pointee(ty) {
            ty = inner;
        }
        // Only a struct, deliberately: `Item::UnionType` and `Item::VariantType` are *not* matched
        // here even though `check_field` treats all three alike, because ADR-0050 §5 refuses `using`
        // on a union — and a variant is refused for the same reason plus a stronger one: promoting a
        // case into scope would make a name read a field the tag may say is not live. Resolution has
        // already reported it; accepting one here would give a value to a promotion that was refused.
        let decl = match self.pool.item(ty) {
            Item::StructType { decl } => *decl,
            _ => return PoolId::ERROR,
        };
        self.pool
            .struct_fields(decl)
            .and_then(|fields| fields.iter().find(|f| f.name == field).map(|f| f.ty))
            .unwrap_or(PoolId::ERROR)
    }

    /// Types a binary operation.
    fn check_binary(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
        expected: Option<PoolId>,
        span: Span,
    ) -> PoolId {
        // **An overload is looked for before anything else** (ADR-0048 §4), because
        // `unify_operands` below refuses unequal operand types and a mixed-type overload —
        // `Vec2 * float64` — has to be reachable. A builtin meaning always wins, which falls out
        // of §3's orphan rule: no overload can exist for two builtin types, so `s64 + s64` cannot
        // find one.
        //
        // The whole lookup is skipped for a file that declares and imports no overload, so
        // ordinary arithmetic pays nothing for this feature existing.
        if let Some(ty) = self.check_operator_overload_call(scope, id, op, lhs, rhs, span) {
            return self.expect(expected, ty, span);
        }

        match op {
            // `<< >>` first, because they are the one binary form whose operands need **not**
            // match: the count is a separate integer, so `x << 1` must not force `1` to `x`'s
            // type nor complain when it differs (ADR-0042 §2). The result is the *left*
            // operand's type.
            BinOp::Shl | BinOp::Shr => {
                let want = expected.filter(|ty| self.int_info(*ty).is_some());
                let value = self.check_expr(scope, lhs, want);
                // The count takes `s64` when it is an untyped literal, and keeps its own type
                // otherwise. Either way it is checked independently of the value.
                let count = self.check_expr(scope, rhs, Some(PoolId::S64));
                if value != PoolId::ERROR && self.int_info(value).is_none() {
                    let text = self.describe(value);
                    // A flags enum accepts `& | ^ ~` but **not** shifts (ADR-0043 §3), and the
                    // reason is specific enough to say: `Perm.READ << 1` would produce `WRITE`
                    // by an accident of the numbering. Saying only "applies to integers" would
                    // be misleading for a type that *does* accept the other four.
                    if self.is_flags(value) {
                        self.reject_shift_on_flags(op, &text, span);
                    } else {
                        self.reject_bitwise(op, &text, span);
                    }
                    return PoolId::ERROR;
                }
                if count != PoolId::ERROR && self.int_info(count).is_none() {
                    let text = self.describe(count);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("a shift count must be an integer, not `{text}`"),
                        )
                        .with_code(E0223),
                    );
                    return PoolId::ERROR;
                }
                self.expect(expected, value, span)
            }
            // `& | ^` are integers only (ADR-0042 §5): a float's bits are a sign, an exponent
            // and a mantissa, so ANDing two of them is the AND of nothing meaningful; and an
            // enum's members are named alternatives, which is the refusal `enum_flags` will
            // lift rather than one to lift here.
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                // A *flags* enum is the one non-integer these accept, and the result keeps the
                // flags type rather than decaying to the backing integer (ADR-0043 §3) — which
                // is what makes a `Perm` stay a `Perm` through a combination.
                let want = expected.filter(|ty| self.int_info(*ty).is_some() || self.is_flags(*ty));
                let (left, right) = self.check_operands(scope, lhs, rhs, want);
                let result = self.unify_operands(left, right, span);
                if result != PoolId::ERROR
                    && self.int_info(result).is_none()
                    && !self.is_flags(result)
                {
                    let text = self.describe(result);
                    if self.is_enum(result) {
                        self.reject_bitwise_on_plain_enum(op, &text, span);
                    } else {
                        self.reject_bitwise(op, &text, span);
                    }
                    return PoolId::ERROR;
                }
                self.expect(expected, result, span)
            }
            BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Rem
            | BinOp::WrapAdd
            | BinOp::WrapSub
            | BinOp::WrapMul => {
                // **Pointer arithmetic, before the numeric path** (ADR-0064). A pointer operand must
                // not be unified with an integer one, so this is decided by typing each operand with
                // no shared expectation and asking whether either is a pointer. Only `+` and `-`
                // apply; `*`, `/`, `%` and the wrapping forms on a pointer fall through to the
                // rejection below, which is what E0223 means for them.
                //
                // **Skipped when a concrete numeric type is expected**, because then the expression
                // *is* numeric — `sum: s64 = xx tiny + 1;` must push `s64` inward so the autocast has
                // a context (E0242 otherwise), and a pointer result could never satisfy an `s64`
                // annotation anyway. So the speculative untyped probe below only runs when the result
                // could actually be a pointer: no expectation, or a pointer expectation.
                let numeric_context =
                    expected.is_some_and(|ty| self.is_numeric(ty) && self.pointee(ty).is_none());
                if matches!(op, BinOp::Add | BinOp::Sub)
                    && !numeric_context
                    && let Some(result) = self.check_pointer_arithmetic(scope, op, lhs, rhs, span)
                {
                    return self.expect(expected, result, span);
                }
                // Push a *numeric* context inward so that `g: u8 = 1 + 2;` types both
                // literals as `u8`, and `f: float32 = 1.5 + 2.5;` types both as `float32`,
                // rather than defaulting either and then complaining.
                let want = expected.filter(|ty| self.is_numeric(*ty));
                let (left, right) = self.check_operands(scope, lhs, rhs, want);
                let result = self.unify_operands(left, right, span);
                if result == PoolId::ERROR {
                    return self.expect(expected, result, span);
                }
                let is_float = jr_pool::FloatKind::of(self.pool, result).is_some();
                if self.int_info(result).is_none() && !is_float {
                    // An enum gets a message that says what to do: the members are named
                    // alternatives rather than magnitudes, so arithmetic on one has no
                    // meaning as a member — but the *number* is one cast away (ADR-0041 §6).
                    if matches!(self.pool.item(result), Item::EnumType { .. }) {
                        let text = self.describe(result);
                        self.reject_enum_operator(op, &text, span);
                        return PoolId::ERROR;
                    }
                    let text = self.describe(result);
                    self.reject_operator(op, &text, span);
                    return PoolId::ERROR;
                }
                // The operators floats do not have (ADR-0040 §7 for `%`; the wrapping forms
                // are ADR-0002's integer opt-out and have no float meaning at all, since
                // nothing wraps).
                if is_float
                    && matches!(
                        op,
                        BinOp::Rem | BinOp::WrapAdd | BinOp::WrapSub | BinOp::WrapMul
                    )
                {
                    let text = self.describe(result);
                    self.reject_float_operator(op, &text, span);
                    return PoolId::ERROR;
                }
                self.expect(expected, result, span)
            }
            BinOp::Eq | BinOp::Ne => {
                let (left, right) = self.check_operands(scope, lhs, rhs, None);
                let operand = self.unify_operands(left, right, span);
                // A view has no equality (ADR-0044 §5). Refused rather than given one of the
                // two available meanings, because "same storage" and "same contents" are both
                // plausible and the wrong reading would look like working code.
                if matches!(self.pool.item(operand), Item::ViewType { .. }) {
                    let text = self.describe(operand);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("`{}` is not supported for `{text}`", bin_op_text(op)),
                        )
                        .with_code(E0241)
                        .with_note(
                            "two views could compare as the same storage or as the same \
                             contents, and Jairs does not pick one for you",
                        )
                        .with_help("compare `.count`, or compare elements in a loop"),
                    );
                    return PoolId::BOOL;
                }
                self.expect(expected, PoolId::BOOL, span)
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let (left, right) = self.check_operands(scope, lhs, rhs, None);
                let operand = self.unify_operands(left, right, span);
                if operand != PoolId::ERROR && !self.is_numeric(operand) {
                    let text = self.describe(operand);
                    if matches!(self.pool.item(operand), Item::EnumType { .. }) {
                        self.reject_enum_operator(op, &text, span);
                    } else {
                        self.reject_operator(op, &text, span);
                    }
                }
                self.expect(expected, PoolId::BOOL, span)
            }
            BinOp::And | BinOp::Or => {
                self.check_expr(scope, lhs, Some(PoolId::BOOL));
                self.check_expr(scope, rhs, Some(PoolId::BOOL));
                self.expect(expected, PoolId::BOOL, span)
            }
        }
    }

    /// Checks `switch e { case v; … else; … }` (ADR-0067).
    ///
    /// Three jobs, in this order because each depends on the last: type the scrutinee, check every arm's
    /// value *against that type* — which is what lets a bare `.RED` resolve, since the scrutinee's type
    /// is the expected type `check_bare_member` wants (§2) — and then judge the set of arms as a whole.
    ///
    /// The set judgement is where the diagnostics live: a duplicate value or a second `else` is E0259,
    /// an enum `switch` missing members is E0258, and an `else` on one that names them all is E0260. The
    /// last is what makes the first worth having, since otherwise every `switch` could end in `else`.
    fn check_switch(
        &mut self,
        body: BodyId,
        value: ExprId,
        arms: &[jr_hir::SwitchArm],
        span: Span,
    ) {
        let scope = ExprScope::Body(body);
        let scrutinee = self.check_expr(scope, value, None);

        // Which enum this is, if any. Only an enum has a finite member set to be exhaustive over (§3);
        // an `s64` switch is legal and simply needs an `else` to be total.
        let enum_decl = match self.pool.item(scrutinee) {
            Item::EnumType { decl, flags } => Some((*decl, *flags)),
            _ => None,
        };
        // **A variant is exhaustible too**, over its *cases* rather than an enum's members (ADR-0068
        // §5). Its case names come from the struct side table, so the same set-judgement below serves
        // both — which is why this wave adds no diagnostic: E0258 and E0260 already say the right
        // things about "handles every member of".
        let variant_cases: Option<Vec<Symbol>> = match self.pool.item(scrutinee) {
            Item::VariantType { decl } => Some(
                self.pool
                    .struct_fields(*decl)
                    .unwrap_or(&[])
                    .iter()
                    .map(|case| case.name)
                    .collect(),
            ),
            _ => None,
        };

        // Members named so far, and their arms' spans, so a duplicate is reported against the *later*
        // arm — the earlier one is the one that works.
        let mut seen_members: Vec<Symbol> = Vec::new();
        let mut seen_else: Option<Span> = None;

        for arm in arms {
            match arm.value {
                None => {
                    // A second `else` can never run.
                    if seen_else.is_some() {
                        self.diags.push(
                            Diagnostic::error(arm.span, "this `switch` already has an `else`")
                                .with_code(E0259)
                                .with_note("a second catch-all can never run"),
                        );
                    }
                    seen_else = Some(arm.span);
                }
                Some(case) => {
                    // Checked against the scrutinee's type, which is what resolves a bare `.RED` and
                    // what rejects a case of the wrong type through the ordinary mismatch (E0214).
                    let want = (scrutinee != PoolId::ERROR).then_some(scrutinee);
                    self.check_expr(scope, case, want);
                    // For an enum, remember *which* member so exhaustiveness and duplicate detection
                    // have something to compare. A case whose member cannot be named — a computed
                    // value, or an error — contributes nothing rather than a wrong entry.
                    if (enum_decl.is_some() || variant_cases.is_some())
                        && let Some(name) = self.case_member_name(body, case)
                    {
                        if seen_members.contains(&name) {
                            let text = self.interner.resolve(name).to_owned();
                            self.diags.push(
                                Diagnostic::error(
                                    arm.span,
                                    format!("`{text}` is already handled by an earlier `case`"),
                                )
                                .with_code(E0259)
                                .with_note("a duplicate case can never run"),
                            );
                        } else {
                            seen_members.push(name);
                        }
                    }
                }
            }
            self.check_stmt(body, arm.body);
        }

        // A variant's set judgement, the same shape as the enum one below but over its cases
        // (ADR-0068 §5). Written out rather than folded into one generic pass because the two get their
        // names from different tables, and a shared helper taking `Vec<Symbol>` would hide which.
        if let Some(cases) = &variant_cases {
            let missing: Vec<String> = cases
                .iter()
                .filter(|name| !seen_members.contains(name))
                .map(|name| self.interner.resolve(*name).to_owned())
                .collect();
            let text = self.describe(scrutinee);
            match (missing.is_empty(), seen_else) {
                (true, Some(else_span)) => {
                    self.diags.push(
                        Diagnostic::error(
                            else_span,
                            format!("this `else` can never run: every case of `{text}` is handled"),
                        )
                        .with_code(E0260)
                        .with_help("remove the `else`"),
                    );
                }
                (false, None) => {
                    let list = missing.join("`, `");
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("this `switch` does not handle every case of `{text}`"),
                        )
                        .with_code(E0258)
                        .with_note(format!("missing: `{list}`"))
                        .with_help("add a `case` for each, or an `else` arm"),
                    );
                }
                (true, None) | (false, Some(_)) => {}
            }
        }

        // The set judgement. Only for an enum: §3 restricts exhaustiveness to the type whose member set
        // is finite and known, which is what makes the diagnostic true rather than approximate.
        if let Some((decl, flags)) = enum_decl {
            let missing: Vec<String> = self
                .pool
                .enum_members(decl)
                .unwrap_or(&[])
                .iter()
                .filter(|member| !seen_members.contains(&member.name))
                .map(|member| self.interner.resolve(member.name).to_owned())
                .collect();
            let ty = self.pool.enum_type(decl, flags);
            let text = self.describe(ty);

            match (missing.is_empty(), seen_else) {
                // Every member named *and* an `else`: the `else` cannot run (§4).
                (true, Some(else_span)) => {
                    self.diags.push(
                        Diagnostic::error(
                            else_span,
                            format!(
                                "this `else` can never run: every member of `{text}` is handled"
                            ),
                        )
                        .with_code(E0260)
                        .with_help("remove the `else`"),
                    );
                }
                // Members missing and no `else`: not exhaustive. The names *are* the fix, so they are
                // listed rather than counted.
                (false, None) => {
                    let list = missing.join("`, `");
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("this `switch` does not handle every member of `{text}`"),
                        )
                        .with_code(E0258)
                        .with_note(format!("missing: `{list}`"))
                        .with_help("add a `case` for each, or an `else` arm"),
                    );
                }
                // Exhaustive by members, or made total by an `else`.
                (true, None) | (false, Some(_)) => {}
            }
        }
    }

    /// The enum member an arm's `case` value names, if it names one syntactically (ADR-0067 §3).
    ///
    /// Reads the *expression* rather than a folded value, because exhaustiveness is about which members
    /// were written: `case .RED` and `case Colour.RED` are the two spellings, and both carry the name.
    ///
    /// `None` for anything else — a computed value, a variable, an error — which contributes nothing to
    /// the member set rather than a wrong entry. That makes a `switch` whose arms are computed
    /// *non*-exhaustive, which is the honest answer: nothing here can prove it covers the members.
    fn case_member_name(&self, body: BodyId, case: ExprId) -> Option<Symbol> {
        match self.hir.body(body).exprs.get(case.index())? {
            // `case .RED` — a bare member, resolved from the scrutinee's type.
            Expr::Member { name, .. } => Some(*name),
            // `case Colour.RED` — qualified. The receiver is the enum, which the arm's type check
            // already agreed with, so the field name is the member.
            Expr::Field { name, .. } => Some(*name),
            _ => None,
        }
    }

    /// Types `p + n`, `n + p`, `p - n` and `p - q` (ADR-0064), or returns `None` for the numeric path.
    ///
    /// Called only for `+` and `-`, and only *before* the numeric handling, because a pointer operand
    /// must not be unified with an integer one — so each operand is typed with **no shared
    /// expectation** and the shape decided from the pair. `None` means "neither operand is a pointer",
    /// which hands the operation back to the ordinary numeric path unchanged; the hot case (`s64 +
    /// s64`) takes it after two `pointee` checks that both say no.
    ///
    /// `jr-mir` re-derives which of the three forms this is from the operands' recorded types rather
    /// than a side table — the same "read the `TypeMap`, do not recompute" discipline the overload
    /// path uses (ADR-0048 §5).
    fn check_pointer_arithmetic(
        &mut self,
        scope: ExprScope,
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
        span: Span,
    ) -> Option<PoolId> {
        // Typed with no expectation, so an integer operand defaults to `s64` and a pointer keeps its
        // type — the two are never made to match.
        let left = self.check_expr(scope, lhs, None);
        let right = self.check_expr(scope, rhs, None);
        let left_ptr = self.pointee(left);
        let right_ptr = self.pointee(right);

        match (left_ptr, right_ptr) {
            // Neither is a pointer: not our case, back to the numeric path. The operands are already
            // typed, and re-typing them there with a numeric expectation is harmless — `check_expr`
            // overwrites the same `TypeMap` entry with the same or a more specific type.
            (None, None) => None,
            // Both pointers. `p + q` is meaningless; `p - q` (the pointer difference) is deferred to
            // its own wave (ADR-0064 §5), because its element-count result needs the stride, which is
            // layout `jr-mir` does not carry. Both are E0223 — the operator does not fit here.
            (Some(_), Some(_)) => {
                let text = self.describe(left);
                self.reject_operator(op, &text, span);
                Some(PoolId::ERROR)
            }
            // `p + n` or `p - n`: pointer on the left, integer on the right. Result is the pointer.
            (Some(_), None) => {
                if self.int_info(right).is_some() {
                    Some(left)
                } else {
                    let rtext = self.describe(right);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("a pointer can only be offset by an integer, not `{rtext}`"),
                        )
                        .with_code(E0223),
                    );
                    Some(PoolId::ERROR)
                }
            }
            // `n + p`: integer on the left, pointer on the right. Legal only for `+` — `n - p` is
            // "an integer minus a pointer", which has no meaning (the distance is `p - n`, the other
            // order). Result is the pointer.
            (None, Some(_)) => {
                if op == BinOp::Add && self.int_info(left).is_some() {
                    Some(right)
                } else if op == BinOp::Sub {
                    let ltext = self.describe(left);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("cannot subtract a pointer from `{ltext}`"),
                        )
                        .with_code(E0223)
                        .with_note("write `p - n` to move a pointer back, not `n - p`"),
                    );
                    Some(PoolId::ERROR)
                } else {
                    let ltext = self.describe(left);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("a pointer can only be offset by an integer, not `{ltext}`"),
                        )
                        .with_code(E0223),
                    );
                    Some(PoolId::ERROR)
                }
            }
        }
    }

    /// Types both operands of a binary operation.
    ///
    /// With no context of its own, whichever side has a type decides the other's,
    /// so that `ptr.* == 9` and `9 == ptr.*` behave the same and neither forces
    /// the literal to `s64` before the comparison is considered.
    fn check_operands(
        &mut self,
        scope: ExprScope,
        lhs: ExprId,
        rhs: ExprId,
        want: Option<PoolId>,
    ) -> (PoolId, PoolId) {
        if let Some(ty) = want {
            let left = self.check_expr(scope, lhs, Some(ty));
            let right = self.check_expr(scope, rhs, Some(ty));
            return (left, right);
        }
        if self.is_untyped_literal(scope, lhs) && !self.is_untyped_literal(scope, rhs) {
            let right = self.check_expr(scope, rhs, None);
            let left = self.check_expr(scope, lhs, Some(right));
            return (left, right);
        }
        let left = self.check_expr(scope, lhs, None);
        let right = self.check_expr(scope, rhs, Some(left));
        (left, right)
    }

    /// Requires both operands to have the same type, returning it.
    fn unify_operands(&mut self, left: PoolId, right: PoolId, span: Span) -> PoolId {
        if left == PoolId::ERROR || right == PoolId::ERROR {
            return PoolId::ERROR;
        }
        if left == right {
            return left;
        }
        let (left_text, right_text) = (self.describe(left), self.describe(right));
        self.diags.push(
            Diagnostic::error(
                span,
                format!("mismatched operand types: `{left_text}` and `{right_text}`"),
            )
            .with_code(E0214)
            .with_note("Jairs does not convert between types implicitly (ADR-0015)"),
        );
        PoolId::ERROR
    }

    /// Resolves an operator to an overload, typing the operands, or `None` for the builtin path.
    ///
    /// Returns the overload's *return* type. `None` means "no overload applies" — which is the
    /// answer for every operator in every file that declares none, so this is the hot path and it
    /// exits on a `has_operators` check before typing anything.
    ///
    /// The resolved procedure is recorded in [`CheckOutput::operator_calls`] so that `jr-mir` can
    /// lower the call **without re-running resolution**: two implementations of one rule are two
    /// chances to disagree, which is why `jr-mir` reads `TypeMap` rather than recomputing types.
    fn check_operator_overload_call(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
        span: Span,
    ) -> Option<PoolId> {
        // `&&`/`||` are control flow and never reach an overload (ADR-0048 §2); bailing here
        // rather than in the lookup keeps their short-circuit path untouched.
        if matches!(op, BinOp::And | BinOp::Or) {
            return None;
        }
        if !self.any_operators_in_scope() {
            return None;
        }

        // Typed with no expectation, because an overload's operand types are what the *lookup*
        // keys on: imposing a context would decide the answer before asking the question.
        let left = self.check_expr(scope, lhs, None);
        let right = self.check_expr(scope, rhs, None);
        if left == PoolId::ERROR || right == PoolId::ERROR {
            return None;
        }

        let (proc, file) = self.find_operator(op, left, right, span)?;
        let ret = self
            .sigs_for_file(file)
            .and_then(|sigs| sigs.proc_sig(proc).map(|sig| sig.ret))
            .unwrap_or(PoolId::ERROR);
        self.operator_calls.insert((scope, id), (file, proc));
        Some(ret)
    }

    /// Reports an operator that does not apply to its operand type.
    fn reject_operator(&mut self, op: BinOp, ty_text: &str, span: Span) {
        let text = bin_op_text(op);
        self.diags.push(
            Diagnostic::error(
                span,
                format!("operator `{text}` is not supported for `{ty_text}`"),
            )
            .with_code(E0223),
        );
    }

    /// Reports an operator floats do not have, naming why rather than saying "unsupported".
    ///
    /// `%` is undefined on floats because C's `fmod` truncates toward zero while Python's `%`
    /// follows the sign of the divisor, and they disagree on `-1.0 % 3.0` — a language
    /// decision with no forcing constraint yet (ADR-0040 §7). The wrapping operators have no
    /// float meaning at all: they are ADR-0002's opt-out from *integer* overflow, and nothing
    /// about IEEE-754 wraps.
    fn reject_float_operator(&mut self, op: BinOp, ty_text: &str, span: Span) {
        let text = bin_op_text(op);
        let note = match op {
            BinOp::Rem => {
                "the sign of a float remainder is a language decision Jairs has not taken: \
                 C's `fmod` truncates toward zero and Python's `%` follows the divisor"
            }
            _ => {
                "the wrapping operators opt out of ADR-0002's integer overflow trap, and \
                 floating-point arithmetic does not overflow — it saturates to infinity"
            }
        };
        self.diags.push(
            Diagnostic::error(
                span,
                format!("operator `{text}` is not supported for `{ty_text}`"),
            )
            .with_code(E0223)
            .with_note(note),
        );
    }

    /// Reports an operator an enum does not have, and says how to get the number (ADR-0041 §6).
    ///
    /// Ordering is refused because with auto-numbering `Colour.RED < Colour.GREEN` would be
    /// true by an accident of *declaration order* — a fact about the source file rather than
    /// about colours. Arithmetic is refused because `Colour.RED + 1` names no member.
    ///
    /// Both notes end in the same advice, because `cast(s64, c)` genuinely is the answer: it
    /// gives ordering and arithmetic on an `s64`, where they mean something.
    fn reject_enum_operator(&mut self, op: BinOp, ty_text: &str, span: Span) {
        let text = bin_op_text(op);
        let note = if is_arithmetic(op) {
            "an enum's members are named alternatives, not magnitudes, so arithmetic on one \
             has no meaning as a member"
        } else {
            "an enum's members are named alternatives, not magnitudes: with auto-numbering \
             an ordering would be true by an accident of declaration order"
        };
        self.diags.push(
            Diagnostic::error(
                span,
                format!("operator `{text}` is not supported for `{ty_text}`"),
            )
            .with_code(E0223)
            .with_note(note)
            .with_help("compare with `==`, or use `cast(s64, x)` to work with the number"),
        );
    }

    /// Reports a bitwise operator on a type that has no bits to work on (ADR-0042 §5).
    ///
    /// The note distinguishes the two reachable cases, because the *advice* differs: a float
    /// has bits but not meaningful ones, while an enum's members genuinely are combinable —
    /// which is what `enum_flags` will be for.
    fn reject_bitwise(&mut self, op: BinOp, ty_text: &str, span: Span) {
        let text = bin_op_text(op);
        let mut diag = Diagnostic::error(
            span,
            format!("operator `{text}` is not supported for `{ty_text}`"),
        )
        .with_code(E0223)
        .with_note("bitwise operators apply to integers and to `enum_flags`");
        if jr_pool::FloatKind::from_name(ty_text).is_some() {
            diag = diag.with_note(
                "a float's bits are a sign, an exponent and a mantissa, so combining two of \
                 them bitwise is not the combination of anything meaningful",
            );
        }
        self.diags.push(diag);
    }

    /// Reports a bitwise operator on a **plain** enum, naming `enum_flags` (ADR-0043 §4).
    ///
    /// A separate message from [`Ctx::reject_bitwise`] because the answer differs: a plain
    /// enum's members are named alternatives and combining them is meaningless, but the
    /// programmer who tried almost certainly wanted a set — and cannot find `enum_flags` if
    /// nothing mentions it.
    fn reject_bitwise_on_plain_enum(&mut self, op: BinOp, ty_text: &str, span: Span) {
        let text = bin_op_text(op);
        self.diags.push(
            Diagnostic::error(
                span,
                format!("operator `{text}` is not supported for `{ty_text}`"),
            )
            .with_code(E0223)
            .with_note(
                "a plain `enum`'s members are named alternatives, so combining two of them \
                 bitwise names no member",
            )
            .with_help("declare it `enum_flags` if its members are meant to combine"),
        );
    }

    /// Reports a shift on a flags enum, which accepts every other bitwise operator.
    ///
    /// The distinction matters because "bitwise operators apply to integers and to
    /// `enum_flags`" is *true* and yet would leave the reader confused: they used a bitwise
    /// operator on an `enum_flags` and were refused.
    fn reject_shift_on_flags(&mut self, op: BinOp, ty_text: &str, span: Span) {
        let text = bin_op_text(op);
        self.diags.push(
            Diagnostic::error(
                span,
                format!("operator `{text}` is not supported for `{ty_text}`"),
            )
            .with_code(E0223)
            .with_note(
                "shifting a flag set would produce another member by an accident of the \
                 numbering; `& | ^ ~` are the operators a flag set has",
            )
            .with_help("use `cast(s64, x)` if the numeric value is what you want to shift"),
        );
    }

    /// Whether `ty` is an `enum_flags` type.
    fn is_flags(&self, ty: PoolId) -> bool {
        matches!(self.pool.item(ty), Item::EnumType { flags: true, .. })
    }

    /// Types a unary operation.
    fn check_unary(
        &mut self,
        scope: ExprScope,
        op: UnOp,
        operand: ExprId,
        expected: Option<PoolId>,
        span: Span,
    ) -> PoolId {
        match op {
            // `~` is integers only, and refusing it on a `bool` is the point: `!` is the
            // boolean negation, and a `bool`'s complement is 254 — not a `bool` at all
            // (ADR-0042 §4).
            UnOp::BitNot => {
                // A flags enum too (ADR-0043 §3): `~Perm.READ` is the complement of a set and
                // keeps the flags type.
                let want = expected.filter(|ty| self.int_info(*ty).is_some() || self.is_flags(*ty));
                let ty = self.check_expr(scope, operand, want);
                if ty != PoolId::ERROR && self.int_info(ty).is_none() && !self.is_flags(ty) {
                    let text = self.describe(ty);
                    let mut diag = Diagnostic::error(
                        span,
                        format!("operator `~` is not supported for `{text}`"),
                    )
                    .with_code(E0223)
                    .with_note("`~` is a bitwise complement and applies to integers");
                    if ty == PoolId::BOOL {
                        diag = diag.with_help("use `!` to negate a `bool`");
                    }
                    self.diags.push(diag);
                    return PoolId::ERROR;
                }
                self.expect(expected, ty, span)
            }
            UnOp::Neg => {
                let want = expected.filter(|ty| self.is_numeric(*ty));
                let ty = self.check_expr(scope, operand, want);
                // Negation is total on floats — it flips the sign bit — and traps on the most
                // negative integer (ADR-0002). Both are accepted here; the difference lives in
                // the arithmetic, not the type check.
                if ty != PoolId::ERROR && !self.is_numeric(ty) {
                    let text = self.describe(ty);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("operator `-` is not supported for `{text}`"),
                        )
                        .with_code(E0223),
                    );
                    return PoolId::ERROR;
                }
                self.expect(expected, ty, span)
            }
            UnOp::Not => {
                self.check_expr(scope, operand, Some(PoolId::BOOL));
                self.expect(expected, PoolId::BOOL, span)
            }
            UnOp::AddrOf => {
                if !self.is_place(scope, operand) {
                    self.diags.push(
                        Diagnostic::error(span, "cannot take the address of this expression")
                            .with_code(E0221)
                            .with_note("only variables, fields, and dereferences have an address"),
                    );
                }
                // `f: *s64 = *a;` pushes `s64` into the operand rather than
                // letting it default.
                let want = expected.and_then(|ty| self.pointee(ty));
                let ty = self.check_expr(scope, operand, want);
                if ty == PoolId::ERROR {
                    return PoolId::ERROR;
                }
                let pointer = self.pool.pointer_to(ty);
                self.expect(expected, pointer, span)
            }
        }
    }

    /// Types a call.
    fn check_call(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        callee: ExprId,
        args: &[ExprId],
        arg_names: &[Option<Symbol>],
        span: Span,
    ) -> PoolId {
        // **`type_info(T)` is an intrinsic and is handled before anything else** (ADR-0075 §2),
        // because its argument is a *type* and every path below types an argument as a runtime value —
        // which is exactly the E0261 refusal. Intercepting here rather than teaching the general
        // argument check about a special case keeps that refusal intact everywhere else.
        //
        // A call rather than a directive (`#type_info`) because a directive cannot be passed as a value
        // or composed, and ADR-0071 already makes a type an argument-position value.
        if self.is_type_info_call(scope, callee) {
            return self.check_type_info(scope, id, callee, args, span);
        }
        // The callee is in **call position**, where a `#foreign` procedure is a legal thing to
        // name — it is only illegal to take one as a *value* (E0256, ADR-0059 §5). This id is
        // recorded so `check_expr`'s `Name` arm skips the E0256 refusal for it, while still typing
        // and `set_expr`-recording the callee exactly as every other expression. Skipping
        // `check_expr` entirely (an earlier attempt) left the callee's type unrecorded, which
        // surfaced as MIR's "an expression was never typed" on `write(…)`.
        self.call_position.insert((scope, callee));
        let callee_ty = self.check_expr(scope, callee, None);
        // Copy the signature out before touching `self` again: the pool borrow
        // and the diagnostic sink cannot both be live.
        let signature = match self.pool.item(callee_ty) {
            Item::ProcType { params, ret, .. } => Some((params.clone(), *ret)),
            _ => None,
        };

        let Some((params, ret)) = signature else {
            if callee_ty != PoolId::ERROR {
                let text = self.describe(callee_ty);
                self.diags.push(
                    Diagnostic::error(span, format!("expected a procedure, found `{text}`"))
                        .with_code(E0215),
                );
            }
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            return PoolId::ERROR;
        };

        // **Named arguments and defaults are resolved into positional order here** (ADR-0053 §1),
        // before the arity check and the type check, so both work on one shape. The result is
        // recorded so `jr-mir` reads it instead of the source order — one pass decides argument
        // order, which is the same split ADR-0048 §5 made for overload resolution.
        let named = arg_names.iter().any(Option::is_some);
        let has_defaults = self
            .callee_sig(scope, callee)
            .is_some_and(|sig| sig.defaults.iter().any(Option::is_some));
        if named || has_defaults {
            if let Some(filled) = self.fill_arguments(scope, callee, args, arg_names, span) {
                for (index, slot) in filled.iter().enumerate() {
                    let want = params.get(index).copied();
                    if let ArgSlot::Given(arg) = slot {
                        self.check_expr(scope, *arg, want);
                    }
                }
                self.filled_calls.insert((scope, id), filled);
                return ret;
            }
            // `fill_arguments` reported the problem; type what was written so a second error in an
            // argument is still found, then poison.
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            return PoolId::ERROR;
        }

        if args.len() != params.len() {
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "this procedure takes {} argument{}, but {} {} supplied",
                        params.len(),
                        if params.len() == 1 { "" } else { "s" },
                        args.len(),
                        if args.len() == 1 { "was" } else { "were" }
                    ),
                )
                .with_code(E0216),
            );
        }

        for (index, arg) in args.iter().enumerate() {
            let want = params.get(index).copied();
            self.check_expr(scope, *arg, want);
        }

        ret
    }

    /// Whether this callee is the `type_info` intrinsic (ADR-0075 §2).
    ///
    /// Recognised **by name and only when the name resolves to nothing**, which is the whole of the
    /// test: `type_info` is not declared anywhere, so a program that declares its own `type_info` gets
    /// its own — the resolution succeeds and this returns false. Reserving the name outright would break
    /// a program that already used it, for no gain.
    fn is_type_info_call(&mut self, scope: ExprScope, callee: ExprId) -> bool {
        let Expr::Name { name, res, .. } = self.expr_of(scope, callee) else {
            return false;
        };
        if self.interner.resolve(name) != "type_info" {
            return false;
        }
        matches!(self.resolve.get(scope, callee).unwrap_or(res), Res::Error)
    }

    /// Types `type_info(T)` and returns `*Type_Info` (ADR-0075 §2).
    ///
    /// The argument is a **type**, so it is marked as a type position before being checked — otherwise
    /// the `Name` arm's E0261 would refuse it for being a type used as a runtime value, which is the
    /// correct refusal in every position but this one.
    fn check_type_info(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        callee: ExprId,
        args: &[ExprId],
        span: Span,
    ) -> PoolId {
        // The callee names no procedure, so it is typed as `void` rather than left unrecorded: MIR
        // reports "an expression was never typed" for a hole, and the callee is never lowered because
        // the call folds to a constant.
        self.types.set_expr(scope, callee, PoolId::VOID);

        if args.len() != 1 {
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "`type_info` takes 1 argument, but {} {} supplied",
                        args.len(),
                        if args.len() == 1 { "was" } else { "were" }
                    ),
                )
                .with_code(E0216),
            );
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            return PoolId::ERROR;
        }

        let arg = args[0];
        // A type is legal *here*, and nowhere new: the allowlist gains one entry rather than E0261
        // gaining an exception (ADR-0071 §3's asymmetry argument).
        self.type_position.insert((scope, arg));

        // **The described type is resolved before the argument is typed as an expression**, because a
        // *builtin* name is not an expression at all: `s64` resolves to no declaration, so
        // `check_expr` yields `ERROR` and bailing on that refused every `type_info(s64)`. Asking what
        // type the name denotes first is what makes a builtin and a declared type take one path.
        let described = self.described_type(scope, arg);

        // Typed anyway, so the argument is recorded in the `TypeMap` — MIR reports "an expression was
        // never typed" for a hole, even one it never lowers. `PoolId::TYPE` is what a name denoting a
        // type has, which is exactly what this is.
        self.types.set_expr(scope, arg, PoolId::TYPE);

        // What it was asked about. A `type`-typed name carries the described type in its
        // `SigEntry::type_value`, which is what `resolve_type_name` reads and what ADR-0071 §1 made a
        // type value out of.
        let Some(described) = described else {
            self.diags.push(
                Diagnostic::error(span, "`type_info` needs a type")
                    .with_code(E0261)
                    .with_note("its argument is the type to describe, e.g. `type_info(Point)`"),
            );
            return PoolId::ERROR;
        };

        // **A type with no runtime layout has no `size` to report** (E0266, ADR-0075 §4). Refused
        // rather than reported as zero, for `type-errors/063`'s reason: a plausible wrong number cannot
        // be told from a real one downstream.
        if let Err(error) = jr_pool::layout_of(self.pool, jr_pool::TargetLayout::LP64, described) {
            let text = self.describe(described);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("`type_info` cannot describe `{text}`, which has no runtime layout"),
                )
                .with_code(E0266)
                .with_note(format!("the layout is unavailable because {error}"))
                .with_help(
                    "a `Type_Info` reports a size and an alignment, and this type has neither",
                ),
            );
            return PoolId::ERROR;
        }

        // The described type is recorded for lowering, which builds the constant.
        self.type_info_calls.insert((scope, id), described);

        // **By value, not by pointer** (ADR-0075 §2): the folded value is an `Item::AggregateValue`,
        // which is a constant and has no address, so a `*Type_Info` would need a pointee to live
        // somewhere. The MIR verifier caught the pointer version as `deref of a non-pointer`.
        match self.type_info_struct(span) {
            Some(info) => info,
            None => PoolId::ERROR,
        }
    }

    /// The type a `type`-valued expression names, if it names one.
    ///
    /// A **builtin** is matched by text, because `s64` is an ordinary identifier that resolves to no
    /// declaration at all (`docs/spec/01-lexical.md` keeps the builtin names out of the lexer), so
    /// `type_info(s64)` would otherwise be an unresolved name. Only a `Res::Error` takes that path: a
    /// name that *did* resolve — to a local, a parameter or a value constant — is not a type, and trying
    /// the builtin table for it would answer the wrong question.
    ///
    /// Matched here rather than by calling `resolve_type_name`, which reports **E0212** as a side effect:
    /// `type_info(x)` for a local `x` then said "unknown type name `x`", which is wrong twice over — `x`
    /// is perfectly well known, and the objection is that it is a value rather than a type. Returning
    /// `None` lets the caller raise E0261, which says exactly that.
    fn described_type(&mut self, scope: ExprScope, arg: ExprId) -> Option<PoolId> {
        let Expr::Name { name, res, .. } = self.expr_of(scope, arg) else {
            return None;
        };
        let res = self.resolve.get(scope, arg).unwrap_or(res);
        match res {
            Res::Item(item) => self.entry_for_item(item).and_then(|e| e.type_value),
            Res::Imported(import, name) => self
                .entry_for_import(import, name)
                .and_then(|e| e.type_value),
            // Unresolved: possibly a builtin, which has no declaration to have resolved to.
            Res::Error => self.builtin_type_named(name),
            // Resolved to something that is not a type. `None` here becomes E0261.
            Res::Local(_) | Res::Param(_) | Res::Promoted { .. } => None,
        }
    }

    /// The builtin type a name spells, without reporting anything if it spells none.
    ///
    /// The three lists are the ones `resolve_type_name` consults — `bool`/`string`, `IntKind::NAMES` and
    /// `FloatKind::NAMES` — read here directly so that a miss is silent. `s64` and `u8` keep their
    /// pre-interned ids for the reason `resolve_type_name` gives: the well-known prefix's indices are
    /// pinned by a test and `PTR_U8` depends on them.
    fn builtin_type_named(&mut self, name: Symbol) -> Option<PoolId> {
        let text = self.interner.resolve(name);
        match text {
            "bool" => return Some(PoolId::BOOL),
            "string" => return Some(PoolId::STRING),
            "void" => return Some(PoolId::VOID),
            _ => {}
        }
        if let Some(kind) = jr_pool::FloatKind::from_name(text) {
            return Some(self.pool.intern(Item::FloatType { bits: kind.bits }));
        }
        let kind = jr_pool::IntKind::from_name(text)?;
        Some(match (kind.signed, kind.bits) {
            (true, 64) => PoolId::S64,
            (false, 8) => PoolId::U8,
            (signed, bits) => self.pool.intern(Item::IntType { signed, bits }),
        })
    }

    /// The `Type_Info` struct type, looked up in the imported modules and **validated** (ADR-0075 §2).
    ///
    /// ADR-0075 §2 declares `Type_Info` in `modules/Basic` so that it is *spellable* — no
    /// compiler-declared type is — and the price is this dependency on a declaration the compiler does
    /// not own. The validation is what keeps the price honest: field names, types and order are checked,
    /// so an edit to `Basic` produces E0265 naming the mismatch rather than a read of whatever now sits
    /// at the old offset. A wrong offset would be a silent wrong value, which is the failure mode
    /// ADR-0017 §4 says must refuse instead.
    fn type_info_struct(&mut self, span: Span) -> Option<PoolId> {
        // **Silent when no imported signatures were supplied at all**, which is `expect`'s rule about a
        // poisoned context rather than a politeness. `Type_Info` lives in `Basic`, so a checker run
        // *without* module resolution cannot possibly find it — and reporting E0265 there would be
        // inventing a library error out of a missing input. `jr-sema`'s own corpus test runs exactly that
        // way on purpose ("sema must stay silent about them rather than inventing type errors on
        // poison"), and it is what caught this.
        //
        // Nothing is lost: a real program reaches this with `Basic` loaded, and a `type_info` in a file
        // that imports nothing is refused anyway — the call yields `PoolId::ERROR` and MIR never sees a
        // value, so `scan` refuses the body rather than lowering a placeholder.
        if self.imports.is_empty() {
            return None;
        }
        let name = self.interner.intern("Type_Info");
        let entry = self
            .imports
            .iter()
            .find_map(|(_, sigs)| sigs.lookup(name))
            .or_else(|| self.sigs.lookup(name));
        let Some(ty) = entry.and_then(|e| e.type_value) else {
            self.report_type_info_shape(span, "it is not declared, or is not a type");
            return None;
        };
        let Item::StructType { decl } = *self.pool.item(ty) else {
            self.report_type_info_shape(span, "it is not a struct");
            return None;
        };
        let Some(fields) = self.pool.struct_fields(decl).map(<[_]>::to_vec) else {
            self.report_type_info_shape(span, "its fields are not recorded");
            return None;
        };
        if fields.len() != TYPE_INFO_FIELDS.len() {
            self.report_type_info_shape(
                span,
                &format!(
                    "it has {} field(s), expected {}",
                    fields.len(),
                    TYPE_INFO_FIELDS.len()
                ),
            );
            return None;
        }
        for (field, (want_name, want_ty)) in fields.iter().zip(TYPE_INFO_FIELDS) {
            let got_name = self.interner.resolve(field.name).to_owned();
            if got_name != *want_name {
                self.report_type_info_shape(
                    span,
                    &format!("its field is named `{got_name}`, expected `{want_name}`"),
                );
                return None;
            }
            // `kind` is an enum declared beside it, so its type is checked by *shape* rather than
            // against a fixed id: an enum's `PoolId` depends on its declaration site.
            let ok = match *want_ty {
                TypeInfoField::Enum => matches!(*self.pool.item(field.ty), Item::EnumType { .. }),
                TypeInfoField::Exact(id) => field.ty == id,
            };
            if !ok {
                let text = self.describe(field.ty);
                self.report_type_info_shape(
                    span,
                    &format!(
                        "its field `{want_name}` has type `{text}`, which is not what is expected"
                    ),
                );
                return None;
            }
        }
        Some(ty)
    }

    /// Reports E0265: `Type_Info` is missing or wrongly shaped (ADR-0075 §2).
    fn report_type_info_shape(&mut self, span: Span, why: &str) {
        self.diags.push(
            Diagnostic::error(
                span,
                format!("the standard library's `Type_Info` is not usable: {why}"),
            )
            .with_code(E0265)
            .with_note("`type_info` returns a `*Type_Info`, which is declared in `modules/Basic`")
            .with_help("import \"Basic\", and keep its `Type_Info` in step with the compiler"),
        );
    }

    /// The callee's per-procedure signature, when the callee names a procedure.
    ///
    /// `Item::ProcType` carries only *types*, so parameter names and defaults have to come from
    /// `ProcSig` — which is keyed by `ProcId` and therefore needs the callee resolved to one
    /// (ADR-0053 §1).
    fn callee_sig(&mut self, scope: ExprScope, callee: ExprId) -> Option<ProcSig> {
        let Expr::Name { res, .. } = self.expr_of(scope, callee) else {
            return None;
        };
        let res = self.resolve.get(scope, callee).unwrap_or(res);
        let item = match res {
            Res::Item(item) => item,
            // A call to an imported procedure resolves through the other file's signatures, which
            // this crate does not hold — so a named argument on a cross-file call is not supported
            // and says so rather than silently ignoring the name.
            Res::Imported(_, _)
            | Res::Local(_)
            | Res::Param(_)
            | Res::Promoted { .. }
            | Res::Error => return None,
        };
        let jr_hir::ItemKind::Const {
            value: jr_hir::ConstValue::Proc(proc),
        } = self.hir.item(item).kind.clone()
        else {
            return None;
        };
        self.sigs.proc_sig(proc).cloned()
    }

    /// Resolves an argument list into one slot per parameter (ADR-0053 §1, §3).
    ///
    /// Returns `None` having reported a diagnostic when any of §3's four rules is broken. The four
    /// are checked in source order so the *first* mistake is the one reported, rather than a cascade.
    fn fill_arguments(
        &mut self,
        scope: ExprScope,
        callee: ExprId,
        args: &[ExprId],
        arg_names: &[Option<Symbol>],
        span: Span,
    ) -> Option<Vec<ArgSlot>> {
        let sig = self.callee_sig(scope, callee)?;
        let mut slots: Vec<Option<ArgSlot>> = vec![None; sig.params.len()];
        let mut seen_named = false;

        for (index, arg) in args.iter().enumerate() {
            match arg_names.get(index).copied().flatten() {
                Some(name) => {
                    seen_named = true;
                    let Some(position) = sig.names.iter().position(|n| *n == name) else {
                        let text = self.interner.resolve(name);
                        let candidates: Vec<&str> = sig
                            .names
                            .iter()
                            .map(|n| self.interner.resolve(*n))
                            .collect();
                        let mut diag = Diagnostic::error(
                            span,
                            format!("this procedure has no parameter named `{text}`"),
                        )
                        .with_code(E0252);
                        // The same near-name machinery E0212 and E0218 use (ADR-0031 §1) — a
                        // misspelled parameter is exactly the case it exists for.
                        if let Some(suggestion) =
                            crate::suggest::nearest(text, candidates.iter().copied())
                        {
                            diag = diag.with_help(format!("did you mean `{suggestion}`?"));
                        }
                        self.diags.push(diag);
                        return None;
                    };
                    if slots[position].is_some() {
                        let text = self.interner.resolve(name);
                        self.diags.push(
                            Diagnostic::error(span, format!("`{text}` is supplied more than once"))
                                .with_code(E0252)
                                .with_note(
                                    "a parameter already filled positionally cannot be named",
                                ),
                        );
                        return None;
                    }
                    slots[position] = Some(ArgSlot::Given(*arg));
                }
                None => {
                    if seen_named {
                        self.diags.push(
                            Diagnostic::error(
                                span,
                                "a positional argument cannot follow a named one",
                            )
                            .with_code(E0252)
                            .with_note(
                                "otherwise a positional argument's meaning would depend on which names came before it",
                            ),
                        );
                        return None;
                    }
                    if index >= slots.len() {
                        self.diags.push(
                            Diagnostic::error(
                                span,
                                format!(
                                    "this procedure takes {} argument{}, but more were supplied",
                                    sig.params.len(),
                                    if sig.params.len() == 1 { "" } else { "s" }
                                ),
                            )
                            .with_code(E0252),
                        );
                        return None;
                    }
                    slots[index] = Some(ArgSlot::Given(*arg));
                }
            }
        }

        // Anything still unfilled must have a default, or the call is incomplete.
        let mut filled = Vec::with_capacity(slots.len());
        for (position, slot) in slots.into_iter().enumerate() {
            match slot {
                Some(slot) => filled.push(slot),
                None => match sig.defaults.get(position).copied().flatten() {
                    Some(value) => filled.push(ArgSlot::Default(value)),
                    None => {
                        let text = self.interner.resolve(sig.names[position]);
                        self.diags.push(
                            Diagnostic::error(
                                span,
                                format!("`{text}` has no argument and no default value"),
                            )
                            .with_code(E0252),
                        );
                        return None;
                    }
                },
            }
        }
        Some(filled)
    }

    /// Types a field access, looking through pointers.
    fn check_field(
        &mut self,
        scope: ExprScope,
        receiver: ExprId,
        name: Symbol,
        name_span: Span,
    ) -> PoolId {
        // The receiver is a position where a **type** is legal: `Colour.RED` names the enum type used
        // as a value (ADR-0041 §1). Recorded before typing it, the way `check_call` records its callee,
        // so that `check_expr`'s `Name` arm skips E0261 here while still typing and recording the
        // receiver exactly as any other expression (ADR-0071 §3).
        self.type_position.insert((scope, receiver));
        let mut ty = self.check_expr(scope, receiver, None);
        while let Some(inner) = self.pointee(ty) {
            ty = inner;
        }
        if ty == PoolId::ERROR {
            return PoolId::ERROR;
        }

        let interner = self.interner;
        let field = interner.resolve(name);

        let receiver_kind = match self.pool.item(ty) {
            Item::StringType => ReceiverKind::Str,
            // A union's field access *is* a struct's: same field list, same side table, same
            // diagnostics. Only the offsets differ, and those are `jr-pool`'s (ADR-0045 §5). A
            // variant's cases are a field list too (ADR-0068 §1), so it joins them — what differs is
            // the tag check MIR emits on the *read*, which is not a typing question.
            Item::StructType { decl } | Item::UnionType { decl } | Item::VariantType { decl } => {
                ReceiverKind::Struct(*decl)
            }
            // The context's fields are the compiler's, not a side table's — there is no `DeclId` to
            // key one on (ADR-0057 §1), so this is its own receiver kind rather than a `Struct`.
            Item::ContextType => ReceiverKind::Context,
            Item::ArrayType { .. } => ReceiverKind::Array,
            Item::ViewType { .. } => ReceiverKind::View,
            // `Colour.RED`: the *receiver* is the enum type used as a value, so its type is
            // `type` and the enum it denotes has to come from the receiver expression rather
            // than from `ty` (ADR-0041 §1).
            Item::TypeType => match self.denoted_enum(scope, receiver) {
                Some((decl, flags)) => ReceiverKind::Enum(decl, flags),
                None => ReceiverKind::Fieldless,
            },
            _ => ReceiverKind::Fieldless,
        };

        match receiver_kind {
            // ADR-0004 fixes `string`'s layout as `{data: *u8, count: s64}` and
            // makes both directly accessible. They are pseudo-fields rather than
            // real ones because `string` is deliberately *not* the struct of that
            // shape (ADR-0015 §2).
            ReceiverKind::Str => match field {
                "data" => PoolId::PTR_U8,
                "count" => PoolId::S64,
                _ => {
                    self.no_such_field(ty, field, name_span);
                    PoolId::ERROR
                }
            },
            // `[N]T` has exactly one pseudo-field, `.count`, and it is the length from the
            // *type* — nothing is loaded (ADR-0039 §5). There is deliberately no `.data`:
            // it would hand out an unbounded `*T` one wave after adding the bounds check.
            ReceiverKind::Array => match field {
                "count" => PoolId::S64,
                _ => {
                    self.no_such_field(ty, field, name_span);
                    PoolId::ERROR
                }
            },
            // A view answers `.count` with the same type an array does, and by a different
            // route: this one is a *load* of the second word rather than a constant from the
            // type (ADR-0044 §4). `.data` is absent for the array's reason — it would hand
            // out an unbounded `*T` with no pointer arithmetic to use it with.
            ReceiverKind::View => match field {
                "count" => PoolId::S64,
                _ => {
                    self.no_such_field(ty, field, name_span);
                    PoolId::ERROR
                }
            },
            // A member is a *value of the enum type*, not of the backing integer: that is
            // what makes `Colour` and `s64` different types (ADR-0041 §3). The member's
            // number is folded in at MIR; here only its existence and its type matter.
            ReceiverKind::Enum(decl, flags) => {
                let known = self
                    .pool
                    .enum_members(decl)
                    .is_some_and(|members| members.iter().any(|m| m.name == name));
                if known {
                    self.pool.enum_type(decl, flags)
                } else {
                    self.no_such_member(decl, flags, field, name_span);
                    PoolId::ERROR
                }
            }
            ReceiverKind::Struct(decl) => {
                // A direct field first, then — failing that — a field of any `using`-embedded
                // base (ADR-0050 §4). Direct wins, so a struct that declares `x` *and* embeds
                // something declaring `x` means its own, which matches the rule everywhere else
                // in the language: the nearer declaration shadows.
                let found = self
                    .pool
                    .struct_fields(decl)
                    .and_then(|fields| fields.iter().find(|f| f.name == name).map(|f| f.ty))
                    .or_else(|| self.embedded_field_type(decl, name));
                match found {
                    Some(field_ty) => field_ty,
                    None => {
                        self.no_such_field(ty, field, name_span);
                        PoolId::ERROR
                    }
                }
            }
            ReceiverKind::Context => match jr_pool::Pool::context_field(field) {
                Some(index) => jr_pool::Pool::context_field_type(index).unwrap_or(PoolId::ERROR),
                None => {
                    let candidates = jr_pool::CONTEXT_FIELD_NAMES.iter().copied();
                    let mut diag =
                        Diagnostic::error(name_span, format!("the context has no field `{field}`"))
                            .with_code(E0218);
                    // The same near-name machinery every other field lookup uses (ADR-0031 §1).
                    if let Some(suggestion) = crate::suggest::nearest(field, candidates) {
                        diag = diag.with_help(format!("did you mean `{suggestion}`?"));
                    }
                    self.diags.push(diag);
                    PoolId::ERROR
                }
            },
            ReceiverKind::Fieldless => {
                let text = self.describe(ty);
                self.diags.push(
                    Diagnostic::error(name_span, format!("type `{text}` has no fields"))
                        .with_code(E0218),
                );
                PoolId::ERROR
            }
        }
    }

    /// Types `base[index]` (ADR-0039 §5).
    ///
    /// Three separate refusals, each pointing at the thing that is wrong: the base if it
    /// is not an array, the index if it is not an integer, and the index again if it is a
    /// literal that cannot be in range. Reporting all three against the whole `a[i]` span
    /// would make the reader look at the wrong end of the expression.
    fn check_index(
        &mut self,
        scope: ExprScope,
        base: ExprId,
        index: ExprId,
        index_span: Span,
        span: Span,
    ) -> PoolId {
        let mut base_ty = self.check_expr(scope, base, None);
        // Auto-deref, exactly as field access does: `p: *[4]u8` indexes through the
        // pointer. Same loop, so the two cannot disagree about how many levels.
        while let Some(inner) = self.pointee(base_ty) {
            base_ty = inner;
        }

        // The index is checked whatever the base turned out to be, so that `notarray[bad]`
        // reports both problems rather than hiding the second behind the first.
        //
        // `Some(PoolId::S64)` is the context an untyped literal takes (ADR-0016 §1), which
        // makes `buf[0]` an `s64` index rather than an unconstrained one.
        let index_ty = self.check_expr(scope, index, Some(PoolId::S64));

        // A view indexes like an array and has no compile-time length, so `len` is `None`
        // and the literal-index check below is skipped. That is not a weaker check: a view's
        // length is unknown at compile time by definition, and `Statement::BoundsCheck` still
        // guards every access at run time (ADR-0044 §4).
        let Some((elem, len)) = self.indexable_parts(base_ty) else {
            // Poison propagates silently: the base's own error was already reported.
            if base_ty != PoolId::ERROR {
                let text = self.describe(base_ty);
                self.diags.push(
                    Diagnostic::error(span, format!("cannot index a value of type `{text}`"))
                        .with_code(E0234)
                        .with_note("only a fixed-size array `[N]T` and a view `[]T` can be indexed")
                        .with_help("dynamic arrays `[..]T` arrive in a later wave"),
                );
            }
            return PoolId::ERROR;
        };

        // An index must be an integer. `bool` and `string` are the reachable mistakes; a
        // pointer is deliberately included, because allowing one would be the first half of
        // pointer arithmetic.
        let index_is_integer =
            index_ty == PoolId::ERROR || matches!(self.pool.item(index_ty), Item::IntType { .. });
        if !index_is_integer {
            let text = self.describe(index_ty);
            self.diags.push(
                Diagnostic::error(
                    index_span,
                    format!("an index must be an integer, not `{text}`"),
                )
                .with_code(E0235),
            );
            return elem;
        }

        // A literal index is decidable now, and a program that can only ever trap is
        // better refused. This does **not** replace the runtime check: it is one shape of
        // index out of many, and ADR-0039 §2's `BoundsCheck` still guards the rest.
        if let Some(len) = len
            && let Expr::Literal(Literal::Int { value, .. }, _) = self.expr_of(scope, index)
            && (value < 0 || u128::try_from(value).is_ok_and(|v| v >= u128::from(len)))
        {
            let text = self.describe(base_ty);
            let mut diag = Diagnostic::error(
                index_span,
                format!("index {value} is out of range for `{text}`"),
            )
            .with_code(E0236);
            diag = if len == 0 {
                diag.with_note("this array has no elements, so no index is in range")
            } else {
                diag.with_note(format!("valid indices are 0 to {}", len - 1))
            };
            self.diags.push(diag);
            return elem;
        }

        elem
    }

    /// The element type of something indexable, and its length when that is known.
    ///
    /// `Some((elem, Some(n)))` for `[N]T` and `Some((elem, None))` for `[]T`. The `None` is
    /// not a failure — it says the length is runtime data, which is the whole difference
    /// between an array and a view (ADR-0044 §1) — so a caller must not treat it as one.
    fn indexable_parts(&self, ty: PoolId) -> Option<(PoolId, Option<u64>)> {
        match self.pool.item(ty) {
            Item::ArrayType { elem, len } => Some((*elem, Some(*len))),
            Item::ViewType { elem } => Some((*elem, None)),
            _ => None,
        }
    }

    /// Types `base[]` — the slice operator (ADR-0044 §2).
    ///
    /// Only a `[N]T` may be sliced, and only into a `[]T` of the same element type. A view
    /// may **not** be sliced again: `xs[]` would be an identity, and an operator that
    /// silently does nothing is one a reader concludes did something (ADR-0044 §6).
    fn check_slice(&mut self, scope: ExprScope, base: ExprId, span: Span) -> PoolId {
        let mut base_ty = self.check_expr(scope, base, None);
        // Auto-deref, matching `check_index` and `check_field`: `p: *[4]u8` slices through
        // the pointer. The same loop in all three, so they cannot disagree about depth.
        while let Some(inner) = self.pointee(base_ty) {
            base_ty = inner;
        }
        if base_ty == PoolId::ERROR {
            return PoolId::ERROR;
        }

        let Some((elem, _)) = self.array_parts(base_ty) else {
            let text = self.describe(base_ty);
            let mut diag =
                Diagnostic::error(span, format!("cannot slice a value of type `{text}`"))
                    .with_code(E0239)
                    .with_note("`[]` makes a view over a fixed-size array `[N]T`");
            // A view sliced again is the mistake worth naming specifically, because the
            // expression *looks* harmless and the fix is to delete the operator.
            if matches!(self.pool.item(base_ty), Item::ViewType { .. }) {
                diag = diag.with_help("this is already a view — drop the `[]`");
            }
            self.diags.push(diag);
            return PoolId::ERROR;
        };

        // A view of a *constant* array would point at storage that has no address, so the
        // base must be a place. `is_place` is the same predicate assignment uses, which is
        // what keeps "can I take its address" one question with one answer.
        if !self.is_place(scope, base) {
            self.diags.push(
                Diagnostic::error(span, "cannot slice this expression")
                    .with_code(E0239)
                    .with_note("`[]` takes the address of its operand, so it needs storage")
                    .with_help("assign it to a variable first, then slice that"),
            );
            return self.pool.view_of(elem);
        }

        self.pool.view_of(elem)
    }

    /// The type an expression *denotes*, when it is a name bound to one (ADR-0071 §2).
    ///
    /// This is what makes `T :: Point;` an alias usable in a type annotation: `resolve_type_name`
    /// reads a `SigEntry::type_value`, so a type-valued constant has to carry the type it denotes
    /// rather than only `PoolId::TYPE`.
    ///
    /// **Reads the aliased name's own entry rather than re-resolving what `Point` means**, for the
    /// reason `jr-mir` reads `TypeMap` instead of typing expressions: two implementations of one rule
    /// are two chances to disagree. The signature phase already computed it.
    ///
    /// `None` for anything that is not a bare name — including a *chain* (`B :: A` where `A :: Point`),
    /// because `A`'s entry is a `SigKind::Const` and following it would need a fixpoint and a cycle
    /// check (ADR-0071 §5, the line ADR-0070 §4 drew for an array length).
    pub(crate) fn aliased_type(&mut self, scope: ExprScope, expr: ExprId) -> Option<PoolId> {
        let Expr::Name { res, .. } = self.expr_of(scope, expr) else {
            return None;
        };
        let res = self.resolve.get(scope, expr).unwrap_or(res);
        let entry = match res {
            Res::Item(item) => self.entry_for_item(item)?,
            Res::Imported(import, name) => self.entry_for_import(import, name)?,
            // A local, a parameter, or a promoted field is never a type: Jairs has no nested type
            // declarations, so none of them can put a type name in scope.
            Res::Local(_) | Res::Param(_) | Res::Promoted { .. } | Res::Error => return None,
        };
        // Only a *nominal declaration* is followed. A `SigKind::Const` whose own `type_value` is set is
        // exactly the alias chain §5 defers, so it is excluded by kind rather than by whether the field
        // happens to be populated — which keeps the refusal true if a later wave populates more of them.
        match entry.kind {
            SigKind::Struct | SigKind::Union | SigKind::Variant | SigKind::Enum => entry.type_value,
            SigKind::Const | SigKind::Var | SigKind::Proc | SigKind::Operator => None,
        }
    }

    /// The enum an expression *denotes*, when it is a name bound to an enum type.
    ///
    /// A receiver like `Colour` has type `type` (ADR-0012), so the type alone cannot say
    /// which enum — the *name* must be resolved to its declaration. Returns `None` for any
    /// other type-valued expression, including a struct name, which is what makes
    /// `Point.x` report "no field" rather than being mistaken for a member lookup.
    fn denoted_enum(
        &mut self,
        scope: ExprScope,
        receiver: ExprId,
    ) -> Option<(jr_pool::DeclId, bool)> {
        let Expr::Name { res, .. } = self.expr_of(scope, receiver) else {
            return None;
        };
        let res = self.resolve.get(scope, receiver).unwrap_or(res);
        let entry = match res {
            Res::Item(item) => self.entry_for_item(item)?,
            Res::Imported(import, name) => self.entry_for_import(import, name)?,
            // A promoted name is a *field*, and a field never denotes a type — Jairs has no
            // nested type declarations, so `using p: Point` cannot put a type name in scope.
            Res::Local(_) | Res::Param(_) | Res::Promoted { .. } | Res::Error => return None,
        };
        let denoted = entry.type_value?;
        match self.pool.item(denoted) {
            Item::EnumType { decl, flags } => Some((*decl, *flags)),
            _ => None,
        }
    }

    /// Reports a name that is not a member of the enum it was looked up in.
    ///
    /// The candidate set is the enum's own members, which is why the suggestion is computed
    /// here rather than in an editor: nothing outside this crate knows them (ADR-0031 §1).
    fn no_such_member(&mut self, decl: jr_pool::DeclId, flags: bool, member: &str, span: Span) {
        let candidates: Vec<String> = self
            .pool
            .enum_members(decl)
            .unwrap_or(&[])
            .iter()
            .map(|m| self.interner.resolve(m.name).to_owned())
            .collect();
        let ty = self.pool.enum_type(decl, flags);
        let text = self.describe(ty);
        let mut diag =
            Diagnostic::error(span, format!("`{text}` has no member `{member}`")).with_code(E0238);
        if let Some(near) = crate::suggest::nearest(member, candidates.iter().map(String::as_str)) {
            diag = diag.with_help(format!("did you mean `{near}`?"));
        }
        self.diags.push(diag);
    }

    /// Reports a field the receiver's type does not have, suggesting a near one.
    ///
    /// The candidate list is the receiver's own fields, which is why the suggestion is
    /// computed here rather than in an editor: nothing outside this crate knows them
    /// (ADR-0031 §1). Field order is declaration order, so a tie resolves to the field
    /// declared first rather than to whatever the pool iterated over.
    fn no_such_field(&mut self, ty: PoolId, field: &str, span: Span) {
        let text = self.describe(ty);
        let candidates: Vec<String> = match self.pool.item(ty) {
            // ADR-0004's two pseudo-fields, spelled out because `string` is not the
            // struct of its own layout and the pool has no field list for it.
            Item::StringType => vec![String::from("data"), String::from("count")],
            // Only `count`. Listing `data` would suggest a pseudo-field arrays do not have
            // (ADR-0039 §5), which is worse than no suggestion.
            Item::ArrayType { .. } | Item::ViewType { .. } => vec![String::from("count")],
            Item::StructType { decl } | Item::UnionType { decl } | Item::VariantType { decl } => {
                self.pool
                    .struct_fields(*decl)
                    .unwrap_or(&[])
                    .iter()
                    .map(|f| self.interner.resolve(f.name).to_owned())
                    .collect()
            }
            _ => Vec::new(),
        };

        let mut diag = Diagnostic::error(span, format!("no field `{field}` on type `{text}`"))
            .with_code(E0218);
        if let Some(near) = crate::suggest::nearest(field, candidates.iter().map(String::as_str)) {
            diag = diag.with_help(format!("did you mean `{near}`?"));
        }
        self.diags.push(diag);
    }

    /// Types a dereference.
    fn check_deref(&mut self, scope: ExprScope, pointer: ExprId, span: Span) -> PoolId {
        let ty = self.check_expr(scope, pointer, None);
        if ty == PoolId::ERROR {
            return PoolId::ERROR;
        }
        match self.pointee(ty) {
            Some(inner) => inner,
            None => {
                let text = self.describe(ty);
                self.diags.push(
                    Diagnostic::error(span, format!("cannot dereference `{text}`"))
                        .with_code(E0219)
                        .with_note("`.*` applies to a pointer"),
                );
                PoolId::ERROR
            }
        }
    }

    /// Types a directive used as an expression.
    fn check_directive(
        &mut self,
        name: Symbol,
        arg: Option<&str>,
        expected: Option<PoolId>,
        span: Span,
    ) -> PoolId {
        let interner = self.interner;
        let ty = match interner.resolve(name) {
            // ADR-0016 §3. The value is interned as well as the type, so that the
            // FFI boundary has an identity and not merely a shape.
            "system_library" | "library" => {
                if let Some(library) = arg {
                    let _ = self.pool.foreign_library_value(library);
                }
                PoolId::FOREIGN_LIBRARY
            }
            // Every other directive in expression position was already rejected
            // by lowering (E0209); a second complaint would be noise.
            _ => PoolId::ERROR,
        };
        self.expect(expected, ty, span)
    }

    // -----------------------------------------------------------------------
    // Expression predicates
    // -----------------------------------------------------------------------

    /// Returns `true` if this expression denotes a location, not just a value.
    ///
    /// Called after the expression has been typed, because a field access on a
    /// pointer is assignable and deciding that needs the receiver's type.
    fn is_place(&mut self, scope: ExprScope, id: ExprId) -> bool {
        match self.expr_of(scope, id) {
            // **`context` itself is not a place** — it is the pointer value, not storage — but
            // `context.allocator` is, because `Expr::Field` on a pointer receiver is assignable and
            // that arm decides it from the receiver's *type*. So writing the field works and
            // rebinding `context` wholesale does not, which is ADR-0057 §2's shape exactly.
            Expr::Context(_) => false,
            Expr::Name { res, .. } => {
                let res = self.resolve.get(scope, id).unwrap_or(res);
                match res {
                    Res::Local(_) | Res::Param(_) => true,
                    // A promoted name **is** a place: `x` where `using p: Point` is in scope means
                    // `p.x`, and an ordinary `p.x` is assignable. Answering `false` here would
                    // silently make `x = 1` a "cannot assign" error inside any procedure taking a
                    // `using` parameter — the promotion would look read-only for no stated reason.
                    Res::Promoted { .. } => true,
                    Res::Item(item) => self
                        .entry_for_item(item)
                        .is_some_and(|entry| entry.is_assignable()),
                    Res::Imported(import, name) => self
                        .entry_for_import(import, name)
                        .is_some_and(|entry| entry.is_assignable()),
                    // Poison: an unresolved name is already an error, and calling
                    // it unassignable as well would report it twice.
                    Res::Error => true,
                }
            }
            Expr::Field { receiver, .. } => {
                // An enum member is a compile-time constant with no storage, so
                // `Colour.RED = 2` is not assignable and `*Colour.RED` has no address to
                // take (ADR-0041 §5). Checked on the *receiver*: a type-valued receiver is
                // never a place, which is also the right answer for a hypothetical
                // `Point.x`.
                let receiver_is_type = self
                    .types
                    .expr_type(scope, receiver)
                    .is_some_and(|ty| ty == PoolId::TYPE);
                if receiver_is_type {
                    return false;
                }
                let through_pointer = self
                    .types
                    .expr_type(scope, receiver)
                    .is_some_and(|ty| self.pointee(ty).is_some());
                through_pointer || self.is_place(scope, receiver)
            }
            // Indexing names a location whenever the thing indexed does. `a[i] = x` is
            // legal for a local array; a hypothetical array-valued *constant* is not
            // assignable, and this defers to the base for exactly that reason rather than
            // answering `true` outright the way `Deref` can.
            Expr::Index { base, .. } => self.is_place(scope, base),
            // A view *is* a pointer to storage, so indexing one always names a location —
            // there is nothing to defer to the base about. But `xs[]` itself produces a
            // two-word value, so slicing is not a place (ADR-0044 §4).
            Expr::Slice { .. } => false,
            // A dereference always names a location.
            Expr::Deref(..) => true,
            // A cast produces a *value*, never a location: `cast(u8, n) = 1` is not
            // assignable even though `n` is.
            Expr::Literal(..)
            | Expr::Binary { .. }
            | Expr::Unary { .. }
            | Expr::Call { .. }
            | Expr::Uninit(_)
            | Expr::Cast { .. }
            // Both produce values. A bare `.RED` is a constant with no storage, exactly as
            // `Colour.RED` is (ADR-0041 §5), and `xx n` is a conversion's result.
            | Expr::Autocast { .. }
            | Expr::Member { .. }
            | Expr::Run(..)
            | Expr::Directive { .. } => false,
            // Error recovery: stay quiet.
            Expr::Error(_) => true,
        }
    }

    /// Returns `true` if this expression is built only out of integer literals.
    ///
    /// Such an expression has no type of its own and takes the other operand's,
    /// which is what makes `1 + 2 == count` compare `s64`s rather than reporting
    /// a mismatch against a defaulted `s64`.
    fn is_untyped_literal(&self, scope: ExprScope, id: ExprId) -> bool {
        match self.expr_of(scope, id) {
            Expr::Literal(literal, _) => match literal {
                // A float literal is untyped for the same reason an integer one is
                // (ADR-0040 §5): it takes its type from context, so `1.5 + x` where `x` is a
                // `float32` must make the literal a `float32` rather than defaulting it to
                // `float64` and then reporting a mismatch.
                //
                // Note this does *not* let `1 + some_float64` through: `1` is an *integer*
                // literal, and its context typing gives it the integer interpretation, so the
                // operands still disagree. ADR-0040 §6 keeps that asymmetry deliberately —
                // `1` and `1.0` are different literals.
                Literal::Int { .. } | Literal::Float { .. } => true,
                // `null` takes its type from context too (ADR-0060 §1), so `p == null` types the
                // `null` as `p`'s pointer type rather than reporting a mismatch — the same reason
                // an integer literal is untyped here.
                Literal::Null => true,
                Literal::Str(_) | Literal::Bool(_) => false,
            },
            Expr::Unary { op, operand, .. } => match op {
                // `~1` is untyped for the same reason `-1` is: the complement of an untyped
                // literal is still an untyped literal, so `x: u8 = ~0;` must take `u8` from
                // its context rather than defaulting to `s64` and then mismatching.
                UnOp::Neg | UnOp::BitNot => self.is_untyped_literal(scope, operand),
                UnOp::Not | UnOp::AddrOf => false,
            },
            Expr::Binary { op, lhs, rhs, .. } => {
                is_arithmetic(op)
                    && self.is_untyped_literal(scope, lhs)
                    && self.is_untyped_literal(scope, rhs)
            }
            Expr::Run(inner, _) => self.is_untyped_literal(scope, inner),
            // An element of an array has the element's type, which is a real type: `buf[0]`
            // is a `u8` and must not take the other operand's type the way `1` does.
            // A view is a real type for the same reason.
            Expr::Index { .. } | Expr::Slice { .. } => false,
            // A cast is emphatically **not** untyped: naming a type is the whole point, so
            // `cast(u8, 1) + big_s64` must be a type error rather than quietly taking `s64`.
            // Answering `true` here would make the cast advisory.
            // Neither is untyped: an `xx` has the context's type and a `.RED` has its enum's.
            // Answering `true` would make them take the *other* operand's type in a binary
            // expression — a second context-typing rule fighting the first (ADR-0046 §1).
            Expr::Cast { .. }
            | Expr::Autocast { .. }
            | Expr::Member { .. }
            | Expr::Name { .. }
            | Expr::Call { .. }
            | Expr::Context(_)
            | Expr::Field { .. }
            | Expr::Deref(..)
            | Expr::Uninit(_)
            | Expr::Directive { .. }
            | Expr::Error(_) => false,
        }
    }

    // -----------------------------------------------------------------------
    // Arena access
    // -----------------------------------------------------------------------

    /// Returns the expression `id` names in the arena `scope` selects.
    ///
    /// An index outside that arena yields `Expr::Error`, so a mismatched
    /// arena degrades to poison instead of silently reading another node.
    fn expr_of(&self, scope: ExprScope, id: ExprId) -> Expr {
        let hir = self.hir;
        let arena = match scope {
            ExprScope::TopLevel => &hir.exprs,
            ExprScope::Body(body) => &hir.body(body).exprs,
        };
        arena
            .get(id.index())
            .cloned()
            .unwrap_or(Expr::Error(self.nowhere()))
    }

    /// A span for a node that has none, used only in error recovery.
    fn nowhere(&self) -> Span {
        Span::new(self.file, TextRange::default())
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// Which field lookup a receiver's type calls for.
enum ReceiverKind {
    /// The builtin `string`, whose `.data`/`.count` are pseudo-fields.
    Str,
    /// A nominal struct, whose fields live in the pool under this declaration.
    Struct(jr_pool::DeclId),
    /// A fixed-size array, whose `.count` is a pseudo-field read from the type.
    Array,
    /// The implicit context, whose fields are the compiler's (ADR-0057 §1).
    ///
    /// Its own variant rather than a [`ReceiverKind::Struct`] because a context has no `DeclId` — a
    /// compiler-declared type has no declaration site — so its fields cannot be in the struct side
    /// table that variant reads.
    Context,
    /// A view, whose `.count` is a pseudo-field **loaded** from the value (ADR-0044 §4).
    ///
    /// Distinct from [`ReceiverKind::Array`] even though both answer `.count` with an `s64`,
    /// because the two differ in *where the answer comes from*: an array's is a constant from
    /// the type and a view's is a load. MIR needs that difference and a shared variant would
    /// hide it.
    View,
    /// An enum type used as a receiver, whose "fields" are its members (ADR-0041 §1).
    ///
    /// Carries `flags` as well as the declaration, because rebuilding the type needs it
    /// (ADR-0043 §2) and a second lookup is a second chance to disagree.
    Enum(jr_pool::DeclId, bool),
    /// Anything else: no fields at all.
    Fieldless,
}

/// Returns `true` for the arithmetic operators.
fn is_arithmetic(op: BinOp) -> bool {
    match op {
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Div
        | BinOp::Rem
        | BinOp::WrapAdd
        | BinOp::WrapSub
        | BinOp::WrapMul => true,
        // Bitwise operators are *not* "arithmetic" for this predicate's purpose: its only
        // caller distinguishes an arithmetic message from an ordering one when rejecting an
        // enum operator (ADR-0041 §6), and a bitwise operator on an enum gets its own message
        // from `reject_bitwise` before reaching that path.
        BinOp::Eq
        | BinOp::Ne
        | BinOp::Lt
        | BinOp::Le
        | BinOp::Gt
        | BinOp::Ge
        | BinOp::And
        | BinOp::Or
        | BinOp::BitAnd
        | BinOp::BitOr
        | BinOp::BitXor
        | BinOp::Shl
        | BinOp::Shr => false,
    }
}

/// The source spelling of a binary operator, for diagnostics.
pub(crate) fn bin_op_text(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::WrapAdd => "+%",
        BinOp::WrapSub => "-%",
        BinOp::WrapMul => "*%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
        BinOp::BitAnd => "&",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
    }
}

/// Returns `true` if an integer literal of magnitude `value` fits the type.
///
/// The HIR stores a literal's magnitude, never a sign — `-1` is negation applied
/// to `1` — so this is a bound on the magnitude. The consequence, stated because
/// it is a real limitation: the most negative value of a signed type cannot be
/// written as a literal, because its magnitude is one past the positive bound.
///
/// Written against `(signed, bits)` rather than against `s64` and `u8` because
/// wave W1's full numeric tower would otherwise rewrite it.
fn literal_fits(signed: bool, bits: u16, value: i128) -> bool {
    // Against the type's **range**, not its maximum magnitude. The old test compared a
    // magnitude, so `-128` was 128 tested against `s8`'s 127 and every signed minimum was
    // unwritable (ADR-0038). `IntKind` already computes both bounds, and using it here means
    // the fit check and the arithmetic cannot disagree about what a type holds.
    let kind = jr_pool::IntKind { signed, bits };
    value >= kind.min() && value <= kind.max()
}

/// A human-readable range for an integer type, for the E0204 note.
///
/// From `IntKind`, the same source `literal_fits` tests against — so the note cannot print a
/// range the check does not enforce. It used to derive both bounds from the maximum magnitude,
/// which is how it came to print "the range of `s8` is -128 to 127" while rejecting `-128`.
fn int_range(signed: bool, bits: u16) -> String {
    let kind = jr_pool::IntKind { signed, bits };
    format!("{} to {}", kind.min(), kind.max())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_bounds_are_per_type_not_per_s64() {
        assert!(literal_fits(false, 8, 255));
        assert!(!literal_fits(false, 8, 256));
        assert!(literal_fits(true, 64, i128::from(i64::MAX)));
        assert!(!literal_fits(true, 64, i128::from(i64::MAX) + 1));
        assert!(literal_fits(false, 64, i128::from(u64::MAX)));
    }

    #[test]
    fn a_signed_minimum_fits_its_own_type() {
        // The bug ADR-0038 fixed. `literal_fits` compared a *magnitude* against the maximum,
        // so 128 was tested against `s8`'s 127 and every signed minimum was rejected — by a
        // diagnostic that printed the range the value sits inside.
        for bits in [8u16, 16, 32, 64] {
            let kind = jr_pool::IntKind { signed: true, bits };
            assert!(
                literal_fits(true, bits, kind.min()),
                "s{bits}'s minimum must fit s{bits}"
            );
            assert!(
                !literal_fits(true, bits, kind.min() - 1),
                "one below s{bits}'s minimum must not"
            );
            assert!(literal_fits(true, bits, kind.max()));
            assert!(!literal_fits(true, bits, kind.max() + 1));
        }
    }

    #[test]
    fn a_negative_literal_never_fits_an_unsigned_type() {
        // Free with a signed comparison, and *not* free with a magnitude one: the old test
        // would have accepted `u8 = -1` as the magnitude 1.
        for bits in [8u16, 16, 32, 64] {
            assert!(!literal_fits(false, bits, -1));
        }
    }

    #[test]
    fn ranges_read_the_way_a_user_expects() {
        assert_eq!(int_range(false, 8), "0 to 255");
        assert_eq!(int_range(true, 8), "-128 to 127");
    }

    #[test]
    fn every_printed_range_is_a_range_the_check_accepts() {
        // The note and the check now read the same `IntKind`, which is what stops the
        // diagnostic printing a bound it then rejects — the shape of the ADR-0038 bug.
        for bits in [8u16, 16, 32, 64] {
            for signed in [true, false] {
                let kind = jr_pool::IntKind { signed, bits };
                let printed = int_range(signed, bits);
                assert!(printed.starts_with(&kind.min().to_string()), "{printed}");
                assert!(printed.ends_with(&kind.max().to_string()), "{printed}");
                assert!(literal_fits(signed, bits, kind.min()));
                assert!(literal_fits(signed, bits, kind.max()));
            }
        }
    }
}
