//! The `InternPool`: canonical identities for every type *and* every compile-time value.
//!
//! Every type and every compile-time value in the compiler is interned here once
//! and thereafter referred to by a [`PoolId`] — a 32-bit index. Comparing two
//! types is then an integer compare rather than a structural walk, and IR nodes
//! that carry a type stay 4 bytes wide.
//!
//! ```
//! use jr_pool::{ContextKind, Pool, PoolId};
//!
//! let mut pool = Pool::new();
//!
//! // `(a: s64, b: s64) -> s64`
//! let add = pool.proc_type(vec![PoolId::S64, PoolId::S64], PoolId::S64, ContextKind::Jairs);
//!
//! // Interning is idempotent, which is the whole point.
//! assert_eq!(add, pool.proc_type(vec![PoolId::S64, PoolId::S64], PoolId::S64, ContextKind::Jairs));
//!
//! // A `#c_call` procedure of the same shape is a *different* type (ADR-0001).
//! assert_ne!(add, pool.proc_type(vec![PoolId::S64, PoolId::S64], PoolId::S64, ContextKind::CCall));
//! ```
//!
//! # What identity means here
//!
//! The pool's key design *is* the language's answer to "when are two types the
//! same type?", which ADR-0015 settles:
//!
//! - **Struct types are nominal.** They are keyed on their declaration site (a
//!   [`DeclId`]), not on their field list, so two separately-declared structs
//!   with identical fields are different types.
//! - **`string` is its own type**, not the struct whose layout ADR-0004 fixes. A
//!   user struct of shape `{data: *u8; count: s64;}` is a different type.
//! - **Pointer types are structural**, and nest.
//! - **A procedure type's identity** includes its parameters, its return type,
//!   its [`ContextKind`] (ADR-0001) and its inert [`EffectRow`] (ADR-0008).
//! - **`void` is a real type**, so a procedure type's return field is total
//!   rather than optional.
//!
//! Because identity is exactly derived `Hash`/`Eq` on [`Item`], these rules are
//! not enforced by checks scattered through the compiler — they fall out of what
//! the key does and does not contain.
//!
//! # Scope
//!
//! This crate answers *equality*, not conversion. Assignability and coercion are
//! `jr-sema`'s business and are deliberately unspecified for now (ADR-0015).
//!
//! It also answers *layout* — size, alignment and field offsets — because ADR-0018
//! §2 puts the one computation both backends share here, where its inputs already
//! live. See [`layout_of`] and [`TargetLayout`] for why, and for why the target is
//! a parameter rather than something this crate looks up.
//!
//! It also owns ADR-0002's integer arithmetic ([`int_binary`], [`int_compare`],
//! [`int_negate`], [`IntKind`]), because two crates now evaluate it — `jr-vm`'s
//! interpreter and `jr-mir`'s constant folder — and `jr-mir` cannot depend on
//! `jr-vm`. [`IntKind::of`] already read this crate's `Item::IntType`, so the
//! signedness and width were here and only the arithmetic over them moved
//! (ADR-0022 §2).
//!
//! It depends only on `jr-base`. It knows nothing about the HIR: callers
//! hand it a [`DeclId`] they built themselves, which keeps the pool a pure,
//! independently testable data structure rather than a second pass over
//! `jr-hir`.

mod arith;
mod cabi;
mod float;
mod item;
mod layout;
mod pool;

pub use arith::{IntCmp, IntKind, IntOp, IntTrap, int_binary, int_compare, int_negate, int_not};
pub use cabi::{Class, classify};
pub use float::{
    FloatCmp, FloatKind, FloatOp, float_binary, float_compare, float_negate, float_to_int,
    int_to_float,
};
pub use item::{ContextKind, DeclId, EffectRow, EnumMember, Field, Item, PoolId, StrId};
pub use layout::{
    CONTEXT_FIELD_NAMES, CONTEXT_FIELD_TYPES, Layout, LayoutError, TAG_ALIGN, TAG_SIZE,
    TargetLayout, TargetOs, align_up, field_offset, layout_of, pair_count, pair_data, pair_layout,
    static_image, string_count, string_data, string_layout, triple_capacity, triple_layout,
    variant_payload_offset,
};
pub use pool::Pool;
