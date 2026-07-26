//! Compile-time values, and the totality of `type_of`.
//!
//! `type_of` being total is what justifies `void` existing as a real type
//! (ADR-0015 §3) and types and values sharing one index space. A gap in it would
//! surface much later as an `unreachable!` in the middle of type checking, so it
//! is exercised over every item kind here rather than left to inference.

use jr_base::{FileId, Interner};
use jr_pool::{ContextKind, DeclId, Field, Item, Pool, PoolId};

fn decl(index: u32) -> DeclId {
    DeclId::new(FileId::from_u32(0), index)
}

/// Every item kind, type and value alike, must answer `type_of`.
#[test]
fn type_of_is_total_over_every_item_kind() {
    let interner = Interner::new();
    let mut pool = Pool::new();

    // One of each type kind.
    let struct_decl = decl(0);
    let struct_ty = pool.struct_type(struct_decl);
    pool.set_struct_fields(
        struct_decl,
        vec![Field::new(interner.intern("x"), PoolId::S64)],
    );
    let ptr = pool.pointer_to(PoolId::S64);
    let proc_ty = pool.proc_type(vec![PoolId::S64], PoolId::VOID, ContextKind::Jairs);

    let types = [
        PoolId::VOID,
        PoolId::BOOL,
        PoolId::S64,
        PoolId::U8,
        PoolId::STRING,
        PoolId::TYPE,
        PoolId::ERROR,
        ptr,
        struct_ty,
        proc_ty,
    ];
    for ty in types {
        assert!(pool.is_type(ty), "{:?} should be a type", pool.item(ty));
        assert_eq!(
            pool.type_of(ty),
            PoolId::TYPE,
            "the type of a type is `type`: {:?}",
            pool.item(ty)
        );
    }

    // One of each value kind, with its expected type.
    let int = pool.int_value(PoolId::S64, 42);
    let string = pool.str_value("hello from Jairs\n");
    let type_value = pool.type_value(struct_ty);
    let proc_value = pool.proc_value(proc_ty, decl(1));

    let values = [
        (PoolId::VOID_VALUE, PoolId::VOID),
        (PoolId::TRUE, PoolId::BOOL),
        (PoolId::FALSE, PoolId::BOOL),
        (int, PoolId::S64),
        (string, PoolId::STRING),
        (type_value, PoolId::TYPE),
        (proc_value, proc_ty),
    ];
    for (value, expected) in values {
        assert!(
            !pool.is_type(value),
            "{:?} should be a value",
            pool.item(value)
        );
        assert_eq!(
            pool.type_of(value),
            expected,
            "wrong type for {:?}",
            pool.item(value)
        );
    }
}

/// The same bit pattern at two types is two values. This is why the type is part
/// of an integer value's key — `42` as `s64` and `42` as `u8` are not the same
/// compile-time value, and polymorph instantiation keys on values (ADR-0005).
#[test]
fn integer_values_are_keyed_by_type_as_well_as_bits() {
    let mut pool = Pool::new();
    let as_s64 = pool.int_value(PoolId::S64, 42);
    let as_u8 = pool.int_value(PoolId::U8, 42);

    assert_ne!(as_s64, as_u8);
    assert_eq!(pool.type_of(as_s64), PoolId::S64);
    assert_eq!(pool.type_of(as_u8), PoolId::U8);

    // And equal values at one type still dedupe.
    assert_eq!(as_s64, pool.int_value(PoolId::S64, 42));
    assert_ne!(as_s64, pool.int_value(PoolId::S64, 43));
}

/// `COMPUTED :: #run add(2, 3)` must be indistinguishable from the literal `5`
/// once folded (`docs/spec/02-declarations.md`). Interning is what delivers that:
/// there is no provenance in the key, so the folded result *is* the literal.
#[test]
fn a_folded_comptime_result_is_indistinguishable_from_a_literal() {
    let mut pool = Pool::new();
    let literal_five = pool.int_value(PoolId::S64, 5);
    let folded_five = pool.int_value(PoolId::S64, 2 + 3);
    assert_eq!(literal_five, folded_five);
}

/// A type used as a value is not the type itself: `Point` the type and `Point`
/// the comptime value have different IDs, and the value's type is `type`.
#[test]
fn a_type_and_that_type_as_a_value_are_distinct_entries() {
    let mut pool = Pool::new();
    let point = pool.struct_type(decl(0));
    let point_as_value = pool.type_value(point);

    assert_ne!(point, point_as_value);
    assert!(pool.is_type(point));
    assert!(!pool.is_type(point_as_value));
    assert_eq!(pool.type_of(point_as_value), PoolId::TYPE);
    assert_eq!(*pool.item(point_as_value), Item::TypeValue(point));

    // Interning it again dedupes, which is what lets ADR-0005 key polymorph
    // instantiation on argument *values* and still de-duplicate `sort(Point)`
    // reached from two files.
    assert_eq!(point_as_value, pool.type_value(point));
}

/// Strings dedupe by contents, not by identity of the `&str` handed in.
#[test]
fn string_values_dedupe_by_contents() {
    let mut pool = Pool::new();
    let owned = String::from("hello");
    let a = pool.str_value("hello");
    let b = pool.str_value(&owned);
    assert_eq!(a, b);

    // The empty string is a legal value and distinct from every other.
    let empty = pool.str_value("");
    assert_ne!(a, empty);
    let Item::StrValue(sid) = *pool.item(empty) else {
        panic!("expected a string value");
    };
    assert_eq!(pool.resolve_str(sid), "");
}

/// The pool grows only when something genuinely new is interned. This is a proxy
/// for "de-duplication actually happens" that does not depend on any particular
/// ID.
#[test]
fn re_interning_does_not_grow_the_pool() {
    let mut pool = Pool::new();
    let _ = pool.pointer_to(PoolId::S64);
    let _ = pool.int_value(PoolId::S64, 1);
    let _ = pool.str_value("x");
    let settled = pool.len();

    for _ in 0..10 {
        let _ = pool.pointer_to(PoolId::S64);
        let _ = pool.int_value(PoolId::S64, 1);
        let _ = pool.str_value("x");
    }
    assert_eq!(pool.len(), settled);
}
