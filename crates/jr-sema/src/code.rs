//! The diagnostic codes owned by semantic analysis.
//!
//! `E02xx` is semantic analysis as a whole, shared with `jr-hir`: E0200–E0211 are
//! lowering and name resolution, and everything from E0212 up is this crate.
//! [`E0204`] is the exception — it is a *relocated* code, not a new one (see its
//! own documentation).
//!
//! Two of these are deliberately shared with another crate rather than
//! duplicated under a new number, because a user reading `E0211` should not have
//! to know which phase noticed the problem.

/// An integer literal does not fit the type its context gives it.
///
/// **Relocated from lowering.** ADR-0016 §1 makes an integer literal's type come
/// from its context, and lowering does not know the context — it tested every
/// literal against `s64`, which silently accepted `x: u8 = 300;` and worded the
/// error after the wrong type. The check moved here and kept its number, because
/// it is the same error about the same source text.
pub(crate) const E0204: &str = "E0204";

/// A name is provided by two or more imported modules.
///
/// **Shared with `jr-hir`**, which raises the same code when the ambiguous name
/// is used in an *expression*. This crate raises it for a use in *type* position,
/// which name resolution never sees: `ResolveMap` covers `Expr::Name` only, and a
/// `TypeRef::Name` is not an expression.
pub(crate) const E0211: &str = "E0211";

/// A type annotation names something that does not exist.
pub(crate) const E0212: &str = "E0212";

/// A type annotation names something that exists but is not a type.
pub(crate) const E0213: &str = "E0213";

/// Mismatched types.
pub(crate) const E0214: &str = "E0214";

/// The callee of a call is not a procedure.
pub(crate) const E0215: &str = "E0215";

/// A call passes the wrong number of arguments.
pub(crate) const E0216: &str = "E0216";

/// A declaration binds the result of a procedure that returns nothing
/// (ADR-0016 §2).
pub(crate) const E0217: &str = "E0217";

/// A field access names a field the type does not have.
pub(crate) const E0218: &str = "E0218";

/// `.*` applied to something that is not a pointer.
pub(crate) const E0219: &str = "E0219";

/// The left-hand side of an assignment is not something that can be assigned to.
pub(crate) const E0220: &str = "E0220";

/// Prefix `*` applied to something with no address.
pub(crate) const E0221: &str = "E0221";

/// The condition of an `if` or `while` is not a `bool`.
pub(crate) const E0222: &str = "E0222";

/// An operator does not apply to the type of its operands.
pub(crate) const E0223: &str = "E0223";

/// A `return` disagrees with the enclosing procedure's return type.
pub(crate) const E0224: &str = "E0224";

/// The library operand of `#foreign` is not a foreign library (ADR-0016 §3).
pub(crate) const E0225: &str = "E0225";

/// A constant's type depends on its own type.
pub(crate) const E0226: &str = "E0226";

/// `cast(T, x)` where either side is not an integer type (ADR-0037 §2).
///
/// Only integer-to-integer conversions exist in this wave. A pointer is deliberately not an
/// integer kind, so casting one is refused here rather than becoming pointer arithmetic by
/// the back door; floats arrive in a later wave.
pub(crate) const E0232: &str = "E0232";

/// An array length that is not a usable integer literal (ADR-0039 §3a).
///
/// Raised for `[COUNT]u8` — a named constant, whose value would have to come from the
/// const-evaluator this crate has no access to (ADR-0018 §3 puts it in `jr-db`, downstream)
/// — and for a literal that is negative or does not fit a `u64`.
///
/// Reported *here* rather than in `jr-hir`'s lowering, even though lowering is what reads
/// the literal token: `tests/corpus/type-errors/` requires its files to lower cleanly and
/// be rejected by sema alone, and rejecting a type is a semantic judgement anyway.
pub(crate) const E0233: &str = "E0233";

/// An enum member's explicit value that is not a usable integer literal (ADR-0041 §3).
pub(crate) const E0237: &str = "E0237";

/// A name that is not a member of the enum it was looked up in (ADR-0041).
pub(crate) const E0238: &str = "E0238";

/// Indexing something that is not an array (ADR-0039 §5).
pub(crate) const E0234: &str = "E0234";

/// An index whose type is not an integer (ADR-0039 §5).
pub(crate) const E0235: &str = "E0235";

/// A constant index proven out of range at compile time (ADR-0039 §2).
///
/// The check is a *runtime* one, but an index that is a literal is decidable here, and a
/// program whose index cannot possibly be in range is better refused than compiled into a
/// guaranteed trap — the same reasoning ADR-0016 §1 applies to a literal that does not fit
/// its type.
pub(crate) const E0236: &str = "E0236";

