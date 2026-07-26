//! The type-identity rules of ADR-0015, pinned as tests.
//!
//! These exist because every one of these rules is a silent failure if it
//! regresses. Nominal identity that quietly becomes structural does not crash —
//! it accepts a `Vec2` where a `Point` was wanted, which then type-checks and
//! compiles and is wrong at runtime. A test is the only thing that notices.

use jr_base::{FileId, Interner};
use jr_pool::{ContextKind, DeclId, Field, Item, Pool, PoolId};

fn decl(file: u32, index: u32) -> DeclId {
    DeclId::new(FileId::from_u32(file), index)
}

// ---------------------------------------------------------------------------
// Struct identity is nominal (ADR-0015 §1)
// ---------------------------------------------------------------------------

/// `Point :: struct { x: s64; y: s64; }` and `Vec2 :: struct { x: s64; y: s64; }`
/// are different types. This is the single most important assertion in the
/// crate: if it fails, the language has structural struct typing.
#[test]
fn identically_shaped_structs_from_different_declarations_are_different_types() {
    let interner = Interner::new();
    let mut pool = Pool::new();

    let point_decl = decl(0, 0);
    let vec2_decl = decl(0, 1);

    let point = pool.struct_type(point_decl);
    let vec2 = pool.struct_type(vec2_decl);

    // Give them byte-for-byte identical bodies.
    let fields = vec![
        Field::new(interner.intern("x"), PoolId::S64),
        Field::new(interner.intern("y"), PoolId::S64),
    ];
    pool.set_struct_fields(point_decl, fields.clone());
    pool.set_struct_fields(vec2_decl, fields);

    assert_ne!(
        point, vec2,
        "structs are nominal: identical fields must not make identical types"
    );
    assert_eq!(
        pool.struct_fields(point_decl),
        pool.struct_fields(vec2_decl)
    );
}

/// The same declaration interned twice is the same type, which is what makes
/// ADR-0005's structural instantiation keying still work under nominal identity:
/// two mentions of `Point` key equally.
#[test]
fn the_same_declaration_interns_to_one_type() {
    let mut pool = Pool::new();
    let d = decl(3, 7);
    assert_eq!(pool.struct_type(d), pool.struct_type(d));
}

/// Declaration identity includes the file, so the same index in two files is two
/// types. Otherwise every file's first struct would collide.
#[test]
fn declaration_identity_is_per_file() {
    let mut pool = Pool::new();
    assert_ne!(pool.struct_type(decl(0, 0)), pool.struct_type(decl(1, 0)));
}

/// A struct type has an ID before its fields are known. This is what allows
/// `Node :: struct { next: *Node; }`: the pointer field needs `Node`'s ID while
/// `Node`'s own body is still being lowered.
#[test]
fn a_struct_type_exists_before_its_body_is_resolved() {
    let interner = Interner::new();
    let mut pool = Pool::new();
    let node_decl = decl(0, 0);

    let node = pool.struct_type(node_decl);
    assert_eq!(pool.struct_fields(node_decl), None);

    let next = pool.pointer_to(node);
    pool.set_struct_fields(node_decl, vec![Field::new(interner.intern("next"), next)]);

    let fields = pool.struct_fields(node_decl).expect("fields were just set");
    assert_eq!(fields[0].ty, next);
    assert_eq!(*pool.item(next), Item::PointerType(node));
}

// ---------------------------------------------------------------------------
// `string` is a distinct builtin (ADR-0015 §2)
// ---------------------------------------------------------------------------

/// A user struct with exactly the `string` layout is not `string`. If this fails,
/// ADR-0004's ability to give `string` special behaviour is gone.
#[test]
fn a_user_struct_matching_the_string_layout_is_not_string() {
    let interner = Interner::new();
    let mut pool = Pool::new();

    let look_alike_decl = decl(0, 0);
    let look_alike = pool.struct_type(look_alike_decl);
    pool.set_struct_fields(
        look_alike_decl,
        vec![
            Field::new(interner.intern("data"), PoolId::PTR_U8),
            Field::new(interner.intern("count"), PoolId::S64),
        ],
    );

    assert_ne!(look_alike, PoolId::STRING);
}

// ---------------------------------------------------------------------------
// Pointers are structural, and nest (ADR-0015 §4)
// ---------------------------------------------------------------------------

#[test]
fn pointers_are_structural() {
    let mut pool = Pool::new();
    assert_eq!(pool.pointer_to(PoolId::S64), pool.pointer_to(PoolId::S64));
    assert_ne!(pool.pointer_to(PoolId::S64), pool.pointer_to(PoolId::U8));
}

