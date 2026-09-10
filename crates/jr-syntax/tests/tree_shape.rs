//! Tree-shape gate: the parser must produce trees `jr-hir` can actually lower.
//!
//! These assertions go through the **typed AST** rather than a text dump,
//! because the typed accessors are the contract every later stage consumes. A
//! text dump can look plausible while `FieldExpr::object()` returns `None`.
//!
//! That is not hypothetical. An earlier version of the parser took a fresh
//! checkpoint for each postfix operator, so `a.b.c` came out as three flat
//! siblings -- `NAME_EXPR(a)`, `FIELD_EXPR(.b)`, `FIELD_EXPR(.c)` -- rather than
//! nested. Every one of the 104 parser unit tests still passed, because none of
//! them ever asked a `FIELD_EXPR` for its receiver. These tests do.

use jr_base::FileId;
use jr_syntax::ast::{AstNode, Expr, Proc, SourceFile, TypeExpr};
use jr_syntax::parse;

/// Parses `X :: <expr>;` and returns the expression.
fn expr_of(source_expr: &str) -> Expr {
    let text = format!("X :: {source_expr};");
    let parsed = parse(&text, FileId::from_usize(0));
    assert!(
        !parsed.has_errors(),
        "test expression {source_expr:?} must parse cleanly, got: {:?}",
        parsed
            .diagnostics()
            .iter()
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
    );

    let file = SourceFile::cast(parsed.syntax()).expect("root is a SourceFile");
    file.syntax()
        .descendants()
        .find_map(|n| {
            jr_syntax::ast::ConstDecl::cast(n)
                .and_then(|d: jr_syntax::ast::ConstDecl| d.value_expr())
        })
        .unwrap_or_else(|| panic!("no value expression parsed for {source_expr:?}"))
}

#[test]
fn named_results_keep_labels_separate_from_their_types() {
    let source = concat!(
        "expect_char :: (s: *string, c: u8) -> (exists: bool) {\n",
        "    todo;\n",
        "}\n",
        "find :: () -> (value: s64, bool) {\n",
        "    todo;\n",
        "}\n",
    );
    let parsed = parse(source, FileId::from_usize(0));
    assert!(
        !parsed.has_errors(),
        "named result declarations must parse cleanly: {:?}",
        parsed.diagnostics()
    );

    let procs: Vec<Proc> = parsed
        .syntax()
        .descendants()
        .filter_map(Proc::cast)
        .collect();
    assert_eq!(procs.len(), 2);

    let expect_results: Vec<_> = procs[0]
        .ret_type()
        .and_then(|ret| ret.result_list())
        .expect("the parenthesised result list")
        .results()
        .collect();
    assert_eq!(expect_results.len(), 1);
    assert_eq!(
        expect_results[0]
            .name_token()
            .map(|token| token.text().to_owned()),
        Some("exists".to_owned())
    );
    assert_eq!(
        expect_results[0]
            .ty()
            .map(|ty| ty.syntax().text().to_string()),
        Some("bool".to_owned())
    );

    let mixed_labels: Vec<_> = procs[1]
        .ret_type()
        .and_then(|ret| ret.result_list())
        .expect("the mixed result list")
        .results()
        .map(|result| result.name_token().map(|token| token.text().to_owned()))
        .collect();
    assert_eq!(mixed_labels, [Some("value".to_owned()), None]);
}

#[test]
fn polymorphic_interface_is_a_child_type_and_interface_stays_contextual() {
    let source = concat!(
        "interface :: struct { value: s64; }\n",
        "read :: (value: *$T/interface Window.Shape) {\n",
        "  todo;\n",
        "}\n",
    );
    let parsed = parse(source, FileId::from_usize(0));
    assert!(
        !parsed.has_errors(),
        "the contextual constraint and an ordinary `interface` identifier must coexist: {:?}",
        parsed.diagnostics()
    );

    let proc = parsed
        .syntax()
        .descendants()
        .find_map(Proc::cast)
        .expect("read procedure");
    let param = proc
        .param_list()
        .expect("parameter list")
        .params()
        .next()
        .expect("value parameter");
    let TypeExpr::Pointer(pointer) = param.ty().expect("pointer parameter type") else {
        panic!("expected the outer pointer");
    };
    let Some(TypeExpr::Poly(poly)) = pointer.pointee() else {
        panic!("expected a polymorphic pointee");
    };
    assert_eq!(
        poly.name_token().map(|token| token.text().to_owned()),
        Some("T".into())
    );
    let Some(TypeExpr::Name(shape)) = poly.interface() else {
        panic!("the interface shape must remain a child type");
    };
    assert_eq!(
        shape.module_token().map(|token| token.text().to_owned()),
        Some("Window".into())
    );
    assert_eq!(
        shape.name_token().map(|token| token.text().to_owned()),
        Some("Shape".into())
    );
}

