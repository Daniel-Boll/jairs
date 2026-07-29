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
    E0232, E0234, E0235, E0236, E0238, E0239, E0241, E0242, E0243, E0244, E0247,
};
use crate::ctx::{BodyEnv, Ctx, Mode};
use crate::map::TypeMap;
use crate::sigs::FileSignatures;

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

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
            Expr::Literal(literal, span) => self.check_literal(&literal, expected, span),
            Expr::Name { span, res, .. } => {
                let res = self.resolve.get(scope, id).unwrap_or(res);
                let ty = self.type_of_name(res);
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
            Expr::Call { callee, args, span } => {
                let ty = self.check_call(scope, callee, &args, span);
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

        // A context that is not an enum is a *different* problem from having none, so it gets
        // its own wording with the type named — conflating them would misdirect the reader
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
                Literal::Int { .. } | Literal::Str(_) | Literal::Bool(_) => false,
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

    /// Types a name reference from its resolution.
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
            Res::Error => PoolId::ERROR,
        }
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
        callee: ExprId,
        args: &[ExprId],
        span: Span,
    ) -> PoolId {
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

    /// Types a field access, looking through pointers.
    fn check_field(
        &mut self,
        scope: ExprScope,
        receiver: ExprId,
        name: Symbol,
        name_span: Span,
    ) -> PoolId {
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
            // diagnostics. Only the offsets differ, and those are `jr-pool`'s (ADR-0045 §5).
            Item::StructType { decl } | Item::UnionType { decl } => ReceiverKind::Struct(*decl),
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
                let found = self
                    .pool
                    .struct_fields(decl)
                    .and_then(|fields| fields.iter().find(|f| f.name == name).map(|f| f.ty));
                match found {
                    Some(field_ty) => field_ty,
                    None => {
                        self.no_such_field(ty, field, name_span);
                        PoolId::ERROR
                    }
                }
            }
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
            Res::Local(_) | Res::Param(_) | Res::Error => return None,
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
            Item::StructType { decl } | Item::UnionType { decl } => self
                .pool
                .struct_fields(*decl)
                .unwrap_or(&[])
                .iter()
                .map(|f| self.interner.resolve(f.name).to_owned())
                .collect(),
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
            Expr::Name { res, .. } => {
                let res = self.resolve.get(scope, id).unwrap_or(res);
                match res {
                    Res::Local(_) | Res::Param(_) => true,
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