/// Slicing something that cannot be sliced, or that has no storage (ADR-0044 §2, §6).
///
/// Covers three shapes: `[]` on a non-array, `[]` on a view (which would be an identity, and
/// an operator that silently does nothing is one a reader concludes did something), and `[]`
/// on an expression that is not a place — `[]` takes an address, so it needs storage.
pub(crate) const E0239: &str = "E0239";

/// An array where a view was expected (ADR-0044 §2).
///
/// A *specific* code rather than the generic mismatch [`E0214`], because this is the one
/// mismatch with one fix and Jairs deliberately has no implicit conversion to apply: the help
/// names `buf[]`. A reader arriving from Jai, where the conversion is implicit, hits this
/// first.
pub(crate) const E0240: &str = "E0240";

/// `==` or `!=` on a view (ADR-0044 §5).
///
/// Refused rather than given a meaning: two views could compare as "the same storage" or as
/// "the same contents", and picking one silently would make the wrong reading a bug that
/// looks like working code.
pub(crate) const E0241: &str = "E0241";

/// `xx` where the context supplies no target type (ADR-0046 §2).
///
/// The help names `cast(T, x)`, because that is the recoverable fact: an explicit form exists
/// and always will. "Cannot infer" on its own is accurate and useless — the ADR-0043 lesson.
pub(crate) const E0242: &str = "E0242";

/// `xx` applied to an untyped literal (ADR-0046 §2).
///
/// A literal already takes its type from context (ADR-0016 §1), so `xx` adds nothing — and it
/// would *suppress* the fit check that makes `x: u8 = 300;` an error. A separate code from
/// [`E0242`] because "delete the `xx`" is a different instruction from "add a type".
pub(crate) const E0243: &str = "E0243";

/// A bare `.RED` whose context is absent or is not an enum (ADR-0046 §3).
///
/// Deliberately **not** "unresolved name `RED`", which would send the reader looking for a
/// declaration that was never meant to exist. When there *is* a context type it is named, since
/// "expected `s64`, and a bare member needs an enum" is a different problem from having no
/// context at all.
pub(crate) const E0244: &str = "E0244";

/// A declaration that cannot be an operator overload (ADR-0048 §2, §3).
///
/// Three shapes, each with its own note because each is a different fact a reader can act on:
/// the wrong number of parameters (unary overloading is out of scope, §6), an operator the
/// language reserves (wrapping is about a machine representation; bitwise belongs to
/// `enum_flags`; `&&`/`||` are control flow), and the **orphan rule** — at least one operand must
/// be a nominal type declared in this file, so that an `#import` cannot change what an operator
/// means for types it does not own.
pub(crate) const E0246: &str = "E0246";

/// A `for` over something that cannot be iterated, or a range whose ends are not integers
/// (ADR-0049 §1).
///
/// A `for` knows three shapes — an array, a view, and a range — and there is deliberately no
/// user-extensible protocol: Jai's `for_expansion` is a macro, which needs W5's `#expand` and
/// hygiene. The help says so, because "cannot iterate" without the reason invites the reader to
/// look for a spelling that does not exist.
pub(crate) const E0247: &str = "E0247";

/// A destructuring statement that does not match its call's results (ADR-0052 §2, §3, §4).
///
/// Four conditions, one code, each with its own note:
///
/// * the target count differs from the result count. **Exact arity**, deliberately: allowing a
///   caller to take a prefix — which Jai does — would make adding a result silently change nothing
///   at any call site, and *reordering* results silently change what every caller binds;
/// * the right-hand side is not a call at all, so there are no results to destructure;
/// * the right-hand side is a call to a procedure returning **one** value, which is `x := f()` and
///   needs no target list;
/// * a results type used where a value's type belongs — `t: (s64, bool)` — which ADR-0052 §4 keeps
///   unspellable so that reusing the struct machinery does not silently add tuples to the language.
pub(crate) const E0251: &str = "E0251";

/// A named argument or default value that does not fit ADR-0053's rules.
///
/// Six conditions, one code, each with its own note:
///
/// * a default that is not a **literal**. Refused because const-eval runs downstream of signature
///   resolution (ADR-0018 §3), so a signature cannot depend on a computed constant — the same
///   ordering ADR-0039 §3a records for an array length;
/// * a default on a `#foreign` parameter, which Jairs does not control the call sites of;
/// * a **positional** argument after a named one, which would make a positional argument's meaning
///   depend on which names preceded it;
/// * a parameter supplied **twice**, positionally and by name;
/// * a parameter with **no argument and no default**; and
/// * a named argument naming a parameter that does not exist — reported with a near-name
///   suggestion, since a misspelling is exactly what ADR-0031 §1's machinery is for.
pub(crate) const E0252: &str = "E0252";