#[test]
fn malformed_polymorphic_interfaces_report_e0135_losslessly() {
    for source in [
        "read :: (value: $T/Shape) { todo; }\n",
        "read :: (value: $T/interface) { todo; }\n",
    ] {
        let parsed = parse(source, FileId::from_usize(0));
        assert!(
            parsed
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == Some("E0135")),
            "expected E0135 for {source:?}, got {:?}",
            parsed
                .diagnostics()
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>()
        );
        assert_eq!(parsed.syntax().text().to_string(), source);
    }
}

#[test]
fn polymorphic_interfaces_are_refused_outside_procedure_parameters() {
    for source in [
        "Box :: struct($T/interface Shape) { value: T; }\n",
        "bad :: () -> $T/interface Shape { todo; }\n",
    ] {
        let parsed = parse(source, FileId::from_usize(0));
        assert!(
            parsed
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == Some("E0135")),
            "expected E0135 for {source:?}, got {:?}",
            parsed.diagnostics()
        );
        assert_eq!(parsed.syntax().text().to_string(), source);
    }
}

// ---------------------------------------------------------------------------
// Postfix chains must nest, with the receiver inside
// ---------------------------------------------------------------------------

#[test]
fn field_access_nests_left_and_keeps_its_receiver() {
    // a.b.c  ==  (a.b).c
    let Expr::Field(outer) = expr_of("a.b.c") else {
        panic!("expected the outermost node to be a field access");
    };
    assert_eq!(
        outer.field_name().map(|t| t.text().to_owned()),
        Some("c".to_owned())
    );

    let Some(Expr::Field(inner)) = outer.object() else {
        panic!("a.b.c must nest: the receiver of `.c` is the field access `a.b`");
    };
    assert_eq!(
        inner.field_name().map(|t| t.text().to_owned()),
        Some("b".to_owned())
    );

    let Some(Expr::Name(base)) = inner.object() else {
        panic!("the innermost receiver must be the name `a`");
    };
    assert_eq!(base.syntax().text().to_string(), "a");
}

#[test]
fn call_keeps_its_callee() {
    let Expr::Call(call) = expr_of("f(1)") else {
        panic!("expected a call");
    };
    assert!(
        matches!(call.callee(), Some(Expr::Name(_))),
        "a call must contain its callee, not sit beside it"
    );
    assert!(call.arg_list().is_some());
}

#[test]
fn deref_keeps_its_operand() {
    let Expr::Deref(deref) = expr_of("p.*") else {
        panic!("expected a dereference");
    };
    assert!(
        matches!(deref.pointer(), Some(Expr::Name(_))),
        "`.*` must contain the pointer expression"
    );
}

#[test]
fn repeated_deref_nests() {
    // p.*.*  ==  (p.*).*
    let Expr::Deref(outer) = expr_of("p.*.*") else {
        panic!("expected a dereference");
    };
    assert!(matches!(outer.pointer(), Some(Expr::Deref(_))));
}

#[test]
fn mixed_postfix_chain_nests_in_source_order() {
    // f(1)(2).g.*  ==  ((((f(1))(2)).g).*)
    let Expr::Deref(deref) = expr_of("f(1)(2).g.*") else {
        panic!("outermost must be `.*`");
    };
    let Some(Expr::Field(field)) = deref.pointer() else {
        panic!("then `.g`");
    };
    let Some(Expr::Call(outer_call)) = field.object() else {
        panic!("then the `(2)` call");
    };
    let Some(Expr::Call(inner_call)) = outer_call.callee() else {
        panic!("then the `(1)` call");
    };
    assert!(matches!(inner_call.callee(), Some(Expr::Name(_))));
}

// ---------------------------------------------------------------------------
// Struct literals keep their optional type and dedicated entries
// ---------------------------------------------------------------------------