#[test]
fn pointers_nest() {
    let mut pool = Pool::new();
    let p = pool.pointer_to(PoolId::S64);
    let pp = pool.pointer_to(p);
    let ppp = pool.pointer_to(pp);

    // Each level is a distinct type.
    assert_ne!(p, pp);
    assert_ne!(pp, ppp);
    assert_ne!(p, ppp);

    // And the nesting is visible in the item, one level at a time.
    assert_eq!(*pool.item(ppp), Item::PointerType(pp));
    assert_eq!(*pool.item(pp), Item::PointerType(p));
    assert_eq!(*pool.item(p), Item::PointerType(PoolId::S64));

    // `**s64` reached again is the same type.
    assert_eq!(pp, pool.pointer_to(p));
}

// ---------------------------------------------------------------------------
// Procedure type identity (ADR-0015 §4, ADR-0001, ADR-0008)
// ---------------------------------------------------------------------------

/// The context flag is part of the identity, per ADR-0001. Two procedure types
/// that differ *only* in it must not unify, or a `#c_call` function pointer could
/// be used where a context-taking one is expected.
#[test]
fn procedure_types_differing_only_in_the_context_flag_are_different() {
    let mut pool = Pool::new();
    let jairs = pool.proc_type(vec![PoolId::S64], PoolId::S64, ContextKind::Jairs);
    let c_call = pool.proc_type(vec![PoolId::S64], PoolId::S64, ContextKind::CCall);
    assert_ne!(jairs, c_call);
}

#[test]
fn procedure_type_identity_covers_parameters_and_return() {
    let mut pool = Pool::new();
    let base = pool.proc_type(vec![PoolId::S64], PoolId::S64, ContextKind::Jairs);

    // Same shape again.
    assert_eq!(
        base,
        pool.proc_type(vec![PoolId::S64], PoolId::S64, ContextKind::Jairs)
    );
    // Different parameter type.
    assert_ne!(
        base,
        pool.proc_type(vec![PoolId::U8], PoolId::S64, ContextKind::Jairs)
    );
    // Different arity.
    assert_ne!(
        base,
        pool.proc_type(
            vec![PoolId::S64, PoolId::S64],
            PoolId::S64,
            ContextKind::Jairs
        )
    );
    // Different return type.
    assert_ne!(
        base,
        pool.proc_type(vec![PoolId::S64], PoolId::U8, ContextKind::Jairs)
    );
    // Parameter order matters.
    let ab = pool.proc_type(
        vec![PoolId::S64, PoolId::U8],
        PoolId::VOID,
        ContextKind::Jairs,
    );
    let ba = pool.proc_type(
        vec![PoolId::U8, PoolId::S64],
        PoolId::VOID,
        ContextKind::Jairs,
    );
    assert_ne!(ab, ba);
}

/// A procedure that returns nothing returns `void` — there is no absent case
/// (ADR-0015 §3). `main :: ()` and `discard :: (unused: s64)` differ only in
/// their parameters.
#[test]
fn returning_nothing_is_the_void_type() {
    let mut pool = Pool::new();
    let main_ty = pool.proc_type(vec![], PoolId::VOID, ContextKind::Jairs);

    let Item::ProcType { ret, .. } = pool.item(main_ty) else {
        panic!("expected a procedure type");
    };
    assert_eq!(*ret, PoolId::VOID);
    assert_ne!(
        main_ty,
        pool.proc_type(vec![], PoolId::S64, ContextKind::Jairs),
        "returning nothing must differ from returning s64"
    );
}

/// Two procedures with the same signature share one *type* but are two distinct
/// *values*, because procedures are constants (ADR-0012).
#[test]
fn procedures_with_one_signature_are_distinct_values() {
    let mut pool = Pool::new();
    let ty = pool.proc_type(vec![PoolId::S64], PoolId::S64, ContextKind::Jairs);

    let a = pool.proc_value(ty, decl(0, 0));
    let b = pool.proc_value(ty, decl(0, 1));

    assert_ne!(a, b);
    assert_eq!(pool.type_of(a), pool.type_of(b));
    assert_eq!(pool.type_of(a), ty);
}

// ---------------------------------------------------------------------------
// The poison type
// ---------------------------------------------------------------------------

/// The error type is equal only to itself. It must never unify with a real type,
/// or one lowering error would silently become a type-check success.
#[test]
fn the_error_type_is_equal_only_to_itself() {
    let mut pool = Pool::new();
    assert_eq!(pool.intern(Item::ErrorType), PoolId::ERROR);
    for other in [PoolId::VOID, PoolId::BOOL, PoolId::S64, PoolId::STRING] {
        assert_ne!(PoolId::ERROR, other);
    }
}
