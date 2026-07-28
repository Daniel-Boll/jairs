//! Lazy, on-demand semantic analysis: type checking, inference, const-evaluation, and polymorph instantiation.
//!
//! Today this crate does the first two. Const-evaluation waits for `jr-vm`
//! (ADR-0016 §4) and polymorph instantiation for wave W5.
//!
//! # Two phases, and why the split is load-bearing
//!
//! [`file_signatures`] types declarations; [`check_file`] types everything else.
//! The split is not a convenience — ADR-0016 §5 requires it. Typing a call into
//! an imported module needs that module's signatures, so if computing signatures
//! could consult another file's *check*, then `Cycle_A` and `Cycle_B` importing
//! each other would make the query graph cyclic and
//! `tests/corpus/imports/valid/005-import-cycle-is-legal.jr` would not terminate.
//! Signatures therefore depend on another file's HIR only, exactly as
//! `file_exports` does one layer down (`jr-db`'s `module_loader`).
//!
//! The consequence to keep in mind: a procedure's signature must be typeable from
//! syntax alone. That holds in Jairs-0 because parameter and return types are
//! always written out, and it stops holding the day return-type inference lands.
//!
//! # The typing rules are decided elsewhere
//!
//! ADR-0015 fixes type *identity* — struct types are nominal, pointers are
//! structural, `string` is not the struct of its own layout, a procedure type
//! includes its context kind — and `jr-pool` implements it. ADR-0016 fixes the
//! *rules*:
//!
//! 1. An integer literal has no intrinsic type. It takes the type of its context
//!    and defaults to `s64`. This is what makes `g: u8 = 255;` legal without a
//!    cast, in a language subset that has no cast yet.
//! 2. Binding the result of a procedure that returns nothing is an error.
//! 3. A `#system_library` constant has a foreign-library handle type, so that
//!    `#foreign libc "write"` can check its library operand.
//! 4. `#run e` has the type of `e` and is **not** folded here. A tree-walking
//!    evaluator in this crate would be the second evaluator that PLAN.md §3.1's
//!    same-MIR invariant exists to forbid.
//! 5. Cross-file typing goes through signatures only.
//!
//! Do not re-derive these; they were decided with the corpus in hand, and the
//! corpus outranks the prose (`docs/spec/README.md`).
//!
//! # Poison, not gating
//!
//! `jr_db::file_diagnostics` runs every phase regardless of whether an earlier
//! one failed, because an IDE wants name resolution on a file that does not parse.
//! That makes silent poison propagation this crate's obligation rather than a
//! nicety: [`jr_pool::PoolId::ERROR`] flows through every operation without
//! producing a diagnostic, and so do `TypeRef::Error`, `Expr::Error` and
//! `Res::Error`. A checker that reported on poison would turn one parse error into
//! a page of invented type errors.
//!
//! # Diagnostic codes
//!
//! `E02xx` is semantic analysis as a whole and is shared with `jr-hir`, which owns
//! E0200–E0211. This crate owns E0212 upwards, plus two codes it does not own:
//!
//! | Code  | Meaning |
//! |-------|---------|
//! | E0204 | integer literal does not fit its contextual type *(relocated from lowering)* |
//! | E0211 | ambiguous name from two imported modules *(shared with `jr-hir`; raised here for type position)* |
//! | E0212 | unknown type name |
//! | E0213 | name is not a type |
//! | E0214 | mismatched types |
//! | E0215 | expression is not callable |
//! | E0216 | wrong number of arguments |
//! | E0217 | cannot bind the result of a procedure that returns nothing |
//! | E0218 | no such field |
//! | E0219 | cannot dereference a non-pointer |
//! | E0220 | invalid assignment target |
//! | E0221 | cannot take the address of this expression |
//! | E0222 | condition is not `bool` |
//! | E0223 | operator not supported for this type |
//! | E0224 | `return` disagrees with the procedure's return type |
//! | E0225 | `#foreign` library operand is not a library |
//! | E0226 | a constant's type depends on itself |
//!
//! E0227 was the first free code when this crate claimed its range; E0227–E0229 are
//! `jr-mir`'s, E0230 and E0231 are `jr-db`'s, and **E0232 is free**.
//!
//! E0212 and E0218 each carry a `did you mean` help line where a near name exists
//! (ADR-0031 §1). The suggestion is computed here rather than in an editor because the
//! candidate set — the fields of a type, the names that denote one — is semantic
//! information only the checker has, and a second implementation of the guess in `jr-lsp`
//! would leave `jr check` permanently worse at explaining its own error.
//!
//! # What is deliberately not checked yet
//!
//! - **Definite assignment.** `c: s64 = ---;` declares an uninitialised local and
//!   reading it before assignment is wave W3's job, not a typing question.
//! - **Missing `return`.** Whether every path through a non-`void` procedure
//!   returns is control flow, and needs MIR's CFG rather than a syntax walk.
//! - **Assignability between values of different types.** ADR-0015 left it
//!   unspecified and ADR-0016 narrowed context typing to *literals* on purpose;
//!   until `cast()` arrives in W1, "the types are equal" is the whole rule.

mod check;
mod code;
mod ctx;
mod map;
mod signature;
mod sigs;
mod suggest;

pub use check::{CheckOutput, check_file};
pub use map::TypeMap;
pub use signature::{ImportedFile, SignatureOutput, file_signatures};
pub use sigs::{FileSignatures, ProcSig, SigEntry, SigKind};