#[test]
fn typed_struct_literal_keeps_its_type_and_named_entries() {
    let Expr::StructLiteral(literal) = expr_of("Point.{y = 2, x = 1,}") else {
        panic!("expected a typed struct literal");
    };
    assert_eq!(
        literal
            .explicit_type()
            .map(|ty| ty.syntax().text().to_string()),
        Some("Point".to_owned())
    );

    let entries: Vec<_> = literal.entries().collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries[0].name_token().map(|token| token.text().to_owned()),
        Some("y".to_owned())
    );
    assert_eq!(
        entries[0]
            .value()
            .map(|value| value.syntax().text().to_string()),
        Some("2".to_owned())
    );
    assert_eq!(
        entries[1].name_token().map(|token| token.text().to_owned()),
        Some("x".to_owned())
    );
}

#[test]
fn inferred_struct_literal_keeps_positional_entries_and_accepts_empty() {
    let Expr::StructLiteral(literal) = expr_of(".{1, 2,}") else {
        panic!("expected an inferred struct literal");
    };
    assert!(literal.explicit_type().is_none());
    let entries: Vec<_> = literal.entries().collect();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|entry| entry.name_token().is_none()));
    assert_eq!(
        entries
            .iter()
            .filter_map(|entry| entry.value())
            .map(|value| value.syntax().text().to_string())
            .collect::<Vec<_>>(),
        ["1", "2"]
    );

    let Expr::StructLiteral(empty) = expr_of(".{}") else {
        panic!("expected an empty inferred struct literal");
    };
    assert!(empty.explicit_type().is_none());
    assert_eq!(empty.entries().count(), 0);
}

#[test]
fn struct_literals_participate_in_postfix_chains() {
    let Expr::Field(field) = expr_of("Point.{x = 1}.x") else {
        panic!("a field access should wrap the completed literal");
    };
    assert!(matches!(field.object(), Some(Expr::StructLiteral(_))));

    let Expr::Index(index) = expr_of(".{1, 2}[0]") else {
        panic!("an index should wrap the completed inferred literal");
    };
    assert!(matches!(index.base(), Some(Expr::StructLiteral(_))));
}

#[test]
fn malformed_struct_literal_entries_report_e0136_losslessly() {
    for source in [
        "X :: .{x =};\n",
        "X :: .{, x = 1};\n",
        "X :: .{x = 1 y = 2};\n",
    ] {
        let parsed = parse(source, FileId::from_usize(0));
        assert!(
            parsed
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == Some("E0136")),
            "expected E0136 for {source:?}, got {:?}",
            parsed.diagnostics()
        );
        assert_eq!(parsed.syntax().text().to_string(), source);
    }
}

// ---------------------------------------------------------------------------
// Prefix vs postfix binding
// ---------------------------------------------------------------------------

#[test]
fn postfix_binds_tighter_than_prefix() {
    // `*p.f` is the address OF THE FIELD -- `*(p.f)` -- not `(*p).f`.
    // Getting this backwards would silently compile to the wrong address.
    let Expr::Unary(unary) = expr_of("*p.f") else {
        panic!("expected address-of at the top");
    };
    assert_eq!(
        unary.op_token().map(|t| t.text().to_owned()),
        Some("*".to_owned())
    );
    assert!(
        matches!(unary.operand(), Some(Expr::Field(_))),
        "`*p.f` must be `*(p.f)`, so the operand of `*` is the field access"
    );
}

#[test]
fn prefix_operators_chain() {
    // !!a  ==  !(!a)
    let Expr::Unary(outer) = expr_of("!!a") else {
        panic!("expected unary");
    };
    assert!(matches!(outer.operand(), Some(Expr::Unary(_))));
}

// ---------------------------------------------------------------------------
// Binary precedence and associativity
// ---------------------------------------------------------------------------

#[test]
fn addition_is_left_associative() {
    // 1 + 2 + 3  ==  (1 + 2) + 3
    let Expr::Binary(outer) = expr_of("1 + 2 + 3") else {
        panic!("expected binary");
    };
    assert!(
        matches!(outer.lhs(), Some(Expr::Binary(_))),
        "left-associative: the nesting must be on the LEFT"
    );
    assert!(matches!(outer.rhs(), Some(Expr::Literal(_))));
}

