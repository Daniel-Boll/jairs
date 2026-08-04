//! Pretty-printer for [`FileHir`].
//!
//! Produces a human-readable text dump of the HIR, analogous to
//! `jr_syntax::dump_tree` for the CST. This is indispensable when debugging
//! `jr-sema` and `jr-mir`.
//!
//! # Example
//!
//! ```
//! # use jr_base::{FileId, Interner};
//! # use jr_syntax::parse;
//! # use jr_hir::{lower_file, dump::dump_hir};
//! let interner = Interner::new();
//! let file = FileId::from_usize(0);
//! let parse = parse("main :: () { }", file);
//! let (hir, _diags) = lower_file(&parse, file, &interner);
//! let text = dump_hir(&hir, &interner);
//! assert!(text.contains("Proc"));
//! ```

use jr_base::Interner;

use crate::hir::{
    AggregateKind, AssignOp, BinOp, Body, BodyId, ConstValue, Expr, ExprId, FileHir, ForIterable,
    ItemKind, Literal, Res, Stmt, StmtId, TypeRef, TypeRefId, UnOp,
};

/// Produces a human-readable dump of the HIR for one file.
pub fn dump_hir(hir: &FileHir, interner: &Interner) -> String {
    let mut out = String::new();
    let mut d = Dumper {
        hir,
        interner,
        out: &mut out,
        indent: 0,
    };
    d.dump_file();
    out
}

struct Dumper<'a> {
    hir: &'a FileHir,
    interner: &'a Interner,
    out: &'a mut String,
    indent: usize,
}

impl<'a> Dumper<'a> {
    fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn sym(&self, sym: jr_base::Symbol) -> &str {
        self.interner.resolve(sym)
    }

