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

    CheckOutput {
        types: ctx.types,
        diagnostics: ctx.diags,
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
            Stmt::Break(_) | Stmt::Continue(_) | Stmt::Error(_) => {}
        }
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
            | AssignOp::WrapMulAssign => true,
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
                self.check_binary(scope, op, lhs, rhs, expected, span)
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
            Expr::Deref(pointer, span) => {
                let ty = self.check_deref(scope, pointer, span);
                self.expect(expected, ty, span)
            }
            // `---` in an initialiser never reaches here: lowering records it as
            // a flag on the declaration. Anywhere else it has no type of its own,
            // so it takes the context's and stays quiet.
            Expr::Uninit(_) => expected.unwrap_or(PoolId::ERROR),
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

    /// Types a literal.
    fn check_literal(&mut self, literal: &Literal, expected: Option<PoolId>, span: Span) -> PoolId {
        match literal {
            Literal::Bool(_) => self.expect(expected, PoolId::BOOL, span),
            Literal::Str(_) => self.expect(expected, PoolId::STRING, span),
            Literal::Int { value, .. } => self.check_int_literal(*value, expected, span),
        }
    }

    /// Types an integer literal against its context (ADR-0016 §1).
    ///
    /// The literal has no intrinsic type. It takes the context's, defaults to
    /// `s64` when there is none, and must fit whichever type it ends up with.
    /// Note what this means for diagnostics: the *contextual* type is the only
    /// one worth naming, because the literal has no other.
    fn check_int_literal(&mut self, value: u64, expected: Option<PoolId>, span: Span) -> PoolId {
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
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
        expected: Option<PoolId>,
        span: Span,
    ) -> PoolId {
        match op {
            BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Rem
            | BinOp::WrapAdd
            | BinOp::WrapSub
            | BinOp::WrapMul => {
                // Push an integer context inward so that `g: u8 = 1 + 2;` types
                // both literals as `u8` rather than defaulting them to `s64` and
                // then complaining.
                let want = expected.filter(|ty| self.int_info(*ty).is_some());
                let (left, right) = self.check_operands(scope, lhs, rhs, want);
                let result = self.unify_operands(left, right, span);
                if result != PoolId::ERROR && self.int_info(result).is_none() {
                    let text = self.describe(result);
                    self.reject_operator(op, &text, span);
                    return PoolId::ERROR;
                }
                self.expect(expected, result, span)
            }
            BinOp::Eq | BinOp::Ne => {
                let (left, right) = self.check_operands(scope, lhs, rhs, None);
                self.unify_operands(left, right, span);
                self.expect(expected, PoolId::BOOL, span)
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let (left, right) = self.check_operands(scope, lhs, rhs, None);
                let operand = self.unify_operands(left, right, span);
                if operand != PoolId::ERROR && self.int_info(operand).is_none() {
                    let text = self.describe(operand);
                    self.reject_operator(op, &text, span);
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
            UnOp::Neg => {
                let want = expected.filter(|ty| self.int_info(*ty).is_some());
                let ty = self.check_expr(scope, operand, want);
                if ty != PoolId::ERROR && self.int_info(ty).is_none() {
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
            Item::StructType { decl } => ReceiverKind::Struct(*decl),
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

    /// Reports a field the receiver's type does not have.
    fn no_such_field(&mut self, ty: PoolId, field: &str, span: Span) {
        let text = self.describe(ty);
        self.diags.push(
            Diagnostic::error(span, format!("no field `{field}` on type `{text}`"))
                .with_code(E0218),
        );
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
                let through_pointer = self
                    .types
                    .expr_type(scope, receiver)
                    .is_some_and(|ty| self.pointee(ty).is_some());
                through_pointer || self.is_place(scope, receiver)
            }
            // A dereference always names a location.
            Expr::Deref(..) => true,
            Expr::Literal(..)
            | Expr::Binary { .. }
            | Expr::Unary { .. }
            | Expr::Call { .. }
            | Expr::Uninit(_)
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
                Literal::Int { .. } => true,
                Literal::Str(_) | Literal::Bool(_) => false,
            },
            Expr::Unary { op, operand, .. } => match op {
                UnOp::Neg => self.is_untyped_literal(scope, operand),
                UnOp::Not | UnOp::AddrOf => false,
            },
            Expr::Binary { op, lhs, rhs, .. } => {
                is_arithmetic(op)
                    && self.is_untyped_literal(scope, lhs)
                    && self.is_untyped_literal(scope, rhs)
            }
            Expr::Run(inner, _) => self.is_untyped_literal(scope, inner),
            Expr::Name { .. }
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
        BinOp::Eq
        | BinOp::Ne
        | BinOp::Lt
        | BinOp::Le
        | BinOp::Gt
        | BinOp::Ge
        | BinOp::And
        | BinOp::Or => false,
    }
}

/// The source spelling of a binary operator, for diagnostics.
fn bin_op_text(op: BinOp) -> &'static str {
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
fn literal_fits(signed: bool, bits: u16, value: u64) -> bool {
    value <= max_magnitude(signed, bits)
}

/// The largest magnitude an integer type can hold.
fn max_magnitude(signed: bool, bits: u16) -> u64 {
    if bits >= 64 {
        return if signed { i64::MAX as u64 } else { u64::MAX };
    }
    let width = u32::from(bits) - u32::from(signed);
    (1u64 << width) - 1
}

/// A human-readable range for an integer type, for the E0204 note.
fn int_range(signed: bool, bits: u16) -> String {
    let max = max_magnitude(signed, bits);
    if signed {
        format!("-{} to {max}", max.wrapping_add(1))
    } else {
        format!("0 to {max}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_bounds_are_per_type_not_per_s64() {
        assert!(literal_fits(false, 8, 255));
        assert!(!literal_fits(false, 8, 256));
        assert!(literal_fits(true, 64, i64::MAX as u64));
        assert!(!literal_fits(true, 64, i64::MAX as u64 + 1));
        assert!(literal_fits(false, 64, u64::MAX));
    }

    #[test]
    fn ranges_read_the_way_a_user_expects() {
        assert_eq!(int_range(false, 8), "0 to 255");
        assert_eq!(int_range(true, 8), "-128 to 127");
    }
}
