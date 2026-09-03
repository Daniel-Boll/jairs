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

/// `Type_Info` is missing from the standard library, or is not shaped as the compiler expects
/// (ADR-0075 §2).
///
/// `type_info(T)` returns a `*Type_Info`, and ADR-0075 §2 declares that struct **in `modules/Basic`**
/// rather than in the compiler, so that it is *spellable*: no compiler-declared type is (`t: Type;` and
/// `c: Context;` both report E0212), and a reflection API whose type a program cannot name in a
/// signature is not usable.
///
/// The price of that choice is a compiler dependency on a declaration it does not own, and this code is
/// what keeps the price honest. The lookup validates the field names, types and order, so an edit to
/// `Basic`'s `Type_Info` produces *this diagnostic naming the mismatch* rather than a read of whatever
/// now sits at the old offset. A wrong offset would be a silent wrong value — this project's named
/// failure mode — and ADR-0017 §4's rule is that such a case refuses instead.
pub(crate) const E0265: &str = "E0265";

/// `type_info` applied to a type with no runtime layout (ADR-0075 §4).
///
/// A `Type_Info` reports a `size` and an `alignment`, and `Item::TypeType` has neither: `layout_of`
/// answers `LayoutError::ComptimeOnly`, whose documentation calls asking for such a size "a category
/// error". So `type_info(Type)` has no answer to give.
///
/// Refused rather than reported as zero, for the reason `type-errors/063` exists: a plausible wrong
/// number is worse than a refusal, because nothing downstream can tell it from a real answer. Distinct
/// from E0261, which objects to a type being used as a *runtime value* — here the type is in a position
/// that legitimately accepts one, and the objection is about which type it is.
pub(crate) const E0266: &str = "E0266";

/// `any_of` applied to something that is not a pointer (ADR-0076 §1).
///
/// An `Any` holds a **pointer** to the value it erases, so the caller decides what is pointed at and the
/// lifetime is visible in the source. `any_of(x)` for a non-pointer `x` is refused rather than having the
/// compiler take the address silently: `any_of(x)` and `any_of(*x)` would then mean the same thing, and
/// one of them is a lie about how long the pointee lives.
///
/// Distinct from E0214 because no particular pointer type was wanted — any pointer will do — so there is
/// no "expected" type to name.
pub(crate) const E0267: &str = "E0267";

/// A call to a polymorphic procedure, which is not yet instantiable (ADR-0081 §2).
///
/// `$T` parameters parse, lower, format and check *as a template* in this sub-wave — the signature is
/// recognised as polymorphic and its body is not checked against non-concrete parameter types. What is
/// deferred is **instantiation**: inferring `$T` from a call's arguments, interning the concrete
/// procedure type, and lowering a distinct procedure per instantiation. Until that sub-wave a call is
/// refused with this code, which is a *by-design* refusal — the construct is named as arriving later —
/// rather than an unimplemented gap left to miscompile.
pub(crate) const E0268: &str = "E0268";

/// A type-argument reference `Name(args)` whose `Name` is not a parameterised struct (ADR-0085 §3).
///
/// `Box(s64)` requires `Box` to be a `struct($T) { … }` declared in this file. This is raised when the
/// name is undeclared, names something that is not a struct, or names an ordinary (non-parameterised)
/// struct — in the last case the arguments are meaningless, so the reference is refused rather than the
/// arguments silently ignored.
///
/// **Its meaning narrowed in ADR-0117**: it used to catch an *imported* parameterised struct too, because
/// cross-file ones were deferred (ADR-0085 §5). They now work, so this means "not a parameterised struct in this
/// file **or any imported one**" — and the note no longer says cross-file is unsupported, because it is not.
pub(crate) const E0269: &str = "E0269";

/// A parameterised type reference `Name(args)` supplied the wrong number of type arguments (ADR-0085 §3).
///
/// `Box($T)` takes exactly one; `Box(s64, bool)` and `Box()` are both refused with this code, naming the
/// count wanted and the count written — the type-side counterpart of an arity error on a call.
pub(crate) const E0270: &str = "E0270";