/// `context` used where there is none (ADR-0057 §3).
///
/// Two conditions: inside a `#c_call` procedure, which receives no implicit context by definition —
/// including every `#foreign` one, since ADR-0001 makes those implicitly `#c_call`; and at **file
/// scope**, where a constant's value is computed by const-eval and there is no call to have carried a
/// context in.
///
/// Distinct from E0201 "unresolved name" deliberately: `context` is a keyword, so it always resolves
/// and the mistake is that this procedure has none — reporting it as unresolved would send the reader
/// looking for a declaration to add.
pub(crate) const E0254: &str = "E0254";

/// `#no_abc` on a `#foreign` procedure (ADR-0058 §3).
///
/// The directive suppresses a procedure's bounds checks, and a `#foreign` declaration has no body —
/// so there is no index in it to leave unchecked and the directive could only be a word that does
/// nothing.
///
/// Refused rather than ignored, because a silently-ignored directive tells the writer nothing: they
/// asked for no bounds checks across a boundary Jairs does not compile, and the honest answer is
/// that the request has no meaning rather than that it was granted.
pub(crate) const E0255: &str = "E0255";

/// A `#foreign` procedure used as a value (ADR-0059 §5).
///
/// Taking a procedure as a value — `f := add` — is how a proc pointer is made. A `#foreign`
/// procedure's type is `ContextKind::CCall` and the VM reaches it through libffi rather than a
/// `ProcRef`, so an indirect call to one is a second calling convention this wave does not build.
///
/// Refused at the point the value is *taken* rather than at the call, so the reader is told where
/// they wrote the mistake — and distinct from a type mismatch (E0214), because `#foreign write`
/// does have a procedure type and the objection is specifically that it is a foreign one.
pub(crate) const E0256: &str = "E0256";

/// `null` in a context that is not a pointer type, or with no context (ADR-0060 §1).
///
/// `null` takes its type from context, like an integer literal — but with no default, because there
/// is no one pointer type to fall back to. `n: s64 = null` is a non-pointer context; `q := null` is
/// no context at all. Distinct from E0214 "mismatched types" because the objection is specifically
/// that a *pointer* was needed, which the note says.
pub(crate) const E0257: &str = "E0257";

/// A `switch` on an enum that does not name every member (ADR-0067 §3).
///
/// The point of adding matching is that the compiler can *prove* a case is handled, so a missing
/// member is an error rather than a warning — a warning would leave the proof optional, the same
/// "behaviour depends on something invisible" ADR-0014 §3 refuses.
///
/// The message names the members that are missing rather than counting them, because the missing name
/// *is* the fix and a count makes the reader re-derive it. Only raised for an enum scrutinee: an `s64`
/// has no finite member set to be exhaustive over, so it needs an `else` instead.
pub(crate) const E0258: &str = "E0258";

/// A duplicate `case` value, or a second `else`, in one `switch` (ADR-0067 §4).
///
/// The second arm can never run, and an arm that cannot run is a statement the reader believes does.
/// Reported against the *later* arm, since the earlier one is the one that works.
pub(crate) const E0259: &str = "E0259";

/// An `else` arm on a `switch` that already names every member of its enum (ADR-0067 §4).
///
/// Unreachable, and this is the diagnostic that makes E0258 worth having: without it every `switch`
/// could end in `else` and the exhaustiveness check would never fire.
pub(crate) const E0260: &str = "E0260";

/// A type used where a runtime value was expected (ADR-0071 §3).
///
/// A type is a compile-time value (ADR-0012), and `jr_pool::LayoutError::ComptimeOnly` says the same
/// thing from the layout side: `Item::TypeType` has no runtime size, and asking for one "is a category
/// error". So a type cannot be stored, and `t := Point;` is asking for storage.
///
/// **This code exists because the case was a silent miscompile.** Before it, `t := Point;` type-checked
/// cleanly and both engines exited 0, lowering to `s0: type` and `v1: type = undef` — a placeholder
/// that is a *legitimate value*, so neither the verifier nor ADR-0017 §4's poison gate could catch it.
/// That is this project's first named failure mode, and ADR-0017 §4's rule is that such a case refuses.
///
/// Distinct from E0214 because the objection is not that some *other* type was wanted — for `t :=
/// Point;` nothing was wanted — but that a type is not a value at all. The message therefore names the
/// positions that do accept a type rather than naming an expected type the reader could write: `Type`
/// is deliberately not spellable (ADR-0071 §1), so there is no annotation to suggest.
pub(crate) const E0261: &str = "E0261";