    fn dump_file(&mut self) {
        self.line("FileHir {");
        self.indent += 1;
        for (i, item) in self.hir.items.iter().enumerate() {
            let name = item
                .name
                .map(|s| self.sym(s).to_owned())
                .unwrap_or_else(|| "<anon>".to_owned());
            match &item.kind {
                ItemKind::Const { value } => match value {
                    ConstValue::Proc(pid) => {
                        let proc = &self.hir.procs[pid.index()];
                        let params: Vec<String> = proc
                            .params
                            .iter()
                            .map(|p| {
                                let ty =
                                    p.ty.map(|t| self.fmt_type_ref_top(t))
                                        .unwrap_or_else(|| "?".to_owned());
                                format!("{}: {}", self.sym(p.name), ty)
                            })
                            .collect();
                        let ret = proc
                            .ret
                            .map(|t| self.fmt_type_ref_top(t))
                            .unwrap_or_else(|| "()".to_owned());
                        let foreign = if proc.foreign.is_some() {
                            " #foreign"
                        } else {
                            ""
                        };
                        self.line(&format!(
                            "Item[{i}] Const {name} :: Proc({}) -> {ret}{foreign}",
                            params.join(", ")
                        ));
                        if let Some(body_id) = proc.body {
                            self.indent += 1;
                            self.dump_body(body_id);
                            self.indent -= 1;
                        }
                    }
                    // One arm for both forms, reading the keyword from `is_union` — the
                    // same discipline `jr-fmt` learned the hard way when emitting a literal
                    // `"enum"` silently rewrote `enum_flags` (ADR-0043).
                    // Printed with the operator so a dump cannot make an overload look like an
                    // ordinary procedure named `operator+`, which is exactly what its synthetic
                    // symbol would otherwise suggest.
                    ConstValue::Operator(pid, op) => {
                        let proc = &self.hir.procs[pid.index()];
                        let params: Vec<String> = proc
                            .params
                            .iter()
                            .map(|p| {
                                let ty =
                                    p.ty.map(|t| self.fmt_type_ref_top(t))
                                        .unwrap_or_else(|| "?".to_owned());
                                format!("{}: {}", self.sym(p.name), ty)
                            })
                            .collect();
                        self.line(&format!(
                            "Item[{i}] Operator {} :: ({})",
                            fmt_bin_op(*op),
                            params.join(", ")
                        ));
                        if let Some(body_id) = proc.body {
                            self.indent += 1;
                            self.dump_body(body_id);
                            self.indent -= 1;
                        }
                    }
                    ConstValue::Struct(sid) | ConstValue::Union(sid) | ConstValue::Variant(sid) => {
                        let s = &self.hir.structs[sid.index()];
                        // One arm for all three forms, reading the keyword from `kind` — a match
                        // rather than a chain of ifs, so a fourth form is a compile error here
                        // (ADR-0068 §2).
                        let keyword = match s.kind {
                            AggregateKind::Struct => "Struct",
                            AggregateKind::Union => "Union",
                            AggregateKind::Variant => "Variant",
                        };
                        self.line(&format!("Item[{i}] Const {name} :: {keyword} {{"));
                        self.indent += 1;
                        for f in &s.fields {
                            let ty =
                                f.ty.map(|t| self.fmt_type_ref_top(t))
                                    .unwrap_or_else(|| "?".to_owned());
                            self.line(&format!("{}: {}", self.sym(f.name), ty));
                        }
                        self.indent -= 1;
                        self.line("}");
                    }
                    ConstValue::Enum(eid) => {
                        let e = &self.hir.enums[eid.index()];
                        self.line(&format!("Item[{i}] Const {name} :: Enum {{"));
                        self.indent += 1;
                        for m in &e.members {
                            // An auto-numbered member prints without a value, because that is
                            // what the source said — the *number* is `jr-sema`'s and printing
                            // one here would show a computation this phase did not do.
                            // The name is bound before `line` is called, because `line`
                            // borrows `self` mutably and `sym` borrows it immutably.
                            let member_name = self.sym(m.name).to_owned();
                            match m.value {
                                Some(v) => {
                                    let value = self.fmt_top_expr(v);
                                    self.line(&format!("{member_name} :: {value}"));
                                }
                                None => self.line(&member_name),
                            }
                        }
                        self.indent -= 1;
                        self.line("}");
                    }
                    ConstValue::Expr(eid) => {
                        let expr_str = self.fmt_top_expr(*eid);
                        self.line(&format!("Item[{i}] Const {name} :: {expr_str}"));
                    }
                },
                ItemKind::Var { ty, init, uninit } => {
                    let ty_str = ty
                        .map(|t| self.fmt_type_ref_top(t))
                        .unwrap_or_else(|| "?".to_owned());
                    let init_str = if *uninit {
                        " = ---".to_owned()
                    } else {
                        init.map(|e| format!(" = {}", self.fmt_top_expr(e)))
                            .unwrap_or_default()
                    };
                    self.line(&format!("Item[{i}] Var {name}: {ty_str}{init_str}"));
                }
                ItemKind::Import { path, .. } => {
                    self.line(&format!("Item[{i}] Import \"{path}\""));
                }
                ItemKind::Run { expr } => {
                    let expr_str = self.fmt_top_expr(*expr);
                    self.line(&format!("Item[{i}] Run {expr_str}"));
                }
            }
        }
        self.indent -= 1;
        self.line("}");
    }

    fn dump_body(&mut self, body_id: BodyId) {
        let body = &self.hir.bodies[body_id.index()];
        let root = body.root;
        self.line("Body {");
        self.indent += 1;
        self.dump_body_stmt(body, root);
        self.indent -= 1;
        self.line("}");
    }

