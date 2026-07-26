//! Unit tests for HIR lowering and resolution.

use jr_base::{FileId, Interner};
use jr_hir::{
    BinOp, ConstValue, Expr, ItemKind, Literal, Res, UnOp, dump::dump_hir, lower_file, resolve,
};
use jr_syntax::parse;

fn file() -> FileId {
    FileId::from_usize(0)
}

fn lower(source: &str) -> (jr_hir::FileHir, jr_diag::Diagnostics, Interner) {
    let interner = Interner::new();
    let f = file();
    let parsed = parse(source, f);
    let (hir, diags) = lower_file(&parsed, f, &interner);
    (hir, diags, interner)
}

// ---------------------------------------------------------------------------
// Literal decoding
// ---------------------------------------------------------------------------

#[test]
fn decimal_integer_literal() {
    let (hir, diags, _) = lower("X :: 42;");
    assert!(diags.is_empty());
    let item = &hir.items[0];
    let ItemKind::Const {
        value: ConstValue::Expr(eid),
    } = &item.kind
    else {
        panic!("expected const expr");
    };
    let Expr::Literal(
        Literal::Int {
            value,
            radix,
            overflowed,
        },
        _,
    ) = &hir.exprs[eid.index()]
    else {
        panic!("expected int literal");
    };
    assert_eq!(*value, 42);
    assert_eq!(*radix, 10);
    assert!(!overflowed);
}

#[test]
fn hex_integer_literal() {
    let (hir, diags, _) = lower("X :: 0xdead_beef;");
    assert!(diags.is_empty());
    let item = &hir.items[0];
    let ItemKind::Const {
        value: ConstValue::Expr(eid),
    } = &item.kind
    else {
        panic!("expected const expr");
    };
    let Expr::Literal(
        Literal::Int {
            value,
            radix,
            overflowed,
        },
        _,
    ) = &hir.exprs[eid.index()]
    else {
        panic!("expected int literal");
    };
    assert_eq!(*value, 0xdead_beef);
    assert_eq!(*radix, 16);
    assert!(!overflowed);
}

#[test]
fn binary_integer_literal() {
    let (hir, diags, _) = lower("X :: 0b1010_1010;");
    assert!(diags.is_empty());
    let item = &hir.items[0];
    let ItemKind::Const {
        value: ConstValue::Expr(eid),
    } = &item.kind
    else {
        panic!("expected const expr");
    };
    let Expr::Literal(Literal::Int { value, radix, .. }, _) = &hir.exprs[eid.index()] else {
        panic!("expected int literal");
    };
    assert_eq!(*value, 0b1010_1010);
    assert_eq!(*radix, 2);
}

#[test]
fn octal_integer_literal() {
    let (hir, diags, _) = lower("X :: 0o755;");
    assert!(diags.is_empty());
    let item = &hir.items[0];
    let ItemKind::Const {
        value: ConstValue::Expr(eid),
    } = &item.kind
    else {
        panic!("expected const expr");
    };
    let Expr::Literal(Literal::Int { value, radix, .. }, _) = &hir.exprs[eid.index()] else {
        panic!("expected int literal");
    };
    assert_eq!(*value, 0o755);
    assert_eq!(*radix, 8);
}

#[test]
fn underscore_separator_in_integer() {
    let (hir, diags, _) = lower("X :: 1_000_000;");
    assert!(diags.is_empty());
    let item = &hir.items[0];
    let ItemKind::Const {
        value: ConstValue::Expr(eid),
    } = &item.kind
    else {
        panic!("expected const expr");
    };
    let Expr::Literal(Literal::Int { value, .. }, _) = &hir.exprs[eid.index()] else {
        panic!("expected int literal");
    };
    assert_eq!(*value, 1_000_000);
}

#[test]
fn max_s64_integer_literal() {
    let (hir, diags, _) = lower("X :: 9223372036854775807;");
    assert!(diags.is_empty());
    let item = &hir.items[0];
    let ItemKind::Const {
        value: ConstValue::Expr(eid),
    } = &item.kind
    else {
        panic!("expected const expr");
    };
    let Expr::Literal(
        Literal::Int {
            value, overflowed, ..
        },
        _,
    ) = &hir.exprs[eid.index()]
    else {
        panic!("expected int literal");
    };
    assert_eq!(*value, i64::MAX as u64);
    assert!(!overflowed);
}