// E0274 — a call to a `#modify` procedure — was ADR-0093 §3's by-design refusal while the predicate was
// unevaluated. **ADR-0095 lifted it**: the predicate now runs, and a `false` refuses the instantiation with
// E0275 (owned by `jr-db`, where the predicate is evaluated). The number is retired rather than reused, the
// way E0120 and E0122 were, so a reader searching for it finds this note.

/// A call to a `#expand` macro, whose splice is not yet built (ADR-0090 §3).
///
/// `#expand` marks a procedure as a **macro**: a call splices its body into the *caller's* scope rather
/// than calling it, so the body can read and modify the caller's locals (deliberately unhygienic, matching
/// Jai and matching `#insert`'s existing splice). The surface — parsing, lowering, formatting, and the
/// declaration checking like any other procedure — is this sub-wave; the **splice** is the next.
///
/// Refused rather than allowed to fall through to an ordinary call, which is what it did before this code
/// existed: `#expand` was *accepted and ignored*, so a macro silently behaved as a procedure. That is the
/// "a directive that is silently ignored is worse than one that is rejected" rule ADR-0058 §3 states, and
/// the reason this refusal ships with the surface rather than after it.
pub(crate) const E0272: &str = "E0272";

// E0271 — a `$N` comptime-value argument that is not a compile-time constant — is raised by `jr-db`'s
// comptime-call pre-pass (ADR-0088), not here, because a value's constancy is a const-eval judgement that
// lives downstream of this crate (ADR-0018 §3). It is defined in `jr-db` beside E0230, the way E0245 is,
// and is listed in `AGENTS.md`'s registry as `jr-db`'s. The `$N` *surface* (ADR-0087) raised no code of
// its own beyond recording the call for that pre-pass.

/// A `has_note` or `note_value` note name that is not a string literal (ADR-0099 §1).
///
/// `has_note(f, "inline")` is folded at *check* time — the answer is in the HIR's `Proc::notes`, which is why
/// it needs no VM at all — so the name must be readable then. A computed name would need const-eval, which
/// ADR-0018 §3 puts downstream of this crate: the same narrowing an array length took (ADR-0039 §3a) and an
/// `#insert` operand took before it (ADR-0072), with ADR-0070 §1's widening route available later if a caller
/// ever wants one.
pub(crate) const E0277: &str = "E0277";

/// `==` or `!=` on two `string`s, or on any other aggregate (ADR-0099 §4).
///
/// A `string` is `{data: *u8, count: s64}` (ADR-0004), so the two available meanings are the ones a view's
/// equality has: same storage, or same contents. ADR-0044 §5 refused a *view*'s `==` for exactly that reason
/// and this is the same refusal one type wider — it was **not** refused before, and instead reached MIR as
/// `expected a scalar, found an aggregate`, a leaked internal error for a program a reader would reasonably
/// expect to compile. Comparing contents needs a byte loop, which is `String`'s job in W7.
pub(crate) const E0278: &str = "E0278";

/// A `typed` or `untyped` operand that is not the pointer it needs (ADR-0106 §1).
///
/// `typed(T, p)` needs a **`*u8`** specifically, not any pointer: it exists to give a *fresh allocation* a
/// type, and an allocator hands back bytes. Allowing `*T` → `*U` would be the general pointer cast E0232
/// refuses, reached by another spelling — and E0232's reason is unchanged, that a wrong pointee type is a
/// silent wrong read (ADR-0045 §1). `untyped(p)` needs a pointer at all, since it views one's bytes.
///
/// One code for both, because they are one boundary's two directions and a reader who hits either needs the
/// same page — the argument ADR-0099 made for E0277 covering two refusals.
pub(crate) const E0279: &str = "E0279";