    fn dump_body_stmt(&mut self, body: &Body, stmt_id: StmtId) {
        match &body.stmts[stmt_id.index()] {
            Stmt::Block(stmts, _) => {
                self.line("Block {");
                self.indent += 1;
                let stmts = stmts.clone();
                for sid in stmts {
                    self.dump_body_stmt(body, sid);
                }
                self.indent -= 1;
                self.line("}");
            }
            // A discard prints as `_`, which is what it is in the source — a hole rather than a
            // binding, so there is no name to print (ADR-0052 §3).
            Stmt::LocalTuple {
                targets,
                call,
                span: _,
            } => {
                let names: Vec<String> = targets
                    .iter()
                    .map(|t| match t {
                        Some(local) => self.sym(body.locals[local.index()].name).to_owned(),
                        None => "_".to_owned(),
                    })
                    .collect();
                let call = *call;
                let rhs = self.fmt_body_expr(body, call);
                self.line(&format!("LocalTuple {} := {rhs}", names.join(", ")));
            }
            Stmt::AssignTuple {
                targets,
                call,
                span: _,
            } => {
                let targets = targets.clone();
                let call = *call;
                let parts: Vec<String> = targets
                    .iter()
                    .map(|t| match t {
                        Some(expr) => self.fmt_body_expr(body, *expr),
                        None => "_".to_owned(),
                    })
                    .collect();
                let rhs = self.fmt_body_expr(body, call);
                self.line(&format!("AssignTuple {} = {rhs}", parts.join(", ")));
            }
            Stmt::ReturnTuple(exprs, _) => {
                let exprs = exprs.clone();
                let parts: Vec<String> =
                    exprs.iter().map(|e| self.fmt_body_expr(body, *e)).collect();
                self.line(&format!("ReturnTuple {}", parts.join(", ")));
            }
            Stmt::Local(local_id, _) => {
                let local = &body.locals[local_id.index()];
                let name = self.sym(local.name).to_owned();
                let ty_str = local
                    .ty
                    .map(|t| self.fmt_type_ref_body(body, t))
                    .unwrap_or_else(|| "?".to_owned());
                let init_str = if local.uninit {
                    " = ---".to_owned()
                } else {
                    local
                        .init
                        .map(|e| format!(" = {}", self.fmt_body_expr(body, e)))
                        .unwrap_or_default()
                };
                self.line(&format!("Local {name}: {ty_str}{init_str}"));
            }
            Stmt::Item(_, _) => {
                self.line("Item(nested)");
            }
            Stmt::Expr(expr_id, _) => {
                let s = self.fmt_body_expr(body, *expr_id);
                self.line(&format!("Expr {s}"));
            }
            Stmt::Assign { lhs, op, rhs, .. } => {
                let lhs_s = self.fmt_body_expr(body, *lhs);
                let rhs_s = self.fmt_body_expr(body, *rhs);
                let op_s = fmt_assign_op(*op);
                self.line(&format!("Assign {lhs_s} {op_s} {rhs_s}"));
            }
            Stmt::If {
                cond, then, else_, ..
            } => {
                let cond_s = self.fmt_body_expr(body, *cond);
                let then = *then;
                let else_ = *else_;
                self.line(&format!("If {cond_s}"));
                self.indent += 1;
                self.dump_body_stmt(body, then);
                if let Some(e) = else_ {
                    self.indent -= 1;
                    self.line("Else");
                    self.indent += 1;
                    self.dump_body_stmt(body, e);
                }
                self.indent -= 1;
            }
            Stmt::While {
                cond,
                body: body_stmt,
                ..
            } => {
                let cond_s = self.fmt_body_expr(body, *cond);
                let body_stmt = *body_stmt;
                self.line(&format!("While {cond_s}"));
                self.indent += 1;
                self.dump_body_stmt(body, body_stmt);
                self.indent -= 1;
            }
            // Printed with the loop variables, the direction and the label, because every one of
            // them changes what the loop does and a dump that hid any would be useless for the
            // thing it is read for.
            Stmt::For {
                value,
                index,
                iterable,
                reverse,
                body: body_stmt,
                label,
                ..
            } => {
                let value_name = self.sym(body.locals[value.index()].name).to_owned();
                let index_name = index
                    .map(|i| format!(", {}", self.sym(body.locals[i.index()].name)))
                    .unwrap_or_default();
                let over = match iterable {
                    ForIterable::Sequence(e) => self.fmt_body_expr(body, *e),
                    ForIterable::Range { start, end } => format!(
                        "{}..{}",
                        self.fmt_body_expr(body, *start),
                        self.fmt_body_expr(body, *end)
                    ),
                };
                let dir = if *reverse { "< " } else { "" };
                let tag = label
                    .map(|l| format!("{}: ", self.sym(l)))
                    .unwrap_or_default();
                let body_stmt = *body_stmt;
                self.line(&format!("{tag}For {dir}{value_name}{index_name}: {over}"));
                self.indent += 1;
                self.dump_body_stmt(body, body_stmt);
                self.indent -= 1;
            }
            Stmt::Defer(inner, _) => {
                let inner = *inner;
                self.line("Defer");
                self.indent += 1;
                self.dump_body_stmt(body, inner);
                self.indent -= 1;
            }
            Stmt::PushContext(inner, _) => {
                let inner = *inner;
                self.line("PushContext");
                self.indent += 1;
                self.dump_body_stmt(body, inner);
                self.indent -= 1;
            }
            // Printed as its own node with the statements nested, so a snapshot shows *what an insert
            // became* — which is the only place the inserted text is visible after lowering, since every
            // statement carries the directive's span rather than one of its own (ADR-0072 §2).
            Stmt::Insert { stmts, operand, .. } => {
                let stmts = stmts.clone();
                let operand = *operand;
                // A **pending** computed insert (ADR-0073) prints its operand and no statements, so a
                // dump shows the pre-expansion shape; a literal or expanded one prints its statements.
                if let Some(op) = operand {
                    self.line(&format!("Insert (computed operand e{})", op.index()));
                } else {
                    self.line(&format!("Insert ({} stmts)", stmts.len()));
                }
                self.indent += 1;
                for inner in stmts {
                    self.dump_body_stmt(body, inner);
                }
                self.indent -= 1;
            }
            Stmt::Switch { value, arms, .. } => {
                let value = *value;
                let arms = arms.clone();
                let text = self.fmt_body_expr(body, value);
                self.line(&format!("Switch {text}"));
                self.indent += 1;
                for arm in &arms {
                    // The `else` arm prints as `else` rather than as an arm with no value, because a
                    // reader of a dump should not have to infer the catch-all from an absence.
                    match arm.value {
                        Some(value) => {
                            let case = self.fmt_body_expr(body, value);
                            self.line(&format!("case {case}"));
                        }
                        None => self.line("else"),
                    }
                    self.indent += 1;
                    self.dump_body_stmt(body, arm.body);
                    self.indent -= 1;
                }
                self.indent -= 1;
            }
            Stmt::Return(expr, _) => {
                let s = expr
                    .map(|e| self.fmt_body_expr(body, e))
                    .unwrap_or_default();
                self.line(&format!("Return {s}"));
            }
            // The label is printed when present, because `break` and `break outer` are
            // different jumps and a dump that hid the difference would be useless for the one
            // thing a labelled break is for.
            Stmt::Break(label, _) => match label {
                Some(name) => self.line(&format!("Break {}", self.sym(*name))),
                None => self.line("Break"),
            },
            Stmt::Continue(label, _) => match label {
                Some(name) => self.line(&format!("Continue {}", self.sym(*name))),
                None => self.line("Continue"),
            },
            Stmt::Error(_) => self.line("Error"),
        }
    }