#[test]
fn overflow_s64_integer_literal_emits_diagnostic() {
    // 9223372036854775808 = i64::MAX + 1
    let (hir, diags, _) = lower("X :: 9223372036854775808;");
    assert_eq!(diags.len(), 1, "expected exactly one overflow diagnostic");
    assert!(diags.iter().any(|d| d.code == Some("E0204")));
    let item = &hir.items[0];
    let ItemKind::Const {
        value: ConstValue::Expr(eid),
    } = &item.kind
    else {
        panic!("expected const expr");
    };
    let Expr::Literal(Literal::Int { overflowed, .. }, _) = &hir.exprs[eid.index()] else {
        panic!("expected int literal");
    };
    assert!(overflowed);
}

#[test]
fn string_literal_plain() {
    let (hir, diags, _) = lower(r#"X :: "simple";"#);
    assert!(diags.is_empty());
    let item = &hir.items[0];
    let ItemKind::Const {
        value: ConstValue::Expr(eid),
    } = &item.kind
    else {
        panic!("expected const expr");
    };
    let Expr::Literal(Literal::Str(s), _) = &hir.exprs[eid.index()] else {
        panic!("expected string literal");
    };
    assert_eq!(s, "simple");
}

#[test]
fn string_literal_escape_sequences() {
    let (hir, diags, _) = lower(r#"X :: "tab:\there\nnewline";"#);
    assert!(diags.is_empty());
    let item = &hir.items[0];
    let ItemKind::Const {
        value: ConstValue::Expr(eid),
    } = &item.kind
    else {
        panic!("expected const expr");
    };
    let Expr::Literal(Literal::Str(s), _) = &hir.exprs[eid.index()] else {
        panic!("expected string literal");
    };
    assert_eq!(s, "tab:\there\nnewline");
}

#[test]
fn string_literal_unicode_escape() {
    let (hir, diags, _) = lower(r#"X :: "caf\u00e9";"#);
    assert!(diags.is_empty());
    let item = &hir.items[0];
    let ItemKind::Const {
        value: ConstValue::Expr(eid),
    } = &item.kind
    else {
        panic!("expected const expr");
    };
    let Expr::Literal(Literal::Str(s), _) = &hir.exprs[eid.index()] else {
        panic!("expected string literal");
    };
    assert_eq!(s, "café");
}

#[test]
fn string_literal_all_escapes() {
    // \n \r \t \0 \\ \"
    let (hir, diags, _) = lower(r#"X :: "\n\r\t\0\\\"";"#);
    assert!(diags.is_empty());
    let item = &hir.items[0];
    let ItemKind::Const {
        value: ConstValue::Expr(eid),
    } = &item.kind
    else {
        panic!("expected const expr");
    };
    let Expr::Literal(Literal::Str(s), _) = &hir.exprs[eid.index()] else {
        panic!("expected string literal");
    };
    assert_eq!(s, "\n\r\t\0\\\"");
}

#[test]
fn unknown_escape_emits_diagnostic() {
    let (_, diags, _) = lower(r#"X :: "\q";"#);
    assert!(diags.iter().any(|d| d.code == Some("E0205")));
}

#[test]
fn invalid_unicode_escape_emits_diagnostic() {
    // \u with only 2 hex digits
    let (_, diags, _) = lower(r#"X :: "\u00";"#);
    assert!(diags.iter().any(|d| d.code == Some("E0206")));
}

// ---------------------------------------------------------------------------
// Operators
// ---------------------------------------------------------------------------

#[test]
fn wrapping_operators_are_distinct_from_trapping() {
    let (hir, diags, _) = lower("X :: 1 +% 2;");
    assert!(diags.is_empty());
    let item = &hir.items[0];
    let ItemKind::Const {
        value: ConstValue::Expr(eid),
    } = &item.kind
    else {
        panic!("expected const expr");
    };
    let Expr::Binary { op, .. } = &hir.exprs[eid.index()] else {
        panic!("expected binary expr");
    };
    assert_eq!(
        *op,
        BinOp::WrapAdd,
        "wrapping add must be distinct from trapping add"
    );
}

#[test]
fn trapping_add_is_distinct_from_wrapping() {
    let (hir, diags, _) = lower("X :: 1 + 2;");
    assert!(diags.is_empty());
    let item = &hir.items[0];
    let ItemKind::Const {
        value: ConstValue::Expr(eid),
    } = &item.kind
    else {
        panic!("expected const expr");
    };
    let Expr::Binary { op, .. } = &hir.exprs[eid.index()] else {
        panic!("expected binary expr");
    };
    assert_eq!(*op, BinOp::Add);
}

#[test]
fn address_of_is_unary_addr_of() {
    let (hir, diags, _) = lower("main :: () { x := 1; p := *x; }");
    assert!(diags.is_empty());
    // Find the address-of expression in the body
    let proc_id = match &hir.items[0].kind {
        ItemKind::Const {
            value: ConstValue::Proc(pid),
        } => *pid,
        _ => panic!("expected proc"),
    };
    let body_id = hir.procs[proc_id.index()].body.unwrap();
    let body = &hir.bodies[body_id.index()];
    // Find the `*x` expression
    let found = body.exprs.iter().any(|e| {
        matches!(
            e,
            Expr::Unary {
                op: UnOp::AddrOf,
                ..
            }
        )
    });
    assert!(found, "expected address-of expression");
}

#[test]
fn paren_expr_is_dropped() {
    // (1 + 2) should lower to just the binary expr, not a paren wrapper
    let (hir, diags, _) = lower("X :: (1 + 2);");
    assert!(diags.is_empty());
    let item = &hir.items[0];
    let ItemKind::Const {
        value: ConstValue::Expr(eid),
    } = &item.kind
    else {
        panic!("expected const expr");
    };
    // The top-level expression should be Binary, not Paren
    let expr = &hir.exprs[eid.index()];
    assert!(
        matches!(expr, Expr::Binary { .. }),
        "PAREN_EXPR must be dropped; got: {expr:?}"
    );
}

// ---------------------------------------------------------------------------
// Procedures
// ---------------------------------------------------------------------------

#[test]
fn proc_with_params_and_return() {
    let (hir, diags, interner) = lower("add :: (a: s64, b: s64) -> s64 { return a + b; }");
    assert!(diags.is_empty());
    let item = &hir.items[0];
    let ItemKind::Const {
        value: ConstValue::Proc(pid),
    } = &item.kind
    else {
        panic!("expected proc");
    };
    let proc = &hir.procs[pid.index()];
    assert_eq!(proc.params.len(), 2);
    assert_eq!(interner.resolve(proc.params[0].name), "a");
    assert_eq!(interner.resolve(proc.params[1].name), "b");
    assert!(proc.ret.is_some());
    assert!(proc.body.is_some());
    assert!(proc.foreign.is_none());
}

#[test]
fn foreign_proc_has_no_body() {
    let (hir, diags, interner) = lower(
        r#"libc :: #system_library "c";
write :: (fd: s64, buf: *u8, count: s64) -> s64 #foreign libc "write";"#,
    );
    assert!(diags.is_empty());
    // Find the write proc
    let write_item = hir
        .items
        .iter()
        .find(|i| {
            i.name
                .map(|s| interner.resolve(s) == "write")
                .unwrap_or(false)
        })
        .expect("write item");
    let ItemKind::Const {
        value: ConstValue::Proc(pid),
    } = &write_item.kind
    else {
        panic!("expected proc");
    };
    let proc = &hir.procs[pid.index()];
    assert!(proc.body.is_none());
    assert!(proc.foreign.is_some());
    let foreign = proc.foreign.as_ref().unwrap();
    assert_eq!(foreign.symbol.as_deref(), Some("write"));
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

#[test]
fn struct_with_fields() {
    let (hir, diags, interner) = lower("Point :: struct { x: s64; y: s64; }");
    assert!(diags.is_empty());
    let item = &hir.items[0];
    let ItemKind::Const {
        value: ConstValue::Struct(sid),
    } = &item.kind
    else {
        panic!("expected struct");
    };
    let s = &hir.structs[sid.index()];
    assert_eq!(s.fields.len(), 2);
    assert_eq!(interner.resolve(s.fields[0].name), "x");
    assert_eq!(interner.resolve(s.fields[1].name), "y");
}

// ---------------------------------------------------------------------------
// Name resolution
// ---------------------------------------------------------------------------

#[test]
fn file_level_name_resolves_to_item() {
    let source = "MAX :: 42;\nX :: MAX;";
    let interner = Interner::new();
    let f = file();
    let parsed = parse(source, f);
    let (hir, lower_diags) = lower_file(&parsed, f, &interner);
    assert!(lower_diags.is_empty());
    let (resolve_map, resolve_diags) = resolve(&hir, &[], &interner);
    assert!(
        resolve_diags.is_empty(),
        "unexpected: {:?}",
        resolve_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );

    // Find the ExprId for `MAX` in `X :: MAX`
    let x_item = hir
        .items
        .iter()
        .find(|i| i.name.map(|s| interner.resolve(s) == "X").unwrap_or(false))
        .expect("X item");
    let ItemKind::Const {
        value: ConstValue::Expr(eid),
    } = &x_item.kind
    else {
        panic!("expected const expr");
    };
    let res = resolve_map.get(*eid);
    assert!(
        matches!(res, Some(Res::Item(_))),
        "MAX should resolve to an item, got: {res:?}"
    );
}

#[test]
fn local_variable_resolves_to_local() {
    let source = "main :: () { x := 1; y := x; }";
    let interner = Interner::new();
    let f = file();
    let parsed = parse(source, f);
    let (hir, lower_diags) = lower_file(&parsed, f, &interner);
    assert!(lower_diags.is_empty());
    let (resolve_map, resolve_diags) = resolve(&hir, &[], &interner);
    assert!(
        resolve_diags.is_empty(),
        "unexpected: {:?}",
        resolve_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );

    // Find the `x` name expr in `y := x`
    let proc_id = match &hir.items[0].kind {
        ItemKind::Const {
            value: ConstValue::Proc(pid),
        } => *pid,
        _ => panic!("expected proc"),
    };
    let body_id = hir.procs[proc_id.index()].body.unwrap();
    let body = &hir.bodies[body_id.index()];

    // Find the name expr for `x` in the init of `y`
    let x_name_expr = body.exprs.iter().enumerate().find(|(_, e)| {
        if let Expr::Name { name, .. } = e {
            interner.resolve(*name) == "x"
        } else {
            false
        }
    });
    let Some((idx, _)) = x_name_expr else {
        panic!("could not find `x` name expr");
    };
    let eid = jr_hir::ExprId::from_usize(idx);
    let res = resolve_map.get(eid);
    assert!(
        matches!(res, Some(Res::Local(_))),
        "`x` should resolve to a local, got: {res:?}"
    );
}

#[test]
fn param_resolves_to_param() {
    let source = "f :: (a: s64) -> s64 { return a; }";
    let interner = Interner::new();
    let f = file();
    let parsed = parse(source, f);
    let (hir, lower_diags) = lower_file(&parsed, f, &interner);
    assert!(lower_diags.is_empty());
    let (resolve_map, resolve_diags) = resolve(&hir, &[], &interner);
    assert!(
        resolve_diags.is_empty(),
        "unexpected: {:?}",
        resolve_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );

    let proc_id = match &hir.items[0].kind {
        ItemKind::Const {
            value: ConstValue::Proc(pid),
        } => *pid,
        _ => panic!("expected proc"),
    };
    let body_id = hir.procs[proc_id.index()].body.unwrap();
    let body = &hir.bodies[body_id.index()];

    let a_expr = body.exprs.iter().enumerate().find(|(_, e)| {
        if let Expr::Name { name, .. } = e {
            interner.resolve(*name) == "a"
        } else {
            false
        }
    });
    let Some((idx, _)) = a_expr else {
        panic!("could not find `a` name expr");
    };
    let eid = jr_hir::ExprId::from_usize(idx);
    let res = resolve_map.get(eid);
    assert!(
        matches!(res, Some(Res::Param(_))),
        "`a` should resolve to a param, got: {res:?}"
    );
}

#[test]
fn duplicate_declaration_emits_e0200() {
    let source = "X :: 1;\nX :: 2;";
    let interner = Interner::new();
    let f = file();
    let parsed = parse(source, f);
    let (hir, lower_diags) = lower_file(&parsed, f, &interner);
    assert!(lower_diags.is_empty());
    let (_, resolve_diags) = resolve(&hir, &[], &interner);
    assert!(
        resolve_diags.iter().any(|d| d.code == Some("E0200")),
        "expected E0200 for duplicate declaration, got: {:?}",
        resolve_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn unresolved_name_emits_e0201() {
    let source = "X :: UNDEFINED;";
    let interner = Interner::new();
    let f = file();
    let parsed = parse(source, f);
    let (hir, lower_diags) = lower_file(&parsed, f, &interner);
    assert!(lower_diags.is_empty());
    let (_, resolve_diags) = resolve(&hir, &[], &interner);
    assert!(
        resolve_diags.iter().any(|d| d.code == Some("E0201")),
        "expected E0201 for unresolved name, got: {:?}",
        resolve_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn order_independent_resolution() {
    // LIMIT refers to MAX_ENTITIES which is declared AFTER it
    let source = "LIMIT :: MAX_ENTITIES;\nMAX_ENTITIES :: 4096;";
    let interner = Interner::new();
    let f = file();
    let parsed = parse(source, f);
    let (hir, lower_diags) = lower_file(&parsed, f, &interner);
    assert!(lower_diags.is_empty());
    let (_, resolve_diags) = resolve(&hir, &[], &interner);
    assert!(
        resolve_diags.is_empty(),
        "order-independent resolution failed: {:?}",
        resolve_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn shadowing_in_nested_blocks_is_allowed() {
    let source = r#"
main :: () {
    x := 1;
    {
        x := 2;
    }
}
"#;
    let interner = Interner::new();
    let f = file();
    let parsed = parse(source, f);
    let (hir, lower_diags) = lower_file(&parsed, f, &interner);
    assert!(lower_diags.is_empty());
    let (_, resolve_diags) = resolve(&hir, &[], &interner);
    // Shadowing is allowed; no diagnostic should be emitted
    assert!(
        resolve_diags.is_empty(),
        "shadowing should not produce diagnostics: {:?}",
        resolve_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

#[test]
fn import_decl_is_recorded() {
    let (hir, diags, _interner) = lower(r#"#import "Basic";"#);
    assert!(diags.is_empty());
    let import_item = hir
        .items
        .iter()
        .find(|i| matches!(&i.kind, ItemKind::Import { .. }));
    assert!(import_item.is_some(), "expected import item");
    let ItemKind::Import { path, .. } = &import_item.unwrap().kind else {
        panic!("expected import kind");
    };
    assert_eq!(path, "Basic");
}

// ---------------------------------------------------------------------------
// Import resolution — unit tests (ADR-0014 §2, §3, §6)
// ---------------------------------------------------------------------------

/// Build a minimal `ItemScope` from a list of name strings.
///
/// Uses `FileId(0)` for all spans; the exact span values do not matter for
/// resolution unit tests.
fn make_scope(names: &[&str], interner: &Interner) -> jr_hir::ItemScope {
    let mut scope = jr_hir::ItemScope::new();
    for (i, name) in names.iter().enumerate() {
        let sym = interner.intern(name);
        scope.insert(sym, jr_hir::ItemId::from_usize(i));
    }
    scope
}

#[test]
fn name_resolves_through_single_import() {
    // `print` is in the imported scope; the file itself does not define it.
    let source = r#"#import "Basic";
main :: () {
    print("hello");
}
"#;
    let interner = Interner::new();
    let f = file();
    let parsed = parse(source, f);
    let (hir, lower_diags) = lower_file(&parsed, f, &interner);
    assert!(lower_diags.is_empty());

    let basic_scope = make_scope(&["print"], &interner);
    let (resolve_map, resolve_diags) = resolve(&hir, &[("Basic", &basic_scope)], &interner);
    assert!(
        resolve_diags.is_empty(),
        "unexpected diagnostics: {:?}",
        resolve_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );

    // Find the `print` name expr in the body
    let proc_id = match &hir.items[1].kind {
        ItemKind::Const {
            value: ConstValue::Proc(pid),
        } => *pid,
        _ => panic!("expected proc at item[1]"),
    };
    let body_id = hir.procs[proc_id.index()].body.unwrap();
    let body = &hir.bodies[body_id.index()];
    let print_expr = body.exprs.iter().enumerate().find(|(_, e)| {
        if let Expr::Name { name, .. } = e {
            interner.resolve(*name) == "print"
        } else {
            false
        }
    });
    let Some((idx, _)) = print_expr else {
        panic!("could not find `print` name expr");
    };
    let eid = jr_hir::ExprId::from_usize(idx);
    let res = resolve_map.get(eid);
    assert!(
        matches!(res, Some(Res::Imported(_, _))),
        "`print` should resolve to Imported, got: {res:?}"
    );
}

#[test]
fn name_resolves_through_second_of_two_imports() {
    // `area` is in the second imported scope.
    let source = r#"#import "Colors";
#import "Shapes";
main :: () {
    a := area();
}
"#;
    let interner = Interner::new();
    let f = file();
    let parsed = parse(source, f);
    let (hir, lower_diags) = lower_file(&parsed, f, &interner);
    assert!(lower_diags.is_empty());

    let colors_scope = make_scope(&["blend", "BLACK"], &interner);
    let shapes_scope = make_scope(&["area", "Rect"], &interner);
    let (resolve_map, resolve_diags) = resolve(
        &hir,
        &[("Colors", &colors_scope), ("Shapes", &shapes_scope)],
        &interner,
    );
    assert!(
        resolve_diags.is_empty(),
        "unexpected diagnostics: {:?}",
        resolve_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );

    // Find the `area` name expr
    let proc_id = match &hir.items[2].kind {
        ItemKind::Const {
            value: ConstValue::Proc(pid),
        } => *pid,
        _ => panic!("expected proc at item[2]"),
    };
    let body_id = hir.procs[proc_id.index()].body.unwrap();
    let body = &hir.bodies[body_id.index()];
    let area_expr = body.exprs.iter().enumerate().find(|(_, e)| {
        if let Expr::Name { name, .. } = e {
            interner.resolve(*name) == "area"
        } else {
            false
        }
    });
    let Some((idx, _)) = area_expr else {
        panic!("could not find `area` name expr");
    };
    let eid = jr_hir::ExprId::from_usize(idx);
    let res = resolve_map.get(eid);
    assert!(
        matches!(res, Some(Res::Imported(_, _))),
        "`area` should resolve to Imported, got: {res:?}"
    );
}

#[test]
fn local_file_declaration_shadows_imported_name() {
    // `blend` is defined locally AND in the imported scope.
    // The local definition must win (ADR-0014 §3).
    let source = r#"#import "Colors";
blend :: (a: s64, b: s64) -> s64 {
    return a - b;
}
main :: () {
    x := blend(10, 4);
}
"#;
    let interner = Interner::new();
    let f = file();
    let parsed = parse(source, f);
    let (hir, lower_diags) = lower_file(&parsed, f, &interner);
    assert!(lower_diags.is_empty());

    let colors_scope = make_scope(&["blend", "BLACK"], &interner);
    let (resolve_map, resolve_diags) = resolve(&hir, &[("Colors", &colors_scope)], &interner);
    assert!(
        resolve_diags.is_empty(),
        "unexpected diagnostics: {:?}",
        resolve_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );

    // Find the `blend` call in main's body
    let main_item = hir
        .items
        .iter()
        .find(|i| {
            i.name
                .map(|s| interner.resolve(s) == "main")
                .unwrap_or(false)
        })
        .expect("main item");
    let proc_id = match &main_item.kind {
        ItemKind::Const {
            value: ConstValue::Proc(pid),
        } => *pid,
        _ => panic!("expected proc"),
    };
    let body_id = hir.procs[proc_id.index()].body.unwrap();
    let body = &hir.bodies[body_id.index()];
    let blend_expr = body.exprs.iter().enumerate().find(|(_, e)| {
        if let Expr::Name { name, .. } = e {
            interner.resolve(*name) == "blend"
        } else {
            false
        }
    });
    let Some((idx, _)) = blend_expr else {
        panic!("could not find `blend` name expr");
    };
    let eid = jr_hir::ExprId::from_usize(idx);
    let res = resolve_map.get(eid);
    // Must resolve to Item (the local file-level `blend`), not Imported.
    assert!(
        matches!(res, Some(Res::Item(_))),
        "`blend` should resolve to local Item (shadowing import), got: {res:?}"
    );
}

#[test]
fn duplicate_import_of_same_module_is_idempotent() {
    // Importing the same module twice must not cause ambiguity (ADR-0014 §6).
    let source = r#"#import "Colors";
#import "Colors";
main :: () {
    x := blend(1, 2);
}
"#;
    let interner = Interner::new();
    let f = file();
    let parsed = parse(source, f);
    let (hir, lower_diags) = lower_file(&parsed, f, &interner);
    assert!(lower_diags.is_empty());

    let colors_scope = make_scope(&["blend", "BLACK"], &interner);
    // Pass the same scope twice (same module name).
    let (resolve_map, resolve_diags) = resolve(
        &hir,
        &[("Colors", &colors_scope), ("Colors", &colors_scope)],
        &interner,
    );
    assert!(
        resolve_diags.is_empty(),
        "duplicate import must not cause ambiguity: {:?}",
        resolve_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );

    // `blend` must still resolve.
    let proc_id = match &hir.items[2].kind {
        ItemKind::Const {
            value: ConstValue::Proc(pid),
        } => *pid,
        _ => panic!("expected proc at item[2]"),
    };
    let body_id = hir.procs[proc_id.index()].body.unwrap();
    let body = &hir.bodies[body_id.index()];
    let blend_expr = body.exprs.iter().enumerate().find(|(_, e)| {
        if let Expr::Name { name, .. } = e {
            interner.resolve(*name) == "blend"
        } else {
            false
        }
    });
    let Some((idx, _)) = blend_expr else {
        panic!("could not find `blend` name expr");
    };
    let eid = jr_hir::ExprId::from_usize(idx);
    let res = resolve_map.get(eid);
    assert!(
        matches!(res, Some(Res::Imported(_, _))),
        "`blend` should resolve to Imported, got: {res:?}"
    );
}

#[test]
fn two_modules_same_name_unused_is_not_an_error() {
    // Both Colors and Palette export `blend`. As long as `blend` is never
    // used, there must be zero diagnostics (ADR-0014 §3).
    let source = r#"#import "Colors";
#import "Palette";
main :: () {
    x := BLACK;
}
"#;
    let interner = Interner::new();
    let f = file();
    let parsed = parse(source, f);
    let (hir, lower_diags) = lower_file(&parsed, f, &interner);
    assert!(lower_diags.is_empty());

    let colors_scope = make_scope(&["blend", "BLACK"], &interner);
    let palette_scope = make_scope(&["blend", "PALETTE_SIZE"], &interner);
    let (_, resolve_diags) = resolve(
        &hir,
        &[("Colors", &colors_scope), ("Palette", &palette_scope)],
        &interner,
    );
    assert!(
        resolve_diags.is_empty(),
        "unused ambiguous name must not produce diagnostics: {:?}",
        resolve_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn two_modules_same_name_used_is_e0211() {
    // Both Colors and Palette export `blend`. Using `blend` must be E0211.
    // Other names (BLACK, PALETTE_SIZE) must still resolve (ADR-0014 §3).
    let source = r#"#import "Colors";
#import "Palette";
main :: () {
    bad := blend(1, 2);
    ok_a := BLACK;
    ok_b := PALETTE_SIZE;
}
"#;
    let interner = Interner::new();
    let f = file();
    let parsed = parse(source, f);
    let (hir, lower_diags) = lower_file(&parsed, f, &interner);
    assert!(lower_diags.is_empty());

    let colors_scope = make_scope(&["blend", "BLACK"], &interner);
    let palette_scope = make_scope(&["blend", "PALETTE_SIZE"], &interner);
    let (resolve_map, resolve_diags) = resolve(
        &hir,
        &[("Colors", &colors_scope), ("Palette", &palette_scope)],
        &interner,
    );

    // Exactly one E0211 for `blend`.
    let e0211_count = resolve_diags
        .iter()
        .filter(|d| d.code == Some("E0211"))
        .count();
    assert_eq!(
        e0211_count, 1,
        "expected exactly one E0211, got: {e0211_count}"
    );

    // No E0201 (unresolved) for BLACK or PALETTE_SIZE.
    let e0201_msgs: Vec<_> = resolve_diags
        .iter()
        .filter(|d| d.code == Some("E0201"))
        .map(|d| d.message.clone())
        .collect();
    assert!(
        e0201_msgs.is_empty(),
        "BLACK and PALETTE_SIZE must still resolve; got E0201: {e0201_msgs:?}"
    );

    // The E0211 diagnostic must mention both module names.
    let e0211_diag = resolve_diags
        .iter()
        .find(|d| d.code == Some("E0211"))
        .unwrap();
    assert!(
        e0211_diag.message.contains("Colors"),
        "E0211 must name `Colors`: {}",
        e0211_diag.message
    );
    assert!(
        e0211_diag.message.contains("Palette"),
        "E0211 must name `Palette`: {}",
        e0211_diag.message
    );

    // `blend` must resolve to Error; BLACK and PALETTE_SIZE must resolve.
    let proc_id = match &hir.items[2].kind {
        ItemKind::Const {
            value: ConstValue::Proc(pid),
        } => *pid,
        _ => panic!("expected proc at item[2]"),
    };
    let body_id = hir.procs[proc_id.index()].body.unwrap();
    let body = &hir.bodies[body_id.index()];

    let mut blend_res = None;
    let mut black_res = None;
    let mut palette_size_res = None;
    for (idx, e) in body.exprs.iter().enumerate() {
        if let Expr::Name { name, .. } = e {
            let text = interner.resolve(*name);
            let eid = jr_hir::ExprId::from_usize(idx);
            match text {
                "blend" => blend_res = resolve_map.get(eid),
                "BLACK" => black_res = resolve_map.get(eid),
                "PALETTE_SIZE" => palette_size_res = resolve_map.get(eid),
                _ => {}
            }
        }
    }
    assert_eq!(blend_res, Some(Res::Error), "`blend` must be Res::Error");
    assert!(
        matches!(black_res, Some(Res::Imported(_, _))),
        "`BLACK` must resolve to Imported, got: {black_res:?}"
    );
    assert!(
        matches!(palette_size_res, Some(Res::Imported(_, _))),
        "`PALETTE_SIZE` must resolve to Imported, got: {palette_size_res:?}"
    );
}

#[test]
fn unknown_name_in_file_with_imports_is_e0201() {
    // Even with imports present, a name that is not in any scope is E0201.
    // This proves the old suppression is gone.
    let source = r#"#import "Colors";
main :: () {
    x := blend(1, 2);
    y := not_exported;
}
"#;
    let interner = Interner::new();
    let f = file();
    let parsed = parse(source, f);
    let (hir, lower_diags) = lower_file(&parsed, f, &interner);
    assert!(lower_diags.is_empty());

    let colors_scope = make_scope(&["blend", "BLACK"], &interner);
    let (_, resolve_diags) = resolve(&hir, &[("Colors", &colors_scope)], &interner);

    let e0201_msgs: Vec<_> = resolve_diags
        .iter()
        .filter(|d| d.code == Some("E0201"))
        .map(|d| d.message.clone())
        .collect();
    assert_eq!(
        e0201_msgs.len(),
        1,
        "expected exactly one E0201 for `not_exported`, got: {e0201_msgs:?}"
    );
    assert!(
        e0201_msgs[0].contains("not_exported"),
        "E0201 must name `not_exported`: {}",
        e0201_msgs[0]
    );
}

#[test]
fn empty_imports_slice_behaves_as_before() {
    // With no imports, file-level names still resolve and unknown names are E0201.
    let source = "MAX :: 42;\nX :: MAX;\nY :: UNDEFINED;";
    let interner = Interner::new();
    let f = file();
    let parsed = parse(source, f);
    let (hir, lower_diags) = lower_file(&parsed, f, &interner);
    assert!(lower_diags.is_empty());

    let (_, resolve_diags) = resolve(&hir, &[], &interner);
    let e0201_count = resolve_diags
        .iter()
        .filter(|d| d.code == Some("E0201"))
        .count();
    assert_eq!(e0201_count, 1, "expected exactly one E0201 for UNDEFINED");
}

#[test]
fn export_scope_contains_all_file_level_names() {
    // FileHir::export_scope() must return the full file scope (W1 over-share).
    let source = "A :: 1;\nB :: 2;\nmain :: () {}";
    let interner = Interner::new();
    let f = file();
    let parsed = parse(source, f);
    let (hir, diags) = lower_file(&parsed, f, &interner);
    assert!(diags.is_empty());

    let scope = hir.export_scope();
    let a_sym = interner.intern("A");
    let b_sym = interner.intern("B");
    let main_sym = interner.intern("main");
    assert!(scope.get(a_sym).is_some(), "export_scope must contain A");
    assert!(scope.get(b_sym).is_some(), "export_scope must contain B");
    assert!(
        scope.get(main_sym).is_some(),
        "export_scope must contain main"
    );
}

#[test]
fn cycle_modules_resolve_without_infinite_recursion() {
    // Cycle_A imports Cycle_B and vice versa. Since we receive already-built
    // scopes, there is no recursion. This test proves the guarantee from
    // ADR-0014 §4.
    //
    // Simulate: file imports Cycle_A; Cycle_A's scope contains `a_calls_b`
    // and `A_VALUE`; Cycle_B's scope contains `b_value`. We only resolve the
    // top-level file here; the cycle is in the module scopes we hand in.
    let source = r#"#import "Cycle_A";
main :: () {
    x := a_calls_b();
    y := A_VALUE;
}
"#;
    let interner = Interner::new();
    let f = file();
    let parsed = parse(source, f);
    let (hir, lower_diags) = lower_file(&parsed, f, &interner);
    assert!(lower_diags.is_empty());

    // Cycle_A's scope (already built, no recursion needed).
    let cycle_a_scope = make_scope(&["a_calls_b", "A_VALUE"], &interner);
    let (_, resolve_diags) = resolve(&hir, &[("Cycle_A", &cycle_a_scope)], &interner);
    assert!(
        resolve_diags.is_empty(),
        "cycle modules must resolve cleanly: {:?}",
        resolve_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Dump
// ---------------------------------------------------------------------------

#[test]
fn dump_produces_non_empty_output() {
    let (hir, _, interner) = lower("main :: () { x := 1; }");
    let dump = dump_hir(&hir, &interner);
    assert!(!dump.is_empty());
    assert!(dump.contains("Proc"));
}

#[test]
fn dump_hello_snapshot() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/valid/024-hello.jr"),
    )
    .expect("failed to read 024-hello.jr");
    let interner = Interner::new();
    let f = file();
    let parsed = parse(&source, f);
    let (hir, diags) = lower_file(&parsed, f, &interner);
    assert!(diags.is_empty(), "unexpected lowering diagnostics");
    let dump = dump_hir(&hir, &interner);
    // Snapshot test: just verify key content is present
    assert!(dump.contains("Point"), "dump should contain Point struct");
    assert!(dump.contains("add"), "dump should contain add proc");
    assert!(dump.contains("main"), "dump should contain main proc");
    assert!(
        dump.contains("MESSAGE"),
        "dump should contain MESSAGE constant"
    );
    assert!(
        dump.contains("COMPUTED"),
        "dump should contain COMPUTED constant"
    );
}