/// An `#align` on a struct field that is not a usable alignment (ADR-0144 §3).
///
/// Four shapes: an operand that is not an integer literal or a literal-valued constant, zero, a
/// value that is not a power of two, and a value above 4096. One code because each is "this is not
/// an alignment", and the *message* says which — the reasoning E0129 and E0130 already use.
///
/// `#align` is a **minimum**, so a value *below* the type's own alignment is not an error: it is
/// already satisfied. That was a decision found while building (ADR-0144 §3): the field's natural
/// alignment is not always knowable while signatures are being resolved — a field whose type is a
/// struct resolved later has no layout yet — so a "must not lower" rule would be enforced sometimes,
/// and a rule enforced sometimes is worse than a rule stated exactly.
///
/// 4096 is a page, and it is the ceiling because a stack slot must be able to honour the request. A
/// request silently not met is worse than a refusal.
pub(crate) const E0282: &str = "E0282";

/// A `#place` on a struct field that is not a usable offset (ADR-0144 §4).
///
/// Two shapes: an operand that is not an integer literal or a literal-valued constant, and a negative
/// value. **Overlap is not an error** and neither is a misaligned offset — the first is the point of
/// the attribute, and the second is handled by every engine because each computes its own addresses.
///
/// Its own code beside E0282 rather than one shared "bad layout attribute", because the two have
/// different rules and a reader filtering by code wants to know which attribute they got wrong.
pub(crate) const E0283: &str = "E0283";

/// An `#soa` struct used in a way it has no meaning for (ADR-0147).
///
/// Three shapes: a count that is not a usable positive array length, a `using` field inside an
/// `#soa` struct (§3), and an index into one that is **not** the receiver of a field access (§2).
///
/// One code because each is "this is not how an `#soa` struct is used", and the *message* says
/// which — the reasoning E0129, E0130 and E0282 already use.
///
/// The third is the interesting one. `e[i]` has no type of its own by design: the alternative is
/// Jai's synthesised struct-of-pointers, which is a real design and a much larger one — it needs a
/// type that exists only as an intermediate, and pointers into N arrays kept consistent. Refusing
/// leaves that available rather than foreclosing it.
pub(crate) const E0284: &str = "E0284";

/// A `#simd` type or operation that a vector cannot have (ADR-0148 §2, §3).
///
/// Three shapes: a total width that is not exactly sixteen bytes, an element that is not a numeric
/// scalar, and integer division. One code, because each is "this is not how a vector works" and the
/// *message* names which — the reasoning E0284 uses for `#soa`'s three, and E0132 for a field
/// attribute's two.
///
/// The width refusal is a **machine fact in the language**, deliberately: a vector operation compiles
/// at exactly 128 bits and nowhere else, which is what probing found before ADR-0148 was written. A
/// portable-looking wider vector would have to be split or scalarised, and ADR-0058 §3's rule — a
/// directive silently ignored is worse than one rejected — applies with unusual force to a construct
/// chosen for speed.
pub(crate) const E0285: &str = "E0285";

/// A type that cannot cross a `#foreign` boundary, in a foreign signature (ADR-0150).
///
/// Raised at the **declaration**, because the signature is what cannot be lowered: a binding that
/// could never be called successfully is itself the error, and a library binding is usually written
/// before its first caller.
///
/// This replaced the **ninth** leaked internal error in this project's most-recorded failure shape.
/// `jr-codegen-llvm`'s signature builder already refused an aggregate here in words, while the
/// Cranelift path simply never declared the procedure — so a legal-looking program produced two
/// different internal errors and no diagnostic.
pub(crate) const E0286: &str = "E0286";

/// The result of a `#must` procedure was discarded (ADR-0151).
///
/// ADR-0008 chose this language's error model — multiple return values plus a `#must` marker — and
/// left the marker unimplemented for the whole programme. This is it: a call whose result is dropped
/// entirely is an error, so ignoring a failure has to be *written* rather than merely done.
///
/// `_ = f();` is accepted, deliberately. An unbypassable check is one people route around with a
/// wrapper procedure, which hides the decision instead of recording it — and the point is visibility,
/// not prohibition. That is the same reasoning `#no_abc` already carries.
pub(crate) const E0287: &str = "E0287";