    fn fmt_top_expr(&self, id: ExprId) -> String {
        fmt_expr_impl(&self.hir.exprs[id.index()], self.interner, true, None)
    }

    fn fmt_body_expr(&self, body: &Body, id: ExprId) -> String {
        fmt_expr_impl(&body.exprs[id.index()], self.interner, false, Some(body))
    }

    fn fmt_type_ref_top(&self, id: TypeRefId) -> String {
        fmt_type_ref_impl(&self.hir.type_refs[id.index()], self.interner, true, None)
    }

    fn fmt_type_ref_body(&self, body: &Body, id: TypeRefId) -> String {
        fmt_type_ref_impl(
            &body.type_refs[id.index()],
            self.interner,
            false,
            Some(body),
        )
    }
}

fn fmt_expr_impl(expr: &Expr, interner: &Interner, is_top: bool, body: Option<&Body>) -> String {
    let sym = |s: jr_base::Symbol| interner.resolve(s).to_owned();
    let sub_expr = |id: ExprId| -> String {
        if is_top {
            // We can't easily recurse without the full context here,
            // so just show the ID for nested exprs in top-level context
            format!("expr#{}", id.index())
        } else if let Some(b) = body {
            fmt_expr_impl(&b.exprs[id.index()], interner, false, Some(b))
        } else {
            format!("expr#{}", id.index())
        }
    };

    match expr {
        Expr::Context(_) => String::from("context"),
        Expr::Literal(lit, _) => fmt_literal(lit),
        Expr::Name { name, res, .. } => {
            let res_str = fmt_res(res);
            format!("{}[{}]", sym(*name), res_str)
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            format!(
                "({} {} {})",
                sub_expr(*lhs),
                fmt_bin_op(*op),
                sub_expr(*rhs)
            )
        }
        Expr::Unary { op, operand, .. } => {
            format!("({} {})", fmt_un_op(*op), sub_expr(*operand))
        }
        Expr::Call { callee, args, .. } => {
            let args_str: Vec<String> = args.iter().map(|a| sub_expr(*a)).collect();
            format!("{}({})", sub_expr(*callee), args_str.join(", "))
        }
        Expr::Field { receiver, name, .. } => {
            format!("{}.{}", sub_expr(*receiver), sym(*name))
        }
        Expr::Index { base, index, .. } => {
            format!("{}[{}]", sub_expr(*base), sub_expr(*index))
        }
        Expr::Slice { base, .. } => format!("{}[]", sub_expr(*base)),
        Expr::Deref(ptr, _) => format!("{}.*", sub_expr(*ptr)),
        Expr::Uninit(_) => "---".to_owned(),
        // The target type is printed as its `TypeRefId`, not resolved: `jr-hir` has no types
        // (that is `jr-sema`'s job), and a dump that resolved one would be claiming knowledge
        // this crate does not have.
        Expr::Cast { ty, operand, .. } => {
            format!("cast(ty{}, {})", ty.index(), sub_expr(*operand))
        }
        Expr::Autocast { operand, .. } => format!("xx {}", sub_expr(*operand)),
        Expr::Member { name, .. } => format!(".{}", sym(*name)),
        Expr::Run(inner, _) => format!("#run {}", sub_expr(*inner)),
        Expr::Directive { name, arg, .. } => {
            let arg_str = arg
                .as_ref()
                .map(|a| format!(" \"{a}\""))
                .unwrap_or_default();
            format!("#{}{}", sym(*name), arg_str)
        }
        Expr::Error(_) => "<error>".to_owned(),
    }
}

