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
