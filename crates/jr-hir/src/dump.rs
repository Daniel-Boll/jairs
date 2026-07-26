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
    AssignOp, BinOp, Body, BodyId, ConstValue, Expr, ExprId, FileHir, ItemKind, Literal, Res, Stmt,
    StmtId, TypeRef, TypeRefId, UnOp,
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
                    ConstValue::Struct(sid) => {
                        let s = &self.hir.structs[sid.index()];
                        self.line(&format!("Item[{i}] Const {name} :: Struct {{"));
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
            Stmt::Return(expr, _) => {
                let s = expr
                    .map(|e| self.fmt_body_expr(body, e))
                    .unwrap_or_default();
                self.line(&format!("Return {s}"));
            }
            Stmt::Break(_) => self.line("Break"),
            Stmt::Continue(_) => self.line("Continue"),
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
        Expr::Literal(lit, _) => fmt_literal(lit),
        Expr::Name { name, res, .. } => {
            let res_str = fmt_res(*res);
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
        Expr::Deref(ptr, _) => format!("{}.*", sub_expr(*ptr)),
        Expr::Uninit(_) => "---".to_owned(),
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
        TypeRef::Struct(sid) => format!("struct#{}", sid.index()),
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
        Literal::Str(s) => format!("{s:?}"),
        Literal::Bool(b) => b.to_string(),
    }
}

fn fmt_res(res: Res) -> String {
    match res {
        Res::Local(id) => format!("local#{}", id.index()),
        Res::Param(id) => format!("param#{}", id.index()),
        Res::Item(id) => format!("item#{}", id.index()),
        Res::Imported(import_id, _) => format!("imported#{}", import_id.index()),
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
    }
}