fn fmt_type_ref_impl(
    tr: &TypeRef,
    interner: &Interner,
    is_top: bool,
    body: Option<&Body>,
) -> String {
    match tr {
        TypeRef::Name(sym) => interner.resolve(*sym).to_owned(),
        // `$T` (ADR-0081 §1), printed with its `$` so a dump distinguishes a polymorphic variable from an
        // ordinary name.
        TypeRef::Poly(sym) => format!("${}", interner.resolve(*sym)),
        // Printed by *arity* rather than by element, because the elements are `TypeRefId`s into an
        // arena this function may not have (the `is_top` split below shows why), and a snapshot
        // must never carry an index that load order can renumber.
        TypeRef::Results(elems) => format!("({} results)", elems.len()),
        TypeRef::Pointer(inner) => {
            let inner_tr = if is_top {
                // Can't easily recurse without the full context
                format!("type#{}", inner.index())
            } else if let Some(b) = body {
                fmt_type_ref_impl(&b.type_refs[inner.index()], interner, false, Some(b))
            } else {
                format!("type#{}", inner.index())
            };
            format!("*{inner_tr}")
        }
        TypeRef::Array { elem, len, .. } => {
            let elem_tr = if is_top {
                format!("type#{}", elem.index())
            } else if let Some(b) = body {
                fmt_type_ref_impl(&b.type_refs[elem.index()], interner, false, Some(b))
            } else {
                format!("type#{}", elem.index())
            };
            // A length that failed to lower prints as `?` rather than a number, so a dump
            // cannot make a rejected `[COUNT]u8` look like it got a length.
            match len {
                Some(n) => format!("[{n}]{elem_tr}"),
                None => format!("[?]{elem_tr}"),
            }
        }
        TypeRef::View { elem } => {
            let elem_tr = if is_top {
                format!("type#{}", elem.index())
            } else if let Some(b) = body {
                fmt_type_ref_impl(&b.type_refs[elem.index()], interner, false, Some(b))
            } else {
                format!("type#{}", elem.index())
            };
            format!("[]{elem_tr}")
        }
        // Printed by *arity*, like `Results` above and for the same reason: the parameter and
        // return `TypeRefId`s index an arena this function may not have, and a snapshot must not
        // carry an index that load order can renumber.
        TypeRef::Proc { params, .. } => format!("({} params) -> _", params.len()),
        TypeRef::Struct(sid) => format!("struct#{}", sid.index()),
        TypeRef::Union(sid) => format!("union#{}", sid.index()),
        TypeRef::Variant(sid) => format!("variant#{}", sid.index()),
        TypeRef::Enum(eid) => format!("enum#{}", eid.index()),
        TypeRef::Error => "<error>".to_owned(),
    }
}