#[test]
fn multiplication_binds_tighter_than_addition() {
    // 1 + 2 * 3  ==  1 + (2 * 3)
    let Expr::Binary(outer) = expr_of("1 + 2 * 3") else {
        panic!("expected binary");
    };
    assert_eq!(
        outer.op_token().map(|t| t.text().to_owned()),
        Some("+".to_owned())
    );
    assert!(matches!(outer.lhs(), Some(Expr::Literal(_))));
    assert!(matches!(outer.rhs(), Some(Expr::Binary(_))));

    // 1 * 2 + 3  ==  (1 * 2) + 3
    let Expr::Binary(other) = expr_of("1 * 2 + 3") else {
        panic!("expected binary");
    };
    assert_eq!(
        other.op_token().map(|t| t.text().to_owned()),
        Some("+".to_owned())
    );
    assert!(matches!(other.lhs(), Some(Expr::Binary(_))));
}

#[test]
fn full_precedence_ladder() {
    // `||` is loosest, so it must end up outermost:
    //   a || b && c == d + e * f
    let Expr::Binary(or) = expr_of("a || b && c == d + e * f") else {
        panic!("expected binary");
    };
    assert_eq!(
        or.op_token().map(|t| t.text().to_owned()),
        Some("||".to_owned())
    );

    let Some(Expr::Binary(and)) = or.rhs() else {
        panic!("`&&` sits under `||`");
    };
    assert_eq!(
        and.op_token().map(|t| t.text().to_owned()),
        Some("&&".to_owned())
    );

    let Some(Expr::Binary(cmp)) = and.rhs() else {
        panic!("`==` sits under `&&`");
    };
    assert_eq!(
        cmp.op_token().map(|t| t.text().to_owned()),
        Some("==".to_owned())
    );

    let Some(Expr::Binary(add)) = cmp.rhs() else {
        panic!("`+` sits under `==`");
    };
    assert_eq!(
        add.op_token().map(|t| t.text().to_owned()),
        Some("+".to_owned())
    );

    let Some(Expr::Binary(mul)) = add.rhs() else {
        panic!("`*` sits under `+`, being the tightest");
    };
    assert_eq!(
        mul.op_token().map(|t| t.text().to_owned()),
        Some("*".to_owned())
    );
}

#[test]
fn wrapping_operators_share_precedence_with_their_trapping_forms() {
    // `+%` must bind exactly like `+`, or ADR-0002's wrapping operators would
    // silently reassociate arithmetic.
    let Expr::Binary(outer) = expr_of("1 +% 2 *% 3") else {
        panic!("expected binary");
    };
    assert_eq!(
        outer.op_token().map(|t| t.text().to_owned()),
        Some("+%".to_owned())
    );
    assert!(matches!(outer.rhs(), Some(Expr::Binary(_))));
}

#[test]
fn parentheses_override_precedence() {
    // (1 + 2) * 3
    let Expr::Binary(outer) = expr_of("(1 + 2) * 3") else {
        panic!("expected binary");
    };
    assert_eq!(
        outer.op_token().map(|t| t.text().to_owned()),
        Some("*".to_owned())
    );
    assert!(
        matches!(outer.lhs(), Some(Expr::Paren(_))),
        "the parenthesised sum must be the left operand of `*`"
    );
}

#[test]
fn unary_binds_tighter_than_binary() {
    // -a + b  ==  (-a) + b
    let Expr::Binary(outer) = expr_of("-a + b") else {
        panic!("expected binary at the top");
    };
    assert!(matches!(outer.lhs(), Some(Expr::Unary(_))));
}

#[test]
fn comparison_is_left_associative() {
    let Expr::Binary(outer) = expr_of("a < b < c") else {
        panic!("expected binary");
    };
    assert!(matches!(outer.lhs(), Some(Expr::Binary(_))));
}

// ---------------------------------------------------------------------------
// #run wraps the whole call, not just the callee name
// ---------------------------------------------------------------------------

#[test]
fn run_directive_wraps_the_entire_call() {
    // `#run add(2, 3)` must be RUN_EXPR(CALL_EXPR(add, (2,3))), not
    // RUN_EXPR(add) followed by a stray call.
    let Expr::Run(run) = expr_of("#run add(2, 3)") else {
        panic!("expected a #run expression");
    };
    let inner = run
        .syntax()
        .descendants()
        .filter_map(Expr::cast)
        .find(|e| matches!(e, Expr::Call(_)));
    assert!(
        inner.is_some(),
        "#run must contain the whole call expression"
    );
    let Some(Expr::Call(call)) = inner else {
        unreachable!()
    };
    assert!(
        matches!(call.callee(), Some(Expr::Name(_))),
        "and that call must own its callee"
    );
}