/// `#must` on a procedure that returns nothing (ADR-0151 §3).
///
/// The marker can never be violated on a `void` procedure and never does anything, so accepting it
/// would leave a reader believing a check is running. ADR-0058 §3's rule about silently-ignored
/// directives, applied to the newest one.
pub(crate) const E0288: &str = "E0288";

/// A call to a `#c_variadic` procedure, which no engine here can make (ADR-0162 §2).
///
/// The **declaration** is legal — that is the whole point of the marker — and the *call* is what cannot be
/// lowered: Cranelift's `Signature` has no notion of a variadic boundary, so every declared parameter is
/// placed by the fixed-arity rules, and on AArch64 a variadic argument belongs on the stack. ADR-0157 §2
/// measured that as a file created with permissions `---------x`: no diagnostic, a plausible-looking result,
/// unreadable.
///
/// So this code exists to turn that silent miscompile into a refusal. It is the same trade ADR-0150 made for
/// an aggregate at a foreign boundary, one wave before that boundary opened.
pub(crate) const E0289: &str = "E0289";

/// An atomic whose pointer is not a `*s64` (ADR-0176 §3).
///
/// **E0291, not E0290**: `jr-hir` owns E0290 for `$$` in a return type (ADR-0168), and the ownership table
/// in `AGENTS.md` is what keeps two crates from minting the same number — a collision `jr-cli`'s
/// `codes.rs` test catches, and did catch while this wave was written.
///
/// Owned by `jr-sema`, continuing this crate's block, because what is wrong is an operand's *type* and a
/// type judgement belongs to the checker.
pub(crate) const E0291: &str = "E0291";

/// A `#system_library` declaration that names no linkable library (ADR-0180 §5).
///
/// Two conditions, one code, because each is one way of the same thing being unaskable: **which library**
/// is this. `#system_library` with no operand, and `#library "x"` — a directive the parser accepts and
/// nothing links, because `foreign_library_of` compares the directive's name against `"system_library"`
/// and returns `None` for anything else.
///
/// **Both type-checked clean and emitted no `-l`.** A symbol then failed at link time with nothing
/// pointing at the cause: `ld: symbol not found`, from a declaration a reader has no reason to doubt. That
/// is a silent-failure shape rather than a wrong answer, and refusing it is the same trade ADR-0289 made
/// for a `#c_variadic` call.
///
/// **E0293, not E0294.** The plan that owed this code assumed Group A would spend E0292 *and* E0293; it
/// spent only E0292, because the second refusal it drafted had no reachable condition (ADR-0179 §4). The
/// number here is what `AGENTS.md`'s table says is free, and `jr-cli`'s `codes.rs` is what makes that
/// claim checkable.
///
/// Owned by `jr-sema`, continuing this crate's block, and raised at the **declaration** rather than at a
/// use: a `#library` nobody calls is still wrong, and reporting per use would say it once per binding.
pub(crate) const E0293: &str = "E0293";

/// An array literal with no elements — `T.[]` (ADR-0194 §2).
///
/// A `[0]T` has no use a caller could name: it cannot be indexed, `size_of` is zero, and a `for` over it
/// runs no iterations — so every operation on one is either an error or a no-op. Refused where it is
/// written rather than allowed and left to surprise whoever measures it.
///
/// **Not folded into E0261**, which is the neighbouring refusal for a literal whose element *type* cannot
/// be resolved: that one means "I do not know what this holds" and this one means "it holds nothing", and
/// a reader chasing the first would look for a misspelled type name.
///
/// Owned by `jr-sema`, continuing this crate's block. E0296 is the first free code.
pub(crate) const E0295: &str = "E0295";