fn fmt_literal(lit: &Literal) -> String {
    match lit {
        Literal::Int {
            value,
            radix,
            overflowed,
        } => {
            let prefix = match radix {
                16 => "0x",
                2 => "0b",
                8 => "0o",
                _ => "",
            };
            let overflow_mark = if *overflowed { "!" } else { "" };
            format!("{prefix}{value}{overflow_mark}")
        }
        // Decoded from bits for display, the same discipline `jr-mir`'s dump learned for a
        // negative integer (ADR-0038): a dump that prints raw bits hides what the value is.
        // `{:?}` rather than `{}` so that `1.0` does not print as `1` and become
        // indistinguishable from an integer literal in a snapshot.
        Literal::Float { bits, malformed } => {
            let mark = if *malformed { "!" } else { "" };
            format!("{:?}{mark}", f64::from_bits(*bits))
        }
        Literal::Str(s) => format!("{s:?}"),
        Literal::Bool(b) => b.to_string(),
        Literal::Null => "null".to_owned(),
    }
}

fn fmt_res(res: &Res) -> String {
    match res {
        Res::Local(id) => format!("local#{}", id.index()),
        Res::Param(id) => format!("param#{}", id.index()),
        Res::Item(id) => format!("item#{}", id.index()),
        Res::Imported(import_id, _) => format!("imported#{}", import_id.index()),
        // The *path* a promoted name denotes, printed as one, so a snapshot shows which binding
        // supplied the field rather than just that promotion happened. The field name is not
        // resolved to text here because `fmt_res` has no interner; the base carries the identity
        // that matters for reading a snapshot.
        Res::Promoted { base, field: _ } => format!("promoted({})", fmt_res(base)),
        Res::Error => "?".to_owned(),
    }
}

fn fmt_bin_op(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::WrapAdd => "+%",
        BinOp::WrapSub => "-%",
        BinOp::WrapMul => "*%",
        BinOp::BitAnd => "&",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
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

fn fmt_un_op(op: UnOp) -> &'static str {
    match op {
        UnOp::Neg => "-",
        UnOp::Not => "!",
        UnOp::AddrOf => "*",
        UnOp::BitNot => "~",
    }
}

fn fmt_assign_op(op: AssignOp) -> &'static str {
    match op {
        AssignOp::Assign => "=",
        AssignOp::AddAssign => "+=",
        AssignOp::SubAssign => "-=",
        AssignOp::MulAssign => "*=",
        AssignOp::DivAssign => "/=",
        AssignOp::RemAssign => "%=",
        AssignOp::WrapAddAssign => "+%=",
        AssignOp::WrapSubAssign => "-%=",
        AssignOp::WrapMulAssign => "*%=",
        AssignOp::BitAndAssign => "&=",
        AssignOp::BitOrAssign => "|=",
        AssignOp::BitXorAssign => "^=",
        AssignOp::ShlAssign => "<<=",
        AssignOp::ShrAssign => ">>=",
    }
}
