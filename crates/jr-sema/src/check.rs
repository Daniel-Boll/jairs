//! The check phase: typing expressions, statements, and bodies.
//!
//! # What this phase owns
//!
//! Everything the signature phase does not: procedure bodies, the unnamed
//! file-level items (`#run`), and the `#foreign` library operand. It deliberately
//! does **not** re-type named file-level declarations — those were typed while
//! computing signatures, and typing them twice would either double every
//! diagnostic or, worse, reach a different answer.
//!
//! # Poison propagation is mandatory, not polite
//!
//! `jr_db::file_diagnostics` does not gate later phases on earlier ones: a file
//! that failed to parse is still lowered, resolved, and checked. Without poison
//! propagation every parse error would arrive here as an invented type error, and
//! the recovery quality the parser was built for would be undone by the checker.
//! So [`PoolId::ERROR`] flows through silently, and so do `TypeRef::Error`,
//! `Expr::Error` and `Res::Error`.
//!
//! # Two things the corpus needs that no ADR states
//!
//! - **`string` has `.data` and `.count`.** ADR-0004 fixes the layout as
//!   `{data: *u8, count: s64}` and says the fields are directly accessible;
//!   `valid/021` and `modules/Basic/module.jr` both rely on it. They are treated
//!   as pseudo-fields of the builtin rather than by making `string` a struct,
//!   because ADR-0015 §2 says a user struct of that shape is a *different* type.
//! - **Field access auto-dereferences.** `valid/015` writes `pp := *origin;
//!   pp.x = 1;`, so `.` looks through any number of pointers, and the result is
//!   assignable because a dereference always has an address.

use jr_base::{Interner, Span, Symbol, TextRange};
use jr_diag::{Diagnostic, Diagnostics};
use jr_hir::{
    AssignOp, BinOp, BodyId, Expr, ExprId, ExprScope, FileHir, ItemKind, Literal, ProcId, Res,
    ResolveMap, Stmt, StmtId, TypeRef, TypeRefId, UnOp,
};
use jr_pool::{Item, Pool, PoolId};
use rustc_hash::FxHashMap;

use crate::code::{
    E0204, E0214, E0215, E0216, E0217, E0218, E0219, E0220, E0221, E0222, E0223, E0224, E0225,
    E0232, E0234, E0235, E0236, E0238, E0239, E0241, E0242, E0243, E0244, E0247, E0251, E0252,
    E0254, E0256, E0257, E0258, E0259, E0260, E0261, E0265, E0266, E0267, E0268, E0272, E0277,
    E0278, E0279, E0284, E0285, E0286, E0287, E0288,
};
use crate::ctx::{BodyEnv, Ctx, Mode};
use crate::map::TypeMap;
use crate::sigs::{FileSignatures, ProcSig, SigKind};

/// A compiler intrinsic: a call the compiler recognises by name and types itself.
///
/// None is declared anywhere, which is what `intrinsic_named` tests: a program declaring its own
/// `any_of` keeps it, so the names are not reserved (ADR-0075 §2, ADR-0076 §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Intrinsic {
    /// `type_info(T)` — a `Type_Info` describing `T` (ADR-0075 §2).
    TypeInfo,
    /// `any_of(p)` — an `Any` erasing the pointer `p` (ADR-0076 §1).
    AnyOf,
    /// `any_as(a, T)` — the value in `a`, trapping unless its type is `T` (ADR-0076 §2).
    AnyAs,
    /// `has_note(decl, "name")` — whether that declaration carries `@name` (ADR-0099 §1).
    HasNote,
    /// `note_value(decl, "name")` — the payload of `@name "payload"`, or `""` (ADR-0099 §1).
    NoteValue,
    /// `noted_count("name")` — how many declarations in this file carry `@name` (ADR-0100 §1).
    NotedCount,
    /// `noted_name("name", i)` — the name of the `i`th of them, or `""` (ADR-0100 §1).
    NotedName,
    /// `noted_declarations("name")` — every declaration carrying `@name`, as a `[]Declaration`
    /// (ADR-0153 §1). W6's headline claim: a metaprogram *iterates* rather than unrolling.
    NotedDeclarations,
    /// `noted_insert("name", "template")` — the template once per noted declaration (ADR-0101 §1).
    NotedInsert,
    /// `size_of(T)` — `T`'s size in bytes, folded (ADR-0106 §1).
    SizeOf,
    /// `typed(T, p)` — a `*u8` viewed as a `*T`, the allocation boundary (ADR-0106 §1).
    Typed,
    /// `untyped(p)` — a `*T` viewed as a `*u8`, for releasing it (ADR-0106 §1).
    Untyped,
    /// `view(p, count)` — a `[]T` over `count` elements at `p` (ADR-0109 §1).
    View,
}

/// How a `Type_Info` field's type is checked.
#[derive(Debug, Clone, Copy)]
enum TypeInfoField {
    /// It must be exactly this type.
    Exact(PoolId),
    /// It must be *some* enum, checked by shape because an enum's `PoolId` depends on where it is
    /// declared — `Type_Info_Kind` lives beside `Type_Info` in `Basic`, so the compiler cannot name
    /// its id in advance.
    Enum,
    /// It must be a pointer to *some* struct, checked by shape for the same reason [`TypeInfoField::Enum`]
    /// is: `Any::type` is a `*Type_Info`, and `Type_Info`'s own id depends on its declaration site, so the
    /// compiler cannot name it in advance without looking it up first.
    PointerToStruct,
    /// It must be a `[]T` over *some* struct, checked by shape for the reason above:
    /// `Type_Info.fields` is a `[]Type_Info_Field` and that struct's id depends on its declaration site
    /// (ADR-0152 §3).
    ViewOfStruct,
}

/// The fields the compiler expects `Basic`'s `Type_Info` to have, in order (ADR-0075 §2).
///
/// **This is the contract with `modules/Basic`.** ADR-0075 §2 declares `Type_Info` in Jairs so it is
/// *spellable* — no compiler-declared type is — and this list is what stops that choice from becoming a
/// silent wrong offset: `type_info_struct` checks the declaration against it and raises E0265 on any
/// mismatch, so editing the struct produces a diagnostic rather than a wrong read.
///
/// Keep it in step with `Type_Info` in `modules/Basic/module.jr`.
/// The fields the compiler expects `Basic`'s `Any` to have, in order (ADR-0076 §3).
///
/// The contract with `modules/Basic`, exactly as [`TYPE_INFO_FIELDS`] is — and the second client of that
/// mechanism, which is the first evidence it generalises. `data` is a `*u8` because a pointer's layout
/// does not depend on its pointee, so erasing one loses nothing.
///
/// Keep it in step with `Any` in `modules/Basic/module.jr`.
const ANY_FIELDS: &[(&str, TypeInfoField)] = &[
    ("type", TypeInfoField::PointerToStruct),
    ("data", TypeInfoField::Exact(PoolId::PTR_U8)),
];

/// The fields the compiler expects `Basic`'s `Declaration` to have, in order (ADR-0153 §1).
///
/// The third client of the library-shape contract `TYPE_INFO_FIELDS` established, which is what makes
/// that mechanism worth having rather than a one-off: editing the struct produces a diagnostic instead of
/// a wrong read.
///
/// Keep it in step with `Declaration` in `modules/Basic/module.jr`.
const DECLARATION_FIELDS: &[(&str, TypeInfoField)] = &[
    ("name", TypeInfoField::Exact(PoolId::STRING)),
    ("note_value", TypeInfoField::Exact(PoolId::STRING)),
];

const TYPE_INFO_FIELDS: &[(&str, TypeInfoField)] = &[
    ("id", TypeInfoField::Exact(PoolId::S64)),
    ("kind", TypeInfoField::Enum),
    ("name", TypeInfoField::Exact(PoolId::STRING)),
    ("size", TypeInfoField::Exact(PoolId::S64)),
    ("alignment", TypeInfoField::Exact(PoolId::S64)),
    ("count", TypeInfoField::Exact(PoolId::S64)),
    ("element", TypeInfoField::Exact(PoolId::S64)),
    // The variable-length member ADR-0078 §3 deferred, delivered by ADR-0152 §3. `ViewOfStruct` rather
    // than an `Exact`, because the element is a *declared* struct — `Type_Info_Field` in `Basic` — whose
    // `PoolId` is not a constant this crate can name, exactly as `PointerToStruct` handles `Any.type`.
    ("fields", TypeInfoField::ViewOfStruct),
];

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// One resolved argument position (ADR-0053 §1).
///
/// A `Vec<ArgSlot>` per call replaces the source-order argument list, so `jr-mir` lowers a call
/// without knowing what a parameter name is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgSlot {
    /// An argument the call site wrote, positionally or by name.
    Given(ExprId),
    /// A parameter's default value, already interned (ADR-0053 §2).
    ///
    /// A `PoolId` rather than an `ExprId` because the default belongs to the *declaration*, not to
    /// this call — so there is no expression in this body to point at, and MIR emits it as a
    /// constant operand directly.
    Default(PoolId),
}

/// What the check phase produces.
pub struct CheckOutput {
    /// The type of every expression and local the checker reached.
    pub types: TypeMap,
    /// Diagnostics about bodies, `#run` items, and foreign bindings.
    pub diagnostics: Diagnostics,
    /// Modules a *local* annotation named a type from.
    ///
    /// The signature phase records the same thing for file-level annotations, on
    /// `FileSignatures`. This phase needs its own channel because a local's annotation is
    /// resolved *here* and `FileSignatures` is an input to this phase rather than an
    /// output of it — so a record made on `Ctx::sigs` during a check is discarded when the
    /// context is dropped.
    ///
    /// Which is not hypothetical: `r: Rect;` in
    /// `tests/corpus/imports/valid/001-import-directory-module.jr` is a local, so without
    /// this field ADR-0031 §2's whole point would be defeated for exactly the file that
    /// motivated it.
    pub type_name_imports: Vec<String>,
    /// Which overload each operator expression resolved to (ADR-0048 §5).
    ///
    /// Keyed on `(ExprScope, ExprId)` for the reason `TypeMap` is: an `ExprId` is **not** unique
    /// within a file, because `FileHir::exprs` and every `Body::exprs` start at 0. A bare
    /// `ExprId` key silently collides and the last writer wins, which is a real bug that was
    /// found and fixed in `jr-hir`'s `ResolveMap`.
    ///
    /// The value is `(FileId, ProcId)` rather than a `ProcId`: an imported overload lives in
    /// another file's arena, so the file is what makes the pair a `ProcRef` at lowering time —
    /// the same shape ADR-0018 §5 chose for a cross-file callee.
    ///
    /// Recorded rather than recomputed so that `jr-mir` never re-runs resolution. Two
    /// implementations of one rule are two chances to disagree, which is why `jr-mir` reads
    /// `TypeMap` instead of typing expressions itself.
    pub operator_calls: FxHashMap<(ExprScope, ExprId), (jr_base::FileId, ProcId)>,
    /// The positional argument list of every call that used a named argument or a default
    /// (ADR-0053 §1).
    ///
    /// **Absent for an all-positional call with no defaults**, so the common path pays nothing and
    /// `jr-mir` falls back to the source order — which for such a call is already correct.
    pub filled_calls: FxHashMap<(ExprScope, ExprId), Vec<ArgSlot>>,
    /// The same folded values, keyed by the call's **span** (ADR-0101 §3).
    ///
    /// Load-bearing when a body *expands*: a computed `#insert` renumbers every `ExprId` after the splice, so
    /// a second folded call in one body is at a different id in the expanded tree and its
    /// [`CheckOutput::folded_calls`] entry cannot be found. A span survives expansion — every synthesized
    /// span points at the directive (ADR-0072 §2) — which is why the insert-operand map is span-keyed too.
    pub folded_call_spans: FxHashMap<jr_base::Span, PoolId>,
    /// Each `typed`/`untyped` call and the pointer type it produces (ADR-0106 §1).
    ///
    /// Real code rather than a fold, so it goes to `jr-mir` rather than into `folded_calls`: a pointer's bits
    /// do not depend on its pointee, and retyping is a store-then-load through a slot.
    pub pointer_views: FxHashMap<(ExprScope, ExprId), PoolId>,
    /// Calls that folded to a value **in this crate** — `has_note` and `note_value` (ADR-0099 §2).
    ///
    /// Separate from [`CheckOutput::type_info_calls`], whose meaning is "build a `Type_Info` for this type"
    /// rather than "here is the value". ADR-0076 §2 records what conflating those two cost: a 40-byte
    /// `Type_Info` stored into a 16-byte `Any`, caught only because the sizes happened to differ.
    pub folded_calls: FxHashMap<(ExprScope, ExprId), PoolId>,
    /// The type each `type_info(T)` call describes (ADR-0075 §2).
    ///
    /// Recorded because a *type* is not an operand: nothing in the expression tree carries a `PoolId`,
    /// so lowering could not recover the argument by looking at the call. This is the same reason
    /// `operator_calls` is recorded rather than recomputed — one pass decides, and `jr-mir` reads.
    pub type_info_calls: FxHashMap<(ExprScope, ExprId), PoolId>,
    /// Which `Any` operation each `any_of`/`any_as` call is, and the type it concerns (ADR-0076).
    ///
    /// Separate from [`CheckOutput::type_info_calls`], which means "replace this call with a `Type_Info`
    /// constant". These calls lower to real code — an aggregate build for `any_of`, a compare-and-read for
    /// `any_as` — so sharing one map folded a `Type_Info` into an `Any` and stored 40 bytes into 16.
    pub any_calls: FxHashMap<(ExprScope, ExprId), (AnyOp, PoolId)>,
    /// Each polymorphic call and the instantiation it requires: `(proc, bound type)` (ADR-0082 §1).
    ///
    /// The expansion pass in `jr-db` reads this to append a substituted procedure per distinct key and
    /// rewrite the call to target it. Empty for a file with no polymorphic calls.
    pub instantiations: FxHashMap<(ExprScope, ExprId), (jr_hir::ProcId, Vec<PoolId>)>,
    /// Each comptime-value call and the argument expressions its `$N` parameters need (ADR-0088 §1):
    /// `(proc, [arg ExprId per comptime parameter])`.
    ///
    /// `jr-db`'s `comptime_call_values` pre-pass reads this, evaluates each argument to a constant, and
    /// the instantiation pass appends a clone with those values baked in. Empty for a program with no
    /// comptime-value calls.
    pub comptime_calls: FxHashMap<(ExprScope, ExprId), (jr_hir::ProcId, Vec<jr_hir::ExprId>)>,
    /// Each variadic call, keyed on the call expression (ADR-0138 §2). `fixed_arg_count`
    /// tells MIR how many trailing arguments to pack into a stack view; `element_ty` is the
    /// view's element type — what each trailing argument was checked against. Empty for a
    /// program with no variadic calls.
    pub variadic_calls: FxHashMap<(ExprScope, ExprId), VariadicCall>,
    /// Each `#soa` field access, keyed on the **index** expression that is its receiver, holding the
    /// field's position (ADR-0147 §2).
    ///
    /// `jr-mir` reads this to build `Field(position)` then `Index(i)` for a place whose HIR nests
    /// them the other way round. Recorded rather than recomputed for the reason `operator_calls` is:
    /// one pass decides and `jr-mir` reads, so the two cannot disagree — and a disagreement here is
    /// a wrong *address*, sema typing an element while MIR reads a whole array.
    pub soa_fields: FxHashMap<(ExprScope, ExprId), u32>,
}

/// The information a variadic call needs so MIR can pack the trailing arguments (ADR-0138 §2).
#[derive(Debug, Clone, Copy)]
pub struct VariadicCall {
    /// The number of *fixed* arguments — those that consume the callee's non-variadic
    /// parameters. Trailing args (arg[fixed_arg_count..]) are packed.
    pub fixed_arg_count: usize,
    /// The element type of the variadic view. `PoolId::ERROR` in a poisoned signature.
    pub element_ty: PoolId,
}

/// The last entry of `variadic_params`, if any, is the variadic parameter — the check
/// spelled inline here so callers do not have to re-derive the "only the last is variadic"
/// invariant (ADR-0138 §1).
fn variadic_last_param(variadic_params: &[bool]) -> bool {
    variadic_params.last().copied().unwrap_or(false)
}

/// Which `Any` intrinsic a call is (ADR-0076).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnyOp {
    /// `any_of(p)` — build an `Any` from a pointer, the payload type being the pointee.
    Of,
    /// `any_as(a, T)` — read an `Any` back as `T`, trapping unless its type matches.
    As,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Type-checks one file against its own and its imports' signatures.
///
/// `signatures` must be the output of [`file_signatures`](crate::file_signatures)
/// for this same file, and `imports` must carry one entry per `#import`, keyed by
/// the module name as written. A missing import is not an error here — name
/// resolution already reported it — so its names simply resolve to poison.
pub fn check_file(
    hir: &FileHir,
    file: jr_base::FileId,
    resolve: &ResolveMap,
    signatures: &FileSignatures,
    imports: &[(&str, &FileSignatures)],
    imported_hirs: &[(jr_base::FileId, &FileHir)],
    pool: &mut Pool,
    interner: &Interner,
) -> CheckOutput {
    // Struct field lists live in the pool, keyed by declaration. Recording them
    // explicitly — rather than trusting that some earlier phase interned them —
    // is what keeps this function callable on its own in a test.
    signatures.record_in(pool);
    for (_, imported) in imports {
        imported.record_in(pool);
    }

    let mut ctx = Ctx::new(
        hir,
        file,
        resolve,
        interner,
        pool,
        imports.to_vec(),
        imported_hirs.to_vec(),
        Mode::Check,
    );
    ctx.sigs = signatures.clone();
    ctx.sigs.set_file(file);

    // Unnamed items. A named item's initialiser was typed by the signature
    // phase; a top-level `#run` has no name and so has no signature.
    for index in 0..hir.items.len() {
        let item = hir.item(jr_hir::ItemId::from_usize(index));
        if let ItemKind::Run { expr } = item.kind {
            ctx.check_expr(ExprScope::TopLevel, expr, None);
        }
    }

    // `Body` has no back-pointer to its `Proc`, so the mapping is recovered by
    // scanning. Without it a `Res::Param` could not be typed at all.
    let mut owner: FxHashMap<BodyId, ProcId> = FxHashMap::default();
    for (index, proc) in hir.procs.iter().enumerate() {
        if let Some(body) = proc.body {
            owner.insert(body, ProcId::from_usize(index));
        }
    }

    for index in 0..hir.procs.len() {
        ctx.check_foreign_binding(ProcId::from_usize(index));
        ctx.check_foreign_signature(ProcId::from_usize(index));
        ctx.check_must_returns_something(ProcId::from_usize(index));
    }

    for index in 0..hir.bodies.len() {
        let body = BodyId::from_usize(index);
        let sig = owner
            .get(&body)
            .and_then(|proc| ctx.sigs.proc_sig(*proc))
            .cloned();
        let (params, ret) = match sig {
            Some(sig) => (sig.params, sig.ret),
            // A body whose procedure has no signature only happens after an
            // error; poison rather than guessing `void`, which would make every
            // `return x;` in it wrong.
            None => (Vec::new(), PoolId::ERROR),
        };
        ctx.body = Some(BodyEnv {
            id: body,
            params,
            ret,
        });
        // **Re-seed this body's comptime-value bindings** (ADR-0089 §1). An instantiation's `$N`
        // parameters keep their baked values while its body is checked, so a local `buf: [N]s64` resolves
        // its length. Re-seeded *per body* rather than left over from the signature phase, because two
        // instantiations of one template share the parameter name `N` with different values — leaving the
        // last one set would give the second instantiation's length to the first's body, a silent wrong
        // array size. Cleared afterwards for the same reason.
        ctx.value_bindings.clear();
        ctx.comptime_param_names.clear();
        ctx.type_bindings.clear();
        ctx.poly_var_names.clear();
        if let Some(proc) = owner.get(&body) {
            for (p, name, value) in &hir.param_values {
                if p == proc {
                    ctx.value_bindings.insert(*name, *value);
                }
            }
            // **This instantiation's bound type variables** (ADR-0092 §1), so `type_info(T)` inside its
            // body describes the bound type rather than hunting a declaration named `T`. Seeded per body
            // exactly as `value_bindings` is, and for the same reason: two instantiations of one template
            // share the variable name `T` with different bindings, so leaving one set would describe the
            // wrong type in the other's body — a silently wrong `size`, which is worse than an error.
            for (p, var, ty) in &hir.proc_bindings {
                if p == proc {
                    ctx.type_bindings.insert(*var, *ty);
                }
            }
            // The `$T` variable *names* this procedure introduces, bound or not (ADR-0092 §1). A template
            // has names and no bindings, which is exactly where `type_info(T)` must be withheld rather
            // than refused.
            if let Some(sig) = ctx.sigs.proc_sig(*proc) {
                let vars = sig.poly_vars.clone();
                for var in vars {
                    ctx.poly_var_names.insert(var);
                }
            }
            // **A `#modify` predicate names its guarded template's `$T`** (ADR-0094 §1). The predicate is a
            // synthetic no-parameter procedure, so it has no `poly_vars` of its own — but its body says
            // `type_info(T)`, where `T` is the template's variable. Without this, checking the *template's*
            // predicate reported E0261 "needs a type", because nothing said `T` was a variable awaiting a
            // binding rather than a name resolving to nothing. A predicate *clone* gets real bindings from
            // `proc_bindings` above; this covers the template's own copy, which is checked and never run.
            for (pred, vars) in &hir.predicate_vars {
                if pred == proc {
                    for var in vars {
                        ctx.poly_var_names.insert(*var);
                    }
                }
            }
            // The comptime parameter *names* of this body's procedure, so a template's own body withholds
            // E0233 for a length naming one rather than refusing a correct program (ADR-0089 §2).
            for param in hir.proc(*proc).params.iter().filter(|p| p.comptime) {
                ctx.comptime_param_names.insert(param.name);
            }
        }
        let root = hir.body(body).root;
        // **Watermark, then stamp** (ADR-0128 §3). Every diagnostic this body produces is stamped with
        // the instantiation backtrace afterwards, rather than each of the checker's hundreds of `push`
        // sites learning about polymorphism — which is the only version of this that cannot be forgotten
        // by the next diagnostic somebody adds.
        let watermark = ctx.diags.len();
        ctx.check_stmt(body, root);
        if let Some(proc) = owner.get(&body) {
            let frames = instantiation_backtrace(hir, &owner, *proc);
            ctx.diags.attach_frames_since(watermark, &frames);
        }
        ctx.body = None;
        ctx.value_bindings.clear();
        ctx.comptime_param_names.clear();
        ctx.type_bindings.clear();
        ctx.poly_var_names.clear();
    }

    // Collected before `ctx.sigs` is dropped. It started as a clone of the file's
    // signatures, so an entry the *signature* phase recorded is in here too; the union is
    // taken by the consumer rather than filtered here, because a module named in both
    // positions is used either way.
    let type_name_imports: Vec<String> = ctx
        .sigs
        .modules_used_in_type_position()
        .map(ToOwned::to_owned)
        .collect();

    CheckOutput {
        types: ctx.types,
        diagnostics: ctx.diags,
        type_name_imports,
        operator_calls: ctx.operator_calls,
        filled_calls: ctx.filled_calls,
        folded_calls: ctx.folded_calls,
        pointer_views: ctx.pointer_views,
        folded_call_spans: ctx.folded_call_spans,
        type_info_calls: ctx.type_info_calls,
        any_calls: ctx.any_calls,
        instantiations: ctx.instantiations,
        comptime_calls: ctx.comptime_calls,
        variadic_calls: ctx.variadic_calls,
        soa_fields: ctx.soa_fields,
    }
}

/// The instantiation backtrace for a body's procedure, innermost frame first.
///
/// Empty for an ordinary procedure, which is the common case and costs one failed map lookup.
///
/// # Why it walks, rather than reporting one frame
///
/// A template that calls a template produces a clone whose body calls another clone, so a diagnostic in
/// the innermost one is only explicable by the whole chain: `main` demanded `outer($T = bool)`, whose
/// body demanded `inner($T = bool)`, and the error is in `inner`. Each site records the *arena* its
/// demanding call sat in, so `owner` turns that into the enclosing procedure and the walk continues while
/// that procedure is itself an instantiation.
///
/// Innermost first because that is the order [`jr_diag`]'s renderer prints, and it matches how a reader
/// reads an error: the thing that broke, then why it was asked for.
///
/// **Bounded**, like every other fixed-point walk in this compiler (`MAX_OPT_ROUNDS`,
/// `MAX_INSTANTIATION_ROUNDS`): a recursive template could otherwise produce a cycle, and a diagnostic
/// path is the worst place to hang. A truncated backtrace is still useful; a hung `jr check` is not.
fn instantiation_backtrace(
    hir: &FileHir,
    owner: &FxHashMap<BodyId, ProcId>,
    proc: ProcId,
) -> Vec<jr_diag::InstantiationFrame> {
    /// Enough to explain any chain a person wrote, and short enough that a cycle costs nothing.
    const MAX_BACKTRACE_FRAMES: usize = 8;

    let mut frames = Vec::new();
    let mut current = proc;
    for _ in 0..MAX_BACKTRACE_FRAMES {
        let Some((_, site)) = hir.instantiation_sites.iter().find(|(p, _)| *p == current) else {
            break;
        };
        frames.push(site.frame.clone());
        // Continue only while the demanding call sat in a body whose procedure is itself an
        // instantiation; a call from an ordinary procedure ends the chain, which is where the user's
        // own code begins.
        let Some(ExprScope::Body(body)) = site.called_from else {
            break;
        };
        let Some(next) = owner.get(&body) else {
            break;
        };
        if *next == current {
            break;
        }
        current = *next;
    }
    frames
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    /// Checks one statement of a body.
    pub(crate) fn check_stmt(&mut self, body: BodyId, stmt: StmtId) {
        let scope = ExprScope::Body(body);
        let hir = self.hir;
        let statement = hir
            .body(body)
            .stmts
            .get(stmt.index())
            .cloned()
            .unwrap_or(Stmt::Error(self.nowhere()));

        match statement {
            Stmt::Block(stmts, _) => {
                for inner in stmts {
                    self.check_stmt(body, inner);
                }
            }
            Stmt::Local(local, _) => self.check_local(body, local),
            Stmt::LocalTuple {
                targets,
                call,
                span,
            } => self.check_local_tuple(body, &targets, call, span),
            Stmt::AssignTuple {
                targets,
                call,
                span,
            } => self.check_assign_tuple(scope, &targets, call, span),
            // Declared but never constructed by lowering; a nested item would be
            // E0207 long before it reached here.
            Stmt::Item(_, _) => {}
            // **`_ = f();`** — the result is received and thrown away, on purpose (ADR-0151 §2). Checked
            // exactly like a statement expression *except* that `#must` is satisfied: that is the whole
            // difference between the two statements, and it is why they are two statements.
            Stmt::Discard { value, .. } => {
                self.check_expr(scope, value, None);
            }
            Stmt::Expr(expr, _) => {
                // A discarded result is fine — `zero();` is a statement in
                // `valid/017` — so there is no expectation to impose.
                self.check_expr(scope, expr, None);
                // **Unless the callee is `#must`** (ADR-0151 §1). This is the *only* place a result is
                // dropped entirely: every other position — an initialiser, an argument, an operand, a
                // `return`, a target list — receives it. So one check here covers the whole language,
                // and `_ = f();` passes because a target list is a reception.
                self.check_must_received(scope, expr);
            }
            Stmt::Assign { lhs, op, rhs, span } => self.check_assign(scope, lhs, op, rhs, span),
            Stmt::If {
                cond, then, else_, ..
            } => {
                self.check_condition(scope, cond, "if");
                self.check_stmt(body, then);
                if let Some(branch) = else_ {
                    self.check_stmt(body, branch);
                }
            }
            Stmt::While {
                cond,
                body: loop_body,
                ..
            } => {
                self.check_condition(scope, cond, "while");
                self.check_stmt(body, loop_body);
            }
            Stmt::Return(expr, span) => self.check_return(scope, expr, span),
            Stmt::ReturnTuple(exprs, span) => self.check_return_tuple(scope, &exprs, span),
            Stmt::For {
                value,
                index,
                iterable,
                reverse: _,
                body: loop_body,
                label: _,
                span,
            } => self.check_for(body, value, index, &iterable, loop_body, span),
            // The deferred statement is checked once, where it was written. `jr-mir` duplicates
            // its lowering across exit paths, not its typing (ADR-0049 §3) — so a type error in a
            // `defer` is reported once rather than once per way out of the scope.
            Stmt::Defer(inner, _) => self.check_stmt(body, inner),
            // `push_context { … }` copies the context on entry (ADR-0063), so it needs one to copy.
            // A `#c_call` procedure has none, and this is the same refusal as `context` itself there
            // — E0254, reused because it means exactly "this needs a context and there isn't one"
            // (ADR-0063 §4). The message names `push_context` so the diagnostic points at what was
            // written. The block is checked regardless, so a body error inside it is still reported.
            Stmt::Switch { value, arms, span } => self.check_switch(body, value, &arms, span),
            // An `#insert`'s statements are checked **as if written here** (ADR-0072 §1) — no scope, no
            // separate environment, so a local the insert declares is in `self.locals` for the statements
            // after it. Nothing here can tell they came from a string, which is the evidence lowering put
            // them in the enclosing body rather than in a nested one.
            Stmt::Insert {
                stmts,
                operand,
                span: _,
            } => {
                // A **computed** operand is checked as an expression **expecting `string`** (ADR-0073 §1),
                // so a non-`string` operand is an ordinary type mismatch at its own span rather than a
                // bespoke refusal. Nothing here evaluates it — that is the operand pre-pass's job — but
                // checking it means the error a reader sees is about *their* expression. `None` for a
                // literal insert, whose text is already lowered into `stmts`.
                if let Some(op) = operand {
                    self.check_expr(scope, op, Some(PoolId::STRING));
                }
                for inner in stmts {
                    self.check_stmt(body, inner);
                }
            }
            Stmt::PushContext(inner, span) => {
                if self.body_is_c_call(body) {
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            "`push_context` is not available in a `#c_call` procedure",
                        )
                        .with_code(E0254)
                        .with_note(
                            "a `#c_call` procedure receives no implicit context to copy (ADR-0057 §3)",
                        )
                        .with_help("remove the `#c_call`, or manage the resource explicitly"),
                    );
                }
                self.check_stmt(body, inner);
            }
            // A label names a *loop*, not a value, so there is nothing to type. Whether the label
            // exists is `jr-mir`'s question, because its loop stack is the only place a loop's
            // identity lives (ADR-0049 §2).
            Stmt::Break(_, _) | Stmt::Continue(_, _) | Stmt::Error(_) => {}
        }
    }

    /// Types a `for` loop and records its variables' types (ADR-0049 §1).
    ///
    /// Three iterable shapes and no more: an array, a view, or a range. The *element* type is what
    /// the value variable gets; the index variable is always `s64`, because that is the type
    /// `.count` has (ADR-0004) and an index that disagreed with the length would need a conversion
    /// to compare with it.
    fn check_for(
        &mut self,
        body: BodyId,
        value: jr_hir::LocalId,
        index: Option<jr_hir::LocalId>,
        iterable: &jr_hir::ForIterable,
        loop_body: jr_hir::StmtId,
        span: Span,
    ) {
        let scope = ExprScope::Body(body);
        let element = match iterable {
            jr_hir::ForIterable::Sequence(expr) => {
                let mut seq = self.check_expr(scope, *expr, None);
                // Auto-deref, matching `check_index` and `check_slice`: `p: *[4]u8` iterates
                // through the pointer. The same loop in all three, so they cannot disagree about
                // how many levels.
                while let Some(inner) = self.pointee(seq) {
                    seq = inner;
                }
                match self.pool.item(seq) {
                    Item::ArrayType { elem, .. } | Item::ViewType { elem } => *elem,
                    _ => {
                        if seq != PoolId::ERROR {
                            let text = self.describe(seq);
                            self.diags.push(
                                Diagnostic::error(
                                    span,
                                    format!("cannot iterate over a value of type `{text}`"),
                                )
                                .with_code(E0247)
                                .with_note(
                                    "a `for` iterates a fixed-size array `[N]T`, a view `[]T`,                                      or a range `a..b`",
                                )
                                .with_help(
                                    // **Not "wave W5's macros unlock it".** W5 is complete and
                                    // `#expand` macros ship (ADR-0090, ADR-0091), so the stated
                                    // blocker is gone while the feature is still absent — an
                                    // expired reason reads as a considered decision.
                                    "a user type cannot be iterated: the macros such a protocol \
                                     would be built on exist (ADR-0091), but no iteration \
                                     protocol is defined and no wave owns one",
                                ),
                            );
                        }
                        PoolId::ERROR
                    }
                }
            }
            // Both ends are context-typed as `s64`, which is what makes `for i: 0..n` an `s64`
            // loop rather than an unconstrained one — the same context ADR-0039 §5 gives an index.
            jr_hir::ForIterable::Range { start, end } => {
                let s = self.check_expr(scope, *start, Some(PoolId::S64));
                let e = self.check_expr(scope, *end, Some(PoolId::S64));
                // An end that is not an integer is the mistake worth naming: `0..buf` reads as an
                // iteration and is a range over something with no ordering.
                for (ty, which) in [(s, "start"), (e, "end")] {
                    if ty != PoolId::ERROR && self.int_info(ty).is_none() {
                        let text = self.describe(ty);
                        self.diags.push(
                            Diagnostic::error(
                                span,
                                format!("the {which} of a range must be an integer, not `{text}`"),
                            )
                            .with_code(E0247),
                        );
                    }
                }
                PoolId::S64
            }
        };

        self.locals.insert((body, value), element);
        self.types.set_local(body, value, element);
        if let Some(index) = index {
            self.locals.insert((body, index), PoolId::S64);
            self.types.set_local(body, index, PoolId::S64);
        }
        self.check_stmt(body, loop_body);
    }

    /// Checks a local declaration and records the local's type.
    /// Checks `q, ok := f();` (ADR-0052 §2).
    ///
    /// Each target's type is the corresponding *result* type, so the locals are typed from the
    /// call rather than from an annotation — a destructuring declaration has no place to write one.
    fn check_local_tuple(
        &mut self,
        body: BodyId,
        targets: &[Option<jr_hir::LocalId>],
        call: ExprId,
        span: Span,
    ) {
        let scope = ExprScope::Body(body);
        let results = self.destructured_results(scope, call, targets.len(), span);
        for (index, target) in targets.iter().enumerate() {
            // A discard is typed as nothing, because it declares nothing.
            let Some(local) = target else { continue };
            let ty = results.get(index).copied().unwrap_or(PoolId::ERROR);
            self.locals.insert((body, *local), ty);
            self.types.set_local(body, *local, ty);
        }
    }

    /// Checks `q, ok = f();` (ADR-0052 §2).
    ///
    /// Each present target must be an assignable place whose type accepts the matching result, so
    /// this reuses `expect` and `is_place` rather than inventing a second assignability rule — two
    /// rules would be two chances to disagree about what `=` means.
    fn check_assign_tuple(
        &mut self,
        scope: ExprScope,
        targets: &[Option<ExprId>],
        call: ExprId,
        span: Span,
    ) {
        let results = self.destructured_results(scope, call, targets.len(), span);
        for (index, target) in targets.iter().enumerate() {
            let Some(target) = target else { continue };
            let target_ty = self.check_expr(scope, *target, None);
            if !self.is_place(scope, *target) {
                let text = self.describe(target_ty);
                self.diags.push(
                    Diagnostic::error(span, format!("cannot assign to this `{text}` target"))
                        .with_code(E0251)
                        .with_note("each target of a destructuring assignment must be assignable"),
                );
                continue;
            }
            if let Some(result) = results.get(index).copied() {
                self.expect(Some(target_ty), result, span);
            }
        }
    }

    /// The result types a destructuring statement's right-hand side produces, checking arity.
    ///
    /// Returns one type per *target* so the caller can index it positionally; a mismatch yields
    /// `PoolId::ERROR` entries, which propagate without inventing a second diagnostic per target.
    ///
    /// This is the one place arity is decided (ADR-0052 §2). Both statement forms ask it, so they
    /// cannot disagree about how many results a call has.
    fn destructured_results(
        &mut self,
        scope: ExprScope,
        call: ExprId,
        want: usize,
        span: Span,
    ) -> Vec<PoolId> {
        let ty = self.check_expr(scope, call, None);
        if ty == PoolId::ERROR {
            return vec![PoolId::ERROR; want];
        }
        let Some(elems) = self.pool.results_elems(ty).map(<[PoolId]>::to_vec) else {
            // One result, or none: a destructuring statement is the wrong form. Named precisely,
            // because "expected 2 values" without saying what it *does* return sends the reader
            // looking for a call site problem.
            let text = self.describe(ty);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("this call returns one value of type `{text}`, not {want}"),
                )
                .with_code(E0251)
                .with_note("a destructuring statement needs a procedure returning several values")
                .with_note("for a single result, write `x := f();`"),
            );
            return vec![PoolId::ERROR; want];
        };
        if elems.len() != want {
            let text = self.describe(ty);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "this call returns {} values, but {want} {} named",
                        elems.len(),
                        if want == 1 { "is" } else { "are" }
                    ),
                )
                .with_code(E0251)
                .with_note(format!("it returns `{text}`"))
                .with_note(
                    "the counts must match exactly; write `_` to discard a result you do not want",
                ),
            );
            return vec![PoolId::ERROR; want];
        }
        elems
    }

    fn check_local(&mut self, body: BodyId, local: jr_hir::LocalId) {
        let scope = ExprScope::Body(body);
        let hir = self.hir;
        let declaration = hir.body(body).local(local).clone();

        // A local's annotation is the one type reference that lives in
        // `Body::type_refs` rather than `FileHir::type_refs`.
        let declared = declaration
            .ty
            .map(|id| self.resolve_type(scope, id, declaration.name_span));

        let ty = match (declared, declaration.init) {
            (Some(annotation), Some(init)) => {
                self.check_expr(scope, init, Some(annotation));
                annotation
            }
            (Some(annotation), None) => annotation,
            (None, Some(init)) => {
                let inferred = self.check_expr(scope, init, None);
                self.reject_void_binding(inferred, declaration.span)
            }
            (None, None) => PoolId::ERROR,
        };

        self.locals.insert((body, local), ty);
        self.types.set_local(body, local, ty);
    }

    /// Checks an assignment.
    fn check_assign(
        &mut self,
        scope: ExprScope,
        lhs: ExprId,
        op: AssignOp,
        rhs: ExprId,
        span: Span,
    ) {
        // Type the target first: `is_place` consults the receiver's type when
        // deciding whether a field access auto-dereferences.
        let target = self.check_expr(scope, lhs, None);
        if !self.is_place(scope, lhs) {
            self.diags.push(
                Diagnostic::error(span, "cannot assign to this expression")
                    .with_code(E0220)
                    .with_note("only variables, fields, and dereferences can be assigned to")
                    .with_help("a `::` declaration is a constant; use `:=` or `: T` for something assignable"),
            );
        }

        let compound = match op {
            AssignOp::Assign => false,
            AssignOp::AddAssign
            | AssignOp::SubAssign
            | AssignOp::MulAssign
            | AssignOp::DivAssign
            | AssignOp::RemAssign
            | AssignOp::WrapAddAssign
            | AssignOp::WrapSubAssign
            | AssignOp::WrapMulAssign
            | AssignOp::BitAndAssign
            | AssignOp::BitOrAssign
            | AssignOp::BitXorAssign
            | AssignOp::ShlAssign
            | AssignOp::ShrAssign => true,
        };

        if compound && target != PoolId::ERROR && self.int_info(target).is_none() {
            let text = self.describe(target);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("compound assignment is not supported for `{text}`"),
                )
                .with_code(E0223),
            );
        }

        self.check_expr(scope, rhs, Some(target));
    }

    /// Checks the condition of an `if` or `while`.
    fn check_condition(&mut self, scope: ExprScope, cond: ExprId, keyword: &str) {
        // Checked without an expectation so that the diagnostic can be about the
        // condition rather than a generic mismatch.
        let ty = self.check_expr(scope, cond, None);
        if ty == PoolId::ERROR || ty == PoolId::BOOL {
            return;
        }
        // The condition's own span, not the statement's: pointing at three lines
        // of `if … { … }` to say "this is not a bool" is not pointing at anything.
        let span = self.expr_of(scope, cond).span();
        let text = self.describe(ty);
        self.diags.push(
            Diagnostic::error(
                span,
                format!("the condition of `{keyword}` must be `bool`, found `{text}`"),
            )
            .with_code(E0222)
            .with_note("Jairs has no implicit conversion to `bool`"),
        );
    }

    /// Checks a `return`.
    ///
    /// Whether every path through a non-`void` procedure actually returns is a
    /// control-flow question, not a typing one, and is not answered here.
    /// Checks `return a, b;` against the procedure's declared results (ADR-0052 §1).
    ///
    /// Each expression is checked against its *positional* result type, so a mismatch names the
    /// position rather than the whole tuple — which is what makes a two-result procedure returning
    /// `(bool, s64)` by mistake report the swap rather than "expected `(s64, bool)`".
    fn check_return_tuple(&mut self, scope: ExprScope, exprs: &[ExprId], span: Span) {
        let ret = self.body.as_ref().map_or(PoolId::ERROR, |body| body.ret);
        let Some(elems) = self.pool.results_elems(ret).map(<[PoolId]>::to_vec) else {
            // The procedure declares one result (or none) and this `return` gives several. Checked
            // here rather than left to `expect`, because a results aggregate has no type to unify
            // with a scalar and the generic mismatch would name an internal type.
            for expr in exprs {
                self.check_expr(scope, *expr, None);
            }
            let text = self.describe(ret);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "this `return` gives {} values, but the procedure returns `{text}`",
                        exprs.len()
                    ),
                )
                .with_code(E0251)
                .with_note("declare several results as `-> (T, U)` to return several values"),
            );
            return;
        };
        if elems.len() != exprs.len() {
            for expr in exprs {
                self.check_expr(scope, *expr, None);
            }
            let text = self.describe(ret);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "this `return` gives {} values, but `{text}` declares {}",
                        exprs.len(),
                        elems.len()
                    ),
                )
                .with_code(E0251),
            );
            return;
        }
        for (expr, want) in exprs.iter().zip(elems) {
            self.check_expr(scope, *expr, Some(want));
        }
    }

    fn check_return(&mut self, scope: ExprScope, expr: Option<ExprId>, span: Span) {
        let ret = self.body.as_ref().map_or(PoolId::ERROR, |body| body.ret);
        match expr {
            Some(value) => {
                if ret == PoolId::VOID {
                    self.check_expr(scope, value, None);
                    self.diags.push(
                        Diagnostic::error(span, "this procedure returns nothing")
                            .with_code(E0224)
                            .with_help("write `return;`, or give the procedure a `-> T`"),
                    );
                } else {
                    self.check_expr(scope, value, Some(ret));
                }
            }
            None => {
                if ret != PoolId::VOID && ret != PoolId::ERROR {
                    let text = self.describe(ret);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!(
                                "this procedure returns `{text}`, but this `return` has no value"
                            ),
                        )
                        .with_code(E0224),
                    );
                }
            }
        }
    }

    /// Rejects binding the result of a procedure that returns nothing.
    ///
    /// ADR-0016 §2. The alternative — a `void`-typed local — costs one comparison
    /// here and propagates meaningless locals into MIR, the mid-end, and both
    /// backends forever.
    pub(crate) fn reject_void_binding(&mut self, ty: PoolId, span: Span) -> PoolId {
        // **A results aggregate is not storable** (ADR-0052 §4). `q := divide(7, 2)` binds *the
        // whole aggregate*, which would make a results type spellable as a variable's type through
        // the back door — and every tuple question ADR-0052 §1 declined to answer would follow.
        // Refused here because this is the one place a binding's inferred type is judged, so the
        // same rule covers a local, a `:=` and anything else that infers.
        if self.pool.results_elems(ty).is_some() {
            let text = self.describe(ty);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("cannot bind `{text}`: a multi-result call needs one name per result"),
                )
                .with_code(E0251)
                .with_note("several results are not a value, so there is nothing to store")
                .with_help("write `a, b := f();`, naming every result, or `_` to discard one"),
            );
            return PoolId::ERROR;
        }
        if ty != PoolId::VOID {
            return ty;
        }
        self.diags.push(
            Diagnostic::error(
                span,
                "cannot bind the result of a procedure that returns nothing",
            )
            .with_code(E0217)
            .with_note("the call has no value to bind")
            .with_help("call it as a statement instead, without the `:=`"),
        );
        PoolId::ERROR
    }

    /// Refuses `#must` on a procedure with nothing to return (ADR-0151 §3).
    ///
    /// A `void` result cannot be received, so the attribute can never be violated and never does
    /// anything. ADR-0058 §3's rule — a directive silently ignored is worse than one rejected — applies
    /// exactly: a reader who wrote it believes a check is running.
    ///
    /// Reported at the **declaration**, which is the mistake's own site. The call-site check stays
    /// silent for a `void` callee so that one error is not reported at every call as well.
    fn check_must_returns_something(&mut self, proc: ProcId) {
        if !self.hir.proc(proc).must {
            return;
        }
        let Some(sig) = self.sigs.proc_sig(proc) else {
            return;
        };
        if sig.ret != PoolId::VOID {
            return;
        }
        let span = self.hir.proc(proc).span;
        self.diags.push(
            Diagnostic::error(
                span,
                "`#must` on a procedure that returns nothing".to_owned(),
            )
            .with_code(E0288)
            .with_note(
                "there is no result to receive, so the marker can never be violated and never \
                     does anything",
            )
            .with_help("give it a result to return, or drop the `#must`"),
        );
    }

    /// Refuses discarding the result of a `#must` call (ADR-0151 §1).
    ///
    /// Called only from the `Stmt::Expr` arm, which is the one position in the language where a value
    /// is produced and dropped. That is what makes this a single check rather than a rule every
    /// expression position has to remember — and it is why `_ = f();` is accepted without a special
    /// case: an assignment to a target list is a *reception*, and it never reaches here.
    ///
    /// # What counts as the call
    ///
    /// The statement's expression itself, not a call nested inside it. `g(f());` receives `f`'s result
    /// as an argument, so `f` is satisfied; whether `g`'s own result may be dropped is `g`'s question,
    /// and it is the expression this sees.
    fn check_must_received(&mut self, scope: ExprScope, expr: ExprId) {
        let Expr::Call { callee, span, .. } = self.expr_of(scope, expr) else {
            return;
        };
        // **Asked of the callee's *type***, not of a `ProcId` (ADR-0151 §1). A call may cross a module
        // boundary, where this file has no HIR for the declaration — and it may go through a procedure
        // *pointer*, which has no declaration at all. The type carries the obligation, so both work
        // here with no extra lookup.
        let callee_ty = self.types.expr_type(scope, callee).unwrap_or(PoolId::ERROR);
        let Some(effects) = self.pool.proc_effects(callee_ty) else {
            return;
        };
        if !effects.must {
            return;
        }
        // A `void` result has nothing to receive. `#must` on such a procedure is meaningless rather
        // than violated, and refusing the *call* would be reporting the declaration's mistake at the
        // wrong place — E0288 reports it at the declaration instead.
        let Item::ProcType { ret, .. } = self.pool.item(callee_ty) else {
            return;
        };
        if *ret == PoolId::VOID {
            return;
        }
        // Named when the callee is a plain name, which covers every ordinary call; a call through a
        // pointer or a field says "this call" rather than inventing a name for something unnamed.
        let name = match self.expr_of(scope, callee) {
            Expr::Name { name, .. } => format!("`{}`", self.interner.resolve(name)),
            _ => "this call".to_owned(),
        };
        self.diags.push(
            Diagnostic::error(
                span,
                format!("the result of {name} must be received: it is `#must`"),
            )
            .with_code(E0287)
            .with_note(
                "`#must` marks a result a caller has to look at — typically a success flag beside a \
                 value, which is this language's error model (ADR-0008)",
            )
            .with_help(
                "receive it — `ok := …` or `ok, value := …` — or write `_ = …` to say the failure is \
                 deliberately ignored",
            ),
        );
    }

    /// Refuses a `#foreign` signature carrying a type that cannot cross a C boundary (ADR-0150).
    ///
    /// # Why at the declaration rather than the call
    ///
    /// The signature is what cannot be lowered, so a declaration that could never be called
    /// successfully *is* the error. Refusing at the call would report the same fact once per call site
    /// and say nothing about a declaration nobody calls yet — and a library binding is usually written
    /// before its first caller.
    ///
    /// # What this replaced
    ///
    /// A leaked internal error, and the **ninth** occurrence of this project's most-recorded failure
    /// shape. Calling such a procedure gave `procedure 0 in file 0 was defined without being declared`
    /// from Cranelift and `no routine for file 0 proc 0` from the VM — two different internal errors
    /// for one legal-looking program, on a declaration that checked clean. `jr-codegen-llvm`'s
    /// signature builder already refused it *in words*; this raises that refusal to where it can carry
    /// a span and name the workaround.
    fn check_foreign_signature(&mut self, proc: ProcId) {
        let hir = self.hir;
        let Some(info) = hir.proc(proc).foreign.clone() else {
            return;
        };
        // The signature is resolved by now: `check` runs after `file_signatures`.
        let Some(sig) = self.sigs.proc_sig(proc) else {
            return;
        };
        let params: Vec<PoolId> = sig.params.clone();
        let ret = sig.ret;

        for (index, ty) in params.iter().enumerate() {
            if let Some(reason) = self.foreign_boundary_refusal(*ty) {
                let name = self
                    .sigs
                    .proc_sig(proc)
                    .and_then(|s| s.names.get(index).copied());
                let described = name.map_or_else(
                    || format!("parameter {}", index + 1),
                    |sym| format!("`{}`", self.interner.resolve(sym)),
                );
                let text = self.describe(*ty);
                self.diags.push(
                    Diagnostic::error(
                        info.span,
                        format!("{described} cannot cross a `#foreign` boundary: it is `{text}`"),
                    )
                    .with_code(E0286)
                    .with_note(reason)
                    .with_help(
                        "pass a pointer instead — `*T` is one register, and the callee reads through \
                         it",
                    ),
                );
            }
        }

        if let Some(reason) = self.foreign_boundary_refusal(ret) {
            let text = self.describe(ret);
            self.diags.push(
                Diagnostic::error(
                    info.span,
                    format!("a `#foreign` procedure cannot return `{text}`"),
                )
                .with_code(E0286)
                .with_note(reason)
                .with_help(
                    "return a pointer, or take a `*T` out-parameter and write through it — which is \
                     what the C signature would do anyway",
                ),
            );
        }
    }

    /// Why `ty` cannot cross a `#foreign` boundary, or `None` if it can (ADR-0150 §1).
    ///
    /// Exhaustive over the pool item rather than a `matches!`, so a new type is a compile error here
    /// instead of silently becoming passable — which is the discipline that would have caught this gap
    /// when `#simd` added a type two waves ago.
    fn foreign_boundary_refusal(&self, ty: PoolId) -> Option<&'static str> {
        match self.pool.item(ty) {
            // Passable: one register each, and the C ABI agrees about all of them.
            Item::VoidType
            | Item::BoolType
            | Item::IntType { .. }
            | Item::FloatType { .. }
            | Item::PointerType(_)
            | Item::EnumType { .. }
            | Item::ProcType { .. }
            // Poison is already reported; refusing it again would double-report one mistake.
            | Item::ErrorType => None,

            // `string` is `{data, count}` (ADR-0004) — two words, and C has no such type. This is the
            // aggregate a caller is most likely to try, which is why it gets its own sentence.
            Item::StringType => Some(
                "a `string` is a pointer and a count, and C has no such type: pass `s.data` and \
                 `s.count` as two arguments",
            ),
            Item::StructType { .. } | Item::UnionType { .. } | Item::VariantType { .. } => Some(
                "passing an aggregate by value needs the platform ABI's field-classification rules, \
                 which no engine here implements yet",
            ),
            Item::ArrayType { .. } => Some(
                "an array is its elements laid out in memory; C decays one to a pointer and Jairs \
                 does not",
            ),
            Item::ViewType { .. } | Item::DynamicArrayType { .. } => Some(
                "a view and a dynamic array are multi-word descriptors: pass `.data` and `.count`",
            ),
            // A vector *is* one register, so this is the one refusal that is not about width. Neither
            // back end declares a vector in a foreign signature and libffi has no vector type in this
            // bridge, so it would be a silent reinterpretation rather than a call.
            Item::VectorType { .. } => Some(
                "a vector is one register but no engine here declares one across a C boundary yet",
            ),
            // Compiler-internal shapes. Reachable only through a bug upstream, so they are refused
            // rather than assumed impossible (ADR-0017 §4's rule about placeholders).
            Item::ResultsType { .. } => Some(
                "a C procedure returns one value: several returns have no C signature",
            ),
            Item::ContextType => Some(
                "a `#foreign` procedure is `#c_call` and receives no context (ADR-0057 §3)",
            ),
            Item::TypeType | Item::ForeignLibraryType => Some(
                "this is a compile-time-only type and has no runtime representation to pass",
            ),
            // Values, not types. A signature position holding one is a bug upstream.
            Item::VoidValue
            | Item::BoolValue(_)
            | Item::IntValue { .. }
            | Item::FloatValue { .. }
            | Item::StaticArray { .. }
            | Item::StrValue(_)
            | Item::AggregateValue { .. }
            | Item::ProcValue { .. }
            | Item::TypeValue(_)
            | Item::ForeignLibraryValue(_) => {
                Some("this is a value rather than a type, which is a compiler bug")
            }
        }
    }

    /// Checks that a `#foreign` procedure's library operand really is a library.
    ///
    /// ADR-0016 §3 exists for this check: before it, `ForeignInfo::library` was a
    /// bare symbol that nothing resolved, which left the whole FFI boundary
    /// untyped — and ADR-0006 puts a libffi bridge inside the comptime VM, so a
    /// mis-declared binding can reach the host machine during compilation.
    fn check_foreign_binding(&mut self, proc: ProcId) {
        let hir = self.hir;
        let Some(info) = hir.proc(proc).foreign.clone() else {
            return;
        };
        let Some(library) = info.library else {
            return;
        };

        let interner = self.interner;
        let name = interner.resolve(library);
        match self.lookup_value_name(library) {
            Some(entry) => {
                if entry.ty != PoolId::FOREIGN_LIBRARY && entry.ty != PoolId::ERROR {
                    let text = self.describe(entry.ty);
                    self.diags.push(
                        Diagnostic::error(
                            info.span,
                            format!("`{name}` is not a foreign library: it is `{text}`"),
                        )
                        .with_code(E0225)
                        .with_help("declare it with `#system_library`"),
                    );
                }
            }
            None => {
                self.diags.push(
                    Diagnostic::error(info.span, format!("unknown foreign library `{name}`"))
                        .with_code(E0225)
                        .with_help(format!(
                            "declare it first, e.g. `{name} :: #system_library \"c\";`"
                        )),
                );
            }
        }
    }

    /// Looks a value name up in this file, then in the imported modules.
    fn lookup_value_name(&mut self, name: Symbol) -> Option<crate::sigs::SigEntry> {
        if let Some(item) = self.hir.scope.get(name) {
            return self.entry_for_item(item);
        }
        // **An imported name found here marks its import used** (ADR-0118 §2). The only caller is the
        // `#foreign` library check, and a library is named in a *declaration attribute* rather than an
        // expression — so `ResolveMap` (which covers `Expr::Name`) never sees it, and a module imported
        // *solely* for its `#system_library` read as unused. `Math` importing `Basic` for `libc` is exactly
        // that, and the quick fix beside the warning would have broken every libm wrap in it — ADR-0031 §2's
        // rule, and the third place this trap has had to be closed after an ordinary type annotation and a
        // type-argument reference (ADR-0117 §5).
        let found = self
            .imports
            .iter()
            .find_map(|(module, sigs)| sigs.lookup(name).map(|entry| (*module, entry)))?;
        let (module, entry) = found;
        self.sigs.insert_type_name_import(name, module);
        Some(entry)
    }
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    /// Types one expression, imposing `expected` on it where that is meaningful.
    ///
    /// `expected` is what makes ADR-0016 §1 work: an integer literal has no type
    /// of its own, so the context is the only thing that can give it one.
    pub(crate) fn check_expr(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        expected: Option<PoolId>,
    ) -> PoolId {
        let expr = self.expr_of(scope, id);
        let ty = match expr {
            // `context` is a `*Context` — passed by pointer so a callee's writes reach *its* callees
            // (ADR-0057 §2). Typed as the pointer rather than the struct, so `context.allocator` goes
            // through the same auto-deref `p.x` already does and needs no special field rule.
            Expr::Context(span) => {
                let ty = self.context_expr_type(scope, span);
                self.expect(expected, ty, span)
            }
            Expr::Literal(literal, span) => self.check_literal(&literal, expected, span),
            Expr::Name { span, res, .. } => {
                let res = self.resolve.get(scope, id).unwrap_or(res);
                // A **`#foreign` procedure taken as a value** is refused (E0256, ADR-0059 §5): its
                // type is `ContextKind::CCall` and the VM reaches it through libffi rather than a
                // `ProcRef`, so an indirect call to one is a second mechanism this wave does not
                // build. Caught here, in value position — a *direct* call routes through
                // `type_of_callee`, which does not refuse, so `write(…)` stays legal.
                if self.is_foreign_proc(&res) && !self.call_position.contains(&(scope, id)) {
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            "a `#foreign` procedure cannot be used as a value yet",
                        )
                        .with_code(E0256)
                        .with_note(
                            "an indirect call to a foreign procedure needs machinery a later wave adds",
                        ),
                    );
                    return PoolId::ERROR;
                }
                let ty = self.type_of_name(res);
                // **A type used where a runtime value is expected is refused** (E0261, ADR-0071 §3).
                // Before this, `t := Point;` type-checked cleanly and both engines exited 0, lowering
                // to `s0: type` and `v1: type = undef` — a slot of a type with *no runtime layout*
                // (`LayoutError::ComptimeOnly`) holding a placeholder that is a legitimate value. That
                // is this project's first named failure mode, invisible to the verifier and to
                // ADR-0017 §4's poison gate alike.
                //
                // Refused here rather than in lowering for ADR-0039 §3a's reason: rejecting a
                // construct is a semantic judgement, and a lowering refusal reports a
                // compiler-internal message for a program that looks well-formed.
                //
                // **Silent when the context is already poisoned**, which is `expect`'s rule and not a
                // politeness: `file_diagnostics` does not gate later phases on earlier ones, so
                // `n: nosuchtype = Point;` would otherwise report E0212 *and* E0261 for one mistake.
                // Checked here rather than left to `expect`, because this arm returns before reaching
                // it — the refusal has to know the same thing `expect` knows.
                if ty == PoolId::TYPE
                    && expected != Some(PoolId::ERROR)
                    && !self.type_is_allowed_here(scope, id)
                {
                    self.reject_type_as_value(span);
                    return PoolId::ERROR;
                }
                self.expect(expected, ty, span)
            }
            Expr::Binary { op, lhs, rhs, span } => {
                // `id` is threaded so that a resolved overload can be recorded against *this*
                // expression: `jr-mir` looks it up by the same key rather than re-resolving
                // (ADR-0048 §5).
                self.check_binary(scope, id, op, lhs, rhs, expected, span)
            }
            Expr::Unary { op, operand, span } => {
                self.check_unary(scope, op, operand, expected, span)
            }
            Expr::Call {
                callee,
                args,
                arg_names,
                span,
            } => {
                let ty = self.check_call(scope, id, callee, &args, &arg_names, span);
                self.expect(expected, ty, span)
            }
            Expr::Field {
                receiver,
                name,
                name_span,
                span,
            } => {
                let ty = self.check_field(scope, receiver, name, name_span);
                self.expect(expected, ty, span)
            }
            Expr::Index {
                base,
                index,
                index_span,
                span,
            } => {
                let ty = self.check_index(scope, base, index, index_span, span);
                self.expect(expected, ty, span)
            }
            Expr::Slice { base, span } => {
                let ty = self.check_slice(scope, base, span);
                self.expect(expected, ty, span)
            }
            // Both take `expected` **directly** rather than through `expect`: the context is
            // the input to typing them, not a constraint on the answer, so passing it on to
            // `expect` afterwards would compare the type against itself (ADR-0046 §1).
            Expr::Autocast { operand, span } => self.check_autocast(scope, operand, expected, span),
            Expr::Member {
                name,
                name_span,
                span,
            } => self.check_bare_member(name, name_span, expected, span),
            Expr::Deref(pointer, span) => {
                let ty = self.check_deref(scope, pointer, span);
                self.expect(expected, ty, span)
            }
            // `---` in an initialiser never reaches here: lowering records it as
            // a flag on the declaration. Anywhere else it has no type of its own,
            // so it takes the context's and stays quiet.
            Expr::Uninit(_) => expected.unwrap_or(PoolId::ERROR),
            Expr::Cast { ty, operand, span } => {
                let ty = self.check_cast(scope, ty, operand, span);
                self.expect(expected, ty, span)
            }
            // ADR-0016 §4: `#run e` has the type of `e` and is not folded. The
            // value arrives when the VM does.
            Expr::Run(inner, _) => self.check_expr(scope, inner, expected),
            Expr::Directive { name, arg, span } => {
                self.check_directive(name, arg.as_deref(), expected, span)
            }
            Expr::Error(_) => PoolId::ERROR,
        };
        self.types.set_expr(scope, id, ty);
        ty
    }

    /// Types `cast(T, x)` (ADR-0037 §2).
    ///
    /// The result type is always `T`, whatever the operand turns out to be — that is what
    /// makes a cast a cast rather than a checked conversion.
    ///
    /// # Why the operand is checked *against* the target
    ///
    /// Because that is what gives a literal operand the comptime fit check for free. Passing
    /// `T` as the operand's `expected` makes `cast(u8, 300)` take exactly the path
    /// `x: u8 = 300;` already takes and raise the same E0204 about the same source text
    /// (ADR-0016 §1). Nothing here re-implements the range test.
    ///
    /// A *runtime* operand takes the other branch: `expected` would demand equality, and a
    /// cast exists precisely to convert between unequal types. So a non-literal operand is
    /// typed with no expectation and then only its *kind* is checked.
    fn check_cast(
        &mut self,
        scope: ExprScope,
        ty: jr_hir::TypeRefId,
        operand: ExprId,
        span: Span,
    ) -> PoolId {
        let target = self.resolve_type(scope, ty, span);
        if target == PoolId::ERROR {
            // The target did not resolve, which E0212 already reported. Still type the
            // operand, so an error inside it is not swallowed by the outer one.
            self.check_expr(scope, operand, None);
            return PoolId::ERROR;
        }

        // A literal operand is context-typed by the target, which is where the comptime fit
        // check comes from. `is_untyped_literal` is the same predicate binary arithmetic uses.
        //
        // A *float* target and an untyped **integer** literal is the one case this shortcut
        // gets wrong: `cast(float64, 1)` would context-type `1` as a `float64`, and
        // `check_int_literal` then reports "expected `float64`, found an integer literal" —
        // a mismatch inside a cast, which is precisely the conversion the user asked for. So
        // the shortcut applies only when the literal and the target belong to the same
        // family, and a cross-family literal falls through to the ordinary path below where
        // it is typed on its own and converted (ADR-0040 §3).
        if self.is_untyped_literal(scope, operand)
            && !self.literal_crosses_families(scope, operand, target)
        {
            self.check_expr(scope, operand, Some(target));
            return target;
        }

        let from = self.check_expr(scope, operand, None);
        if from == PoolId::ERROR {
            return target;
        }

        // Four directions now: int→int, int→float, float→int, float→float (ADR-0040 §3). A
        // pointer is deliberately in none of them, so casting one is still refused rather
        // than becoming pointer arithmetic by the back door.
        // An enum casts **to** a numeric type but not from one: `cast(s64, c)` is how the
        // number is obtained (ADR-0041 §3, §6), while `cast(Colour, 1)` would manufacture a
        // value that may name no member at all — which is the hole a nominal type exists to
        // close. Asymmetric on purpose, and stated here because the symmetry is tempting.
        let from_numeric = self.is_numeric(from) || self.is_enum(from);
        let to_numeric = self.is_numeric(target);
        if !from_numeric || !to_numeric {
            let (from_text, to_text) = (self.describe(from), self.describe(target));
            self.diags.push(
                Diagnostic::error(span, format!("cannot cast `{from_text}` to `{to_text}`"))
                    .with_code(E0232)
                    .with_note(
                        "`cast` converts between numeric types — integers and floats — and \
                         from an enum to one",
                    ),
            );
        }
        target
    }

    /// Types `xx expr` (ADR-0046 §2).
    ///
    /// The conversion rule is **ADR-0037 §2's, unchanged**: `xx` is legal exactly where `cast`
    /// is legal and nowhere else. That equivalence is the design — a reader can always
    /// mechanically recover the `cast` — so this deliberately delegates rather than
    /// re-implementing a looser test.
    fn check_autocast(
        &mut self,
        scope: ExprScope,
        operand: ExprId,
        expected: Option<PoolId>,
        span: Span,
    ) -> PoolId {
        // No context means no target, and there is deliberately no fallback: a defaulted `xx`
        // would silently convert to a type nobody wrote (ADR-0046 §1).
        let Some(target) = expected else {
            // The operand is still typed, so an error inside it is not swallowed by this one.
            self.check_expr(scope, operand, None);
            self.diags.push(
                Diagnostic::error(span, "the target type of `xx` cannot be inferred here")
                    .with_code(E0242)
                    .with_note(
                        "`xx` takes its target type from the context — an annotation, a \
                         parameter, or the other side of a comparison",
                    )
                    .with_help("write the conversion explicitly, e.g. `cast(u8, x)`"),
            );
            return PoolId::ERROR;
        };
        if target == PoolId::ERROR {
            self.check_expr(scope, operand, None);
            return PoolId::ERROR;
        }

        // An untyped literal already takes the context's type, so `xx` adds nothing here and
        // would *hide* E0204's fit check. Reported before the operand is typed against the
        // target, because typing it that way is exactly what the `xx` is redundantly asking for.
        if self.is_untyped_literal(scope, operand) {
            let text = self.describe(target);
            self.diags.push(
                Diagnostic::error(span, "`xx` on a literal has no effect")
                    .with_code(E0243)
                    .with_note(format!(
                        "a literal already takes its type from the context, which here is \
                         `{text}`"
                    ))
                    .with_help("remove the `xx`"),
            );
            self.check_expr(scope, operand, Some(target));
            return target;
        }

        let from = self.check_expr(scope, operand, None);
        if from == PoolId::ERROR {
            return target;
        }
        // The same pair of predicates `check_cast` applies, so the two cannot drift: numeric to
        // numeric, or an enum to a numeric type but never the reverse (ADR-0041 §3).
        let from_ok = self.is_numeric(from) || self.is_enum(from);
        let to_ok = self.is_numeric(target);
        if !from_ok || !to_ok {
            let (from_text, to_text) = (self.describe(from), self.describe(target));
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("cannot convert `{from_text}` to `{to_text}` with `xx`"),
                )
                .with_code(E0232)
                .with_note(
                    "`xx` converts between numeric types — integers and floats — and from an \
                     enum to one, exactly as `cast` does",
                ),
            );
            return target;
        }
        target
    }

    /// Types a bare `.RED` (ADR-0046 §3, executing ADR-0041 §2's plan).
    ///
    /// Takes no `ExprScope`: a bare member names no scope, which is the whole point — the
    /// namespace comes from the context type rather than from anywhere a name could be looked
    /// up.
    fn check_bare_member(
        &mut self,
        name: Symbol,
        name_span: Span,
        expected: Option<PoolId>,
        span: Span,
    ) -> PoolId {
        let Some(target) = expected else {
            self.diags.push(
                Diagnostic::error(
                    span,
                    "the enum a bare `.` member belongs to cannot be inferred here",
                )
                .with_code(E0244)
                .with_note(
                    "a bare member takes its enum from the context — an annotation, a \
                         parameter, or the other side of a comparison",
                )
                .with_help("name the enum, e.g. `Colour.RED`"),
            );
            return PoolId::ERROR;
        };
        if target == PoolId::ERROR {
            return PoolId::ERROR;
        }

        // **A bare member against a `variant` names one of its cases** (ADR-0068 §5). The same idea
        // ADR-0046 built this for — the context supplies the namespace the source omitted — with a
        // variant's case list as the namespace instead of an enum's members. Handled before the enum
        // gate below so that a `switch v { case .i; … }` resolves rather than being told it needs an
        // enum, and the *type* is the variant, because that is what the arm is compared against.
        if let Item::VariantType { decl, .. } = *self.pool.item(target) {
            let known = self
                .pool
                .struct_fields(decl)
                .is_some_and(|cases| cases.iter().any(|case| case.name == name));
            if known {
                return target;
            }
            let text = self.interner.resolve(name).to_owned();
            let ty_text = self.describe(target);
            self.diags.push(
                Diagnostic::error(name_span, format!("`{ty_text}` has no case `{text}`"))
                    .with_code(E0244),
            );
            return PoolId::ERROR;
        }

        // A context that is neither an enum nor a variant is a *different* problem from having none,
        // so it gets its own wording with the type named — conflating them would misdirect the reader
        // (ADR-0046 §4).
        let Item::EnumType { decl, flags } = *self.pool.item(target) else {
            let text = self.describe(target);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("expected `{text}`, and a bare `.` member needs an enum"),
                )
                .with_code(E0244)
                .with_note("a bare member is only meaningful where the context type is an enum"),
            );
            return target;
        };

        // The same lookup and the same suggestion the qualified form uses (ADR-0041 §4), so the
        // two spellings cannot disagree about which members exist.
        let known = self
            .pool
            .enum_members(decl)
            .is_some_and(|members| members.iter().any(|m| m.name == name));
        if known {
            return target;
        }
        let interner = self.interner;
        let text = interner.resolve(name).to_owned();
        self.no_such_member(decl, flags, &text, name_span);
        PoolId::ERROR
    }

    /// Whether `ty` is an enum type.
    fn is_enum(&self, ty: PoolId) -> bool {
        matches!(self.pool.item(ty), Item::EnumType { .. })
    }

    /// Whether `ty` is an integer or a float type.
    /// Whether `ty` is a **multi-word aggregate**, which `==` has no meaning for (ADR-0099 §4).
    ///
    /// Structural rather than layout-based, because `Layout` records only size and alignment — an `s64` and
    /// a two-field struct of `s32`s have the same eight bytes and only one of them is comparable. Exhaustive
    /// over `Item` for the reason AGENTS.md states: a new aggregate kind must be a compile error here rather
    /// than silently falling through to a comparison the VM cannot make.
    fn is_aggregate(&self, ty: PoolId) -> bool {
        match self.pool.item(ty) {
            Item::StringType
            | Item::StructType { .. }
            | Item::UnionType { .. }
            | Item::VariantType { .. }
            | Item::ArrayType { .. }
            // A vector is sixteen bytes in one register, so `==` on it is the same unanswerable
            // question an array's is (ADR-0099 §4): elementwise, or all-lanes? The all-lanes answer
            // needs a mask, which is ADR-0148 §5's deferred wave — so this stays a refusal until
            // there is a type to express it in, rather than picking one silently.
            | Item::VectorType { .. }
            | Item::ViewType { .. }
            | Item::DynamicArrayType { .. }
            | Item::ResultsType { .. }
            // A context is a struct in every way that matters here (ADR-0057), so comparing two is the
            // same unanswerable question.
            | Item::ContextType => true,
            Item::VoidType
            | Item::BoolType
            | Item::IntType { .. }
            | Item::FloatType { .. }
            | Item::PointerType(_)
            | Item::ProcType { .. }
            | Item::EnumType { .. }
            | Item::TypeType
            | Item::ErrorType
            | Item::IntValue { .. }
            | Item::FloatValue { .. }
            | Item::BoolValue(_)
            // A compiler-emitted table is a *value* (ADR-0152 §1), never a type asked about here.
            | Item::StaticArray { .. }
            | Item::StrValue(_)
            | Item::TypeValue(_)
            | Item::ProcValue { .. }
            | Item::AggregateValue { .. }
            // Not scalars, but not comparable aggregates either: a foreign library is a handle sema already
            // refuses as a value (E0242), and a void has nothing to compare. `false` sends them down the
            // ordinary path, which already reports what is wrong with them.
            | Item::ForeignLibraryType
            | Item::ForeignLibraryValue(_)
            | Item::VoidValue => false,
        }
    }

    fn is_numeric(&self, ty: PoolId) -> bool {
        self.int_info(ty).is_some() || jr_pool::FloatKind::of(self.pool, ty).is_some()
    }

    /// Whether an untyped literal operand and a cast target are in different numeric families.
    ///
    /// `cast(float64, 1)` and `cast(s64, 1.5)` are both legal conversions, and both would be
    /// reported as type mismatches if the literal were context-typed by the target. This is
    /// what keeps the context-typing shortcut from swallowing the very conversion `cast`
    /// exists to express.
    fn literal_crosses_families(
        &mut self,
        scope: ExprScope,
        operand: ExprId,
        target: PoolId,
    ) -> bool {
        let target_is_float = jr_pool::FloatKind::of(self.pool, target).is_some();
        let operand_is_float = self.untyped_literal_is_float(scope, operand);
        target_is_float != operand_is_float
    }

    /// Whether an untyped literal expression is built from *float* literals.
    ///
    /// Mirrors `is_untyped_literal`'s recursion, because `-1.5` and `1.5 + 2.5` are untyped
    /// float expressions just as `-1` and `1 + 2` are untyped integer ones.
    fn untyped_literal_is_float(&self, scope: ExprScope, id: ExprId) -> bool {
        match self.expr_of(scope, id) {
            Expr::Literal(literal, _) => match literal {
                Literal::Float { .. } => true,
                Literal::Int { .. } | Literal::Str(_) | Literal::Bool(_) | Literal::Null => false,
            },
            Expr::Unary { operand, .. } => self.untyped_literal_is_float(scope, operand),
            Expr::Binary { lhs, .. } => self.untyped_literal_is_float(scope, lhs),
            Expr::Run(inner, _) => self.untyped_literal_is_float(scope, inner),
            _ => false,
        }
    }

    /// Types a literal.
    fn check_literal(&mut self, literal: &Literal, expected: Option<PoolId>, span: Span) -> PoolId {
        match literal {
            Literal::Bool(_) => self.expect(expected, PoolId::BOOL, span),
            Literal::Str(_) => self.expect(expected, PoolId::STRING, span),
            Literal::Int { value, .. } => self.check_int_literal(*value, expected, span),
            Literal::Float { .. } => self.check_float_literal(expected, span),
            Literal::Null => self.check_null_literal(expected, span),
        }
    }

    /// Types `null` against its context (ADR-0060 §1).
    ///
    /// `null` has no intrinsic type and takes its context's, exactly as an integer literal does —
    /// but unlike an integer there is **no default**: a bare `null` with no context is E0257,
    /// because there is no one pointer type to fall back to. The context must be a *pointer* type;
    /// `n: s64 = null` is the same E0257, the literal being fine and the context wrong for it.
    fn check_null_literal(&mut self, expected: Option<PoolId>, span: Span) -> PoolId {
        match expected {
            Some(want) if want == PoolId::ERROR => PoolId::ERROR,
            Some(want) if self.pointee(want).is_some() => want,
            Some(want) => {
                let text = self.describe(want);
                self.diags.push(
                    Diagnostic::error(
                        span,
                        format!("mismatched types: expected `{text}`, found `null`"),
                    )
                    .with_code(E0257)
                    .with_note("`null` is a pointer, so its context must be a pointer type"),
                );
                PoolId::ERROR
            }
            None => {
                self.diags.push(
                    Diagnostic::error(span, "`null` needs a pointer type from its context")
                        .with_code(E0257)
                        .with_note(
                            "unlike an integer literal, `null` has no default type — annotate the                              binding or call, e.g. `p: *u8 = null`",
                        ),
                );
                PoolId::ERROR
            }
        }
    }

    /// Types a float literal against its context (ADR-0040 §5).
    ///
    /// The same shape as [`Ctx::check_int_literal`] and one rule shorter: a float literal
    /// takes its context's type, defaults to `float64`, and — unlike an integer literal —
    /// **has no fit check**. There is nothing to check: ADR-0040 §1 makes an out-of-range
    /// value saturate to `inf`, so every float literal has an answer in every float type,
    /// where `x: u8 = 300;` has none and is E0204.
    fn check_float_literal(&mut self, expected: Option<PoolId>, span: Span) -> PoolId {
        let default = self.pool.intern(Item::FloatType { bits: 64 });
        match expected {
            None => default,
            Some(want) if want == PoolId::ERROR => PoolId::ERROR,
            Some(want) => {
                if jr_pool::FloatKind::of(self.pool, want).is_none() {
                    // Deliberately not "expected `s64`, found `float64`": the literal has no
                    // intrinsic type, so naming one would be inventing it. The phrasing
                    // matches `check_int_literal`'s for the mirror-image mistake.
                    let text = self.describe(want);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("mismatched types: expected `{text}`, found a float literal"),
                        )
                        .with_code(E0214),
                    );
                    return want;
                }
                want
            }
        }
    }

    /// Types an integer literal against its context (ADR-0016 §1).
    ///
    /// The literal has no intrinsic type. It takes the context's, defaults to
    /// `s64` when there is none, and must fit whichever type it ends up with.
    /// Note what this means for diagnostics: the *contextual* type is the only
    /// one worth naming, because the literal has no other.
    fn check_int_literal(&mut self, value: i128, expected: Option<PoolId>, span: Span) -> PoolId {
        let target = match expected {
            None => PoolId::S64,
            Some(want) => {
                if want == PoolId::ERROR {
                    return PoolId::ERROR;
                }
                if self.int_info(want).is_none() {
                    let text = self.describe(want);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!(
                                "mismatched types: expected `{text}`, found an integer literal"
                            ),
                        )
                        .with_code(E0214),
                    );
                    return want;
                }
                want
            }
        };

        if let Some((signed, bits)) = self.int_info(target)
            && !literal_fits(signed, bits, value)
        {
            let text = self.describe(target);
            self.diags.push(
                Diagnostic::error(span, format!("integer literal does not fit `{text}`"))
                    .with_code(E0204)
                    .with_note(format!(
                        "an integer literal takes its type from its context, which here is `{text}`"
                    ))
                    .with_note(format!(
                        "the range of `{text}` is {}",
                        int_range(signed, bits)
                    )),
            );
        }
        target
    }

    /// The type of a `context` expression, refusing it where there is no context (ADR-0057 §3).
    ///
    /// Two refusals, both E0254 and each with its own note: a `#c_call` procedure receives none by
    /// definition, and file scope has no call to have carried one.
    fn context_expr_type(&mut self, scope: ExprScope, span: Span) -> PoolId {
        match scope {
            ExprScope::TopLevel => {
                self.diags.push(
                    Diagnostic::error(span, "`context` is not available at file scope")
                        .with_code(E0254)
                        .with_note(
                            "a constant's value is computed before any call, so no context has been passed",
                        ),
                );
                PoolId::ERROR
            }
            ExprScope::Body(body) => {
                if self.body_is_c_call(body) {
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            "`context` is not available in a `#c_call` procedure",
                        )
                        .with_code(E0254)
                        .with_note("a `#c_call` procedure receives no implicit context (ADR-0001)")
                        .with_help("remove the `#c_call`, or pass what is needed explicitly"),
                    );
                    return PoolId::ERROR;
                }
                self.pool.context_pointer()
            }
        }
    }

    /// Whether the procedure owning `body` is `#c_call` (ADR-0057 §3).
    ///
    /// Reads `Proc::c_call` *or* `foreign`, because ADR-0001 makes every `#foreign` procedure
    /// implicitly `#c_call` and sema is where that implication already lives — asking only the flag
    /// would let a `#foreign` procedure mention `context`.
    fn body_is_c_call(&self, body: BodyId) -> bool {
        self.hir
            .procs
            .iter()
            .find(|proc| proc.body == Some(body))
            .is_some_and(|proc| proc.c_call || proc.foreign.is_some())
    }

    /// Types a name reference from its resolution.
    /// Whether `res` names a `#foreign` procedure (ADR-0059 §5).
    ///
    /// A same-file item only: a cross-file procedure value resolves to `Res::Imported` and is
    /// refused earlier for a different reason (ADR-0059 §1), so this need not chase imports.
    fn is_foreign_proc(&mut self, res: &Res) -> bool {
        match res {
            Res::Item(item) => self
                .hir
                .items
                .get(item.index())
                .and_then(|it| match &it.kind {
                    jr_hir::ItemKind::Const {
                        value: jr_hir::ConstValue::Proc(proc),
                    } => self.hir.procs.get(proc.index()),
                    _ => None,
                })
                .is_some_and(|proc| proc.foreign.is_some()),
            // An **imported** `#foreign` procedure, asked of its *type* rather than the other
            // file's HIR (ADR-0062 §3). `ContextKind::CCall` is exactly what `#foreign` means
            // (ADR-0001), and the type is what this file already has — chasing the declaration
            // across the module boundary would be a second answer to the same question.
            //
            // Without this arm an imported `#foreign` procedure assigned into a proc-pointer field
            // reported "expected `(s64) -> *u8`, found `(s64) -> *u8`" — identical text, because the
            // two types differ only in the invisible `ContextKind`. A message a reader cannot act on.
            Res::Imported(import, name) => {
                let ty = self
                    .entry_for_import(*import, *name)
                    .map_or(PoolId::ERROR, |entry| entry.ty);
                matches!(
                    self.pool.item(ty),
                    Item::ProcType {
                        context: jr_pool::ContextKind::CCall,
                        ..
                    }
                )
            }
            Res::Local(_) | Res::Param(_) | Res::Promoted { .. } | Res::Error => false,
        }
    }

    /// Whether a type is a legal thing to name at this expression (ADR-0071 §3).
    ///
    /// A lookup in `type_position`, which the two positions that accept one populate: a field
    /// access's receiver and a `::` constant's initialiser. See that field's documentation for why
    /// this is an allowlist.
    fn type_is_allowed_here(&self, scope: ExprScope, id: ExprId) -> bool {
        self.type_position.contains(&(scope, id))
    }

    /// Reports a type used where a runtime value was expected (E0261, ADR-0071 §3).
    ///
    /// The note names the positions that *do* accept a type rather than naming a type the reader
    /// could annotate with, because `Type` is deliberately not spellable (ADR-0071 §1) — a help line
    /// suggesting an annotation would name something the parser rejects. "Cannot be stored" without
    /// saying where it *can* go is a diagnostic a reader cannot act on.
    fn reject_type_as_value(&mut self, span: Span) {
        self.diags.push(
            Diagnostic::error(span, "a type is a compile-time value, not a runtime one")
                .with_code(E0261)
                .with_note("a type has no runtime representation, so there is nothing to store")
                .with_help(
                    "bind it with `::`, e.g. `T :: Point;`, or write it as a type annotation",
                ),
        );
    }

    fn type_of_name(&mut self, res: Res) -> PoolId {
        match res {
            Res::Local(local) => self
                .body
                .as_ref()
                .and_then(|body| self.locals.get(&(body.id, local)).copied())
                .unwrap_or(PoolId::ERROR),
            // A `ParamId` indexes the enclosing `Proc::params`, which is why the
            // body has to know which procedure it belongs to.
            Res::Param(param) => self
                .body
                .as_ref()
                .and_then(|body| body.params.get(param.index()).copied())
                .unwrap_or(PoolId::ERROR),
            Res::Item(item) => self
                .entry_for_item(item)
                .map_or(PoolId::ERROR, |entry| entry.ty),
            Res::Imported(import, name) => self
                .entry_for_import(import, name)
                .map_or(PoolId::ERROR, |entry| entry.ty),
            // A promoted name is the *base's* type, then a field of it (ADR-0050 §2). The base is
            // itself a `Res`, so this recurses: a name promoted through an embedded field is a
            // chain, and typing it one level would silently give the wrong type for the
            // transitive case ADR-0050 §4 promises.
            Res::Promoted { base, field } => {
                let base_ty = self.type_of_name((*base).clone());
                self.promoted_field_type(base_ty, field)
            }
            Res::Error => PoolId::ERROR,
        }
    }

    /// The type of `name` found through a `using`-embedded field of the struct `decl`.
    ///
    /// Searches breadth-first over the embedded bases, so a shallower embedding wins — which
    /// matters when two levels both provide a name and is the same "nearer declaration shadows"
    /// rule the direct-field check above uses.
    ///
    /// Returns `None` when nothing provides it, leaving the caller to raise E0218 with its
    /// near-name suggestion (ADR-0031 §1) rather than duplicating that diagnostic here.
    fn embedded_field_type(&mut self, decl: jr_pool::DeclId, name: Symbol) -> Option<PoolId> {
        // A cycle is impossible — a struct cannot contain itself by value, and the recursive-type
        // refusal already covers it (ADR-0050 §4) — but the depth bound is kept anyway, because a
        // malformed pool would otherwise loop forever inside the compiler rather than report.
        let mut frontier: Vec<jr_pool::DeclId> = vec![decl];
        for _ in 0..16u32 {
            let mut next = Vec::new();
            for current in frontier.drain(..) {
                let fields = match self.pool.struct_fields(current) {
                    Some(fields) => fields.to_vec(),
                    None => continue,
                };
                for field in &fields {
                    if !field.using {
                        continue;
                    }
                    let mut base_ty = field.ty;
                    while let Some(inner) = self.pointee(base_ty) {
                        base_ty = inner;
                    }
                    let Item::StructType {
                        decl: inner_decl, ..
                    } = self.pool.item(base_ty)
                    else {
                        continue;
                    };
                    let inner_decl = *inner_decl;
                    if let Some(found) = self
                        .pool
                        .struct_fields(inner_decl)
                        .and_then(|fs| fs.iter().find(|f| f.name == name).map(|f| f.ty))
                    {
                        return Some(found);
                    }
                    next.push(inner_decl);
                }
            }
            if next.is_empty() {
                return None;
            }
            frontier = next;
        }
        None
    }

    /// The type of `field` within `base_ty`, for a `using`-promoted name.
    ///
    /// Auto-derefs, so `using p: *Point` types `x` as `Point`'s `x` — matching the auto-deref
    /// `p.x` already does, because the two spellings must agree (ADR-0050 §1).
    ///
    /// Raises no diagnostic: resolution built the promotion from the struct's *own* field list, so
    /// a field that does not exist here means the two disagree, which is a compiler bug rather than
    /// a program error. `PoolId::ERROR` propagates without inventing a message that would point at
    /// the user's code for our mistake.
    fn promoted_field_type(&mut self, base_ty: PoolId, field: Symbol) -> PoolId {
        let mut ty = base_ty;
        while let Some(inner) = self.pointee(ty) {
            ty = inner;
        }
        // Only a struct, deliberately: `Item::UnionType` and `Item::VariantType` are *not* matched
        // here even though `check_field` treats all three alike, because ADR-0050 §5 refuses `using`
        // on a union — and a variant is refused for the same reason plus a stronger one: promoting a
        // case into scope would make a name read a field the tag may say is not live. Resolution has
        // already reported it; accepting one here would give a value to a promotion that was refused.
        match self.pool.item(ty) {
            Item::StructType { .. } => {}
            _ => return PoolId::ERROR,
        }
        self.pool
            .fields_of(ty)
            .and_then(|fields| fields.iter().find(|f| f.name == field).map(|f| f.ty))
            .unwrap_or(PoolId::ERROR)
    }

    /// Types a binary operation.
    /// Whether `op` is an operation `vector` has, reporting E0285 when it is not (ADR-0148 §3, §6).
    ///
    /// The set differs by lane type, and for integers it differs from *scalar* arithmetic:
    ///
    /// | lanes | has | refused |
    /// |---|---|---|
    /// | integer | `+% -% *%` | `+ - *` (would have to trap), `/ %` (no machine has them) |
    /// | float | `+ - * /` | `%` and the wrapping forms, exactly as a scalar float (ADR-0040 §7) |
    ///
    /// # Why an integer vector wants the *wrapping* spelling
    ///
    /// A scalar `+` traps on overflow (ADR-0002), and **no vector add can**: there is no per-lane
    /// overflow flag on any target, and Cranelift's `iadd` on a vector wraps silently. So the three
    /// engines can only agree on wrapping — the VM loops and could trap, native cannot.
    ///
    /// Three ways out, and this is the third. Let `+` wrap on a vector: then one spelling means two
    /// things depending on the type, which is the silent-semantic-difference this project refuses.
    /// Make `+` trap: then every lane needs a compare and a branch, which destroys the entire reason
    /// the construct exists — a pessimisation as invisible as a miscompile (§3's argument for
    /// refusing integer division rather than scalarising it). Or require the operators that *say*
    /// they wrap, which is what ADR-0002 put in the language for exactly this: a program gets the
    /// arithmetic it asked for and the engines agree.
    fn check_vector_operator(&mut self, op: BinOp, vector: PoolId, span: Span) -> bool {
        let Some((elem, _)) = self.vector_parts(vector) else {
            return true;
        };
        let float_lanes = jr_pool::FloatKind::of(self.pool, elem).is_some();
        let text = self.describe(vector);
        let operator = bin_op_text(op);

        if float_lanes {
            // A float vector is the easy half: exactly the scalar float set, for the same reasons
            // (ADR-0040 §7 for `%`, ADR-0002 for the wrapping forms having no float meaning).
            if matches!(
                op,
                BinOp::Rem | BinOp::WrapAdd | BinOp::WrapSub | BinOp::WrapMul
            ) {
                self.reject_float_operator(op, &text, span);
                return false;
            }
            return true;
        }

        match op {
            BinOp::WrapAdd | BinOp::WrapSub | BinOp::WrapMul => true,
            // The trapping forms, whose refusal is the §6 decision above.
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                let wrapping = match op {
                    BinOp::Add => "+%",
                    BinOp::Sub => "-%",
                    _ => "*%",
                };
                self.diags.push(
                    Diagnostic::error(
                        span,
                        format!(
                            "`{operator}` on `{text}` would have to trap, and no vector add can"
                        ),
                    )
                    .with_code(E0285)
                    .with_note(
                        "no target has a per-lane overflow flag, so a trapping vector operation \
                         would need a compare and a branch for every lane",
                    )
                    .with_help(format!(
                        "use `{wrapping}`, which says the arithmetic wraps — that is what the \
                         hardware does"
                    )),
                );
                false
            }
            // Division, the fact the probe found (ADR-0148 §3). `%` is a divide too.
            BinOp::Div | BinOp::Rem => {
                self.diags.push(
                    Diagnostic::error(
                        span,
                        format!("`{operator}` is not a vector operation on integers"),
                    )
                    .with_code(E0285)
                    .with_note(
                        "no machine has an integer vector divide, so only float vectors can be \
                         divided",
                    )
                    .with_help("divide the lanes individually, or use a float vector"),
                );
                false
            }
            // Reached only if a new arithmetic `BinOp` is added; the caller has already narrowed to
            // the arithmetic group, and the bitwise, shift and comparison groups are handled by
            // their own arms in `check_binary`.
            BinOp::BitAnd
            | BinOp::BitOr
            | BinOp::BitXor
            | BinOp::Shl
            | BinOp::Shr
            | BinOp::Eq
            | BinOp::Ne
            | BinOp::Lt
            | BinOp::Le
            | BinOp::Gt
            | BinOp::Ge
            | BinOp::And
            | BinOp::Or => {
                self.reject_operator(op, &text, span);
                false
            }
        }
    }

    fn check_binary(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
        expected: Option<PoolId>,
        span: Span,
    ) -> PoolId {
        // **An overload is looked for before anything else** (ADR-0048 §4), because
        // `unify_operands` below refuses unequal operand types and a mixed-type overload —
        // `Vec2 * float64` — has to be reachable. A builtin meaning always wins, which falls out
        // of §3's orphan rule: no overload can exist for two builtin types, so `s64 + s64` cannot
        // find one.
        //
        // The whole lookup is skipped for a file that declares and imports no overload, so
        // ordinary arithmetic pays nothing for this feature existing.
        if let Some(ty) = self.check_operator_overload_call(scope, id, op, lhs, rhs, span) {
            return self.expect(expected, ty, span);
        }

        match op {
            // `<< >>` first, because they are the one binary form whose operands need **not**
            // match: the count is a separate integer, so `x << 1` must not force `1` to `x`'s
            // type nor complain when it differs (ADR-0042 §2). The result is the *left*
            // operand's type.
            BinOp::Shl | BinOp::Shr => {
                let want = expected.filter(|ty| self.int_info(*ty).is_some());
                let value = self.check_expr(scope, lhs, want);
                // The count takes `s64` when it is an untyped literal, and keeps its own type
                // otherwise. Either way it is checked independently of the value.
                let count = self.check_expr(scope, rhs, Some(PoolId::S64));
                if value != PoolId::ERROR && self.int_info(value).is_none() {
                    let text = self.describe(value);
                    // A flags enum accepts `& | ^ ~` but **not** shifts (ADR-0043 §3), and the
                    // reason is specific enough to say: `Perm.READ << 1` would produce `WRITE`
                    // by an accident of the numbering. Saying only "applies to integers" would
                    // be misleading for a type that *does* accept the other four.
                    if self.is_flags(value) {
                        self.reject_shift_on_flags(op, &text, span);
                    } else {
                        self.reject_bitwise(op, &text, span);
                    }
                    return PoolId::ERROR;
                }
                if count != PoolId::ERROR && self.int_info(count).is_none() {
                    let text = self.describe(count);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("a shift count must be an integer, not `{text}`"),
                        )
                        .with_code(E0223),
                    );
                    return PoolId::ERROR;
                }
                self.expect(expected, value, span)
            }
            // `& | ^` are integers only (ADR-0042 §5): a float's bits are a sign, an exponent
            // and a mantissa, so ANDing two of them is the AND of nothing meaningful; and an
            // enum's members are named alternatives, which is the refusal `enum_flags` will
            // lift rather than one to lift here.
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                // A *flags* enum is the one non-integer these accept, and the result keeps the
                // flags type rather than decaying to the backing integer (ADR-0043 §3) — which
                // is what makes a `Perm` stay a `Perm` through a combination.
                let want = expected.filter(|ty| self.int_info(*ty).is_some() || self.is_flags(*ty));
                let (left, right) = self.check_operands(scope, lhs, rhs, want);
                let result = self.unify_operands(left, right, span);
                if result != PoolId::ERROR
                    && self.int_info(result).is_none()
                    && !self.is_flags(result)
                {
                    let text = self.describe(result);
                    if self.is_enum(result) {
                        self.reject_bitwise_on_plain_enum(op, &text, span);
                    } else {
                        self.reject_bitwise(op, &text, span);
                    }
                    return PoolId::ERROR;
                }
                self.expect(expected, result, span)
            }
            BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Rem
            | BinOp::WrapAdd
            | BinOp::WrapSub
            | BinOp::WrapMul => {
                // **Pointer arithmetic, before the numeric path** (ADR-0064). A pointer operand must
                // not be unified with an integer one, so this is decided by typing each operand with
                // no shared expectation and asking whether either is a pointer. Only `+` and `-`
                // apply; `*`, `/`, `%` and the wrapping forms on a pointer fall through to the
                // rejection below, which is what E0223 means for them.
                //
                // **Skipped when a concrete numeric type is expected**, because then the expression
                // *is* numeric — `sum: s64 = xx tiny + 1;` must push `s64` inward so the autocast has
                // a context (E0242 otherwise), and a pointer result could never satisfy an `s64`
                // annotation anyway. So the speculative untyped probe below only runs when the result
                // could actually be a pointer: no expectation, or a pointer expectation.
                let numeric_context =
                    expected.is_some_and(|ty| self.is_numeric(ty) && self.pointee(ty).is_none());
                if matches!(op, BinOp::Add | BinOp::Sub)
                    && !numeric_context
                    && let Some(result) = self.check_pointer_arithmetic(scope, op, lhs, rhs, span)
                {
                    return self.expect(expected, result, span);
                }
                // Push a *numeric* context inward so that `g: u8 = 1 + 2;` types both
                // literals as `u8`, and `f: float32 = 1.5 + 2.5;` types both as `float32`,
                // rather than defaulting either and then complaining.
                let want = expected.filter(|ty| self.is_numeric(*ty));
                let (left, right) = self.check_operands(scope, lhs, rhs, want);
                let result = self.unify_operands(left, right, span);
                if result == PoolId::ERROR {
                    return self.expect(expected, result, span);
                }
                // **A vector, before the scalar numeric path** (ADR-0148 §3, §6). Its own decision
                // because the operator set differs by *lane* type and, for integers, differs from
                // scalar arithmetic in a way the program must spell.
                if result != PoolId::ERROR && self.vector_parts(result).is_some() {
                    if !self.check_vector_operator(op, result, span) {
                        return PoolId::ERROR;
                    }
                    return self.expect(expected, result, span);
                }
                let is_float = jr_pool::FloatKind::of(self.pool, result).is_some();
                if self.int_info(result).is_none() && !is_float {
                    // An enum gets a message that says what to do: the members are named
                    // alternatives rather than magnitudes, so arithmetic on one has no
                    // meaning as a member — but the *number* is one cast away (ADR-0041 §6).
                    if matches!(self.pool.item(result), Item::EnumType { .. }) {
                        let text = self.describe(result);
                        self.reject_enum_operator(op, &text, span);
                        return PoolId::ERROR;
                    }
                    let text = self.describe(result);
                    self.reject_operator(op, &text, span);
                    return PoolId::ERROR;
                }
                // The operators floats do not have (ADR-0040 §7 for `%`; the wrapping forms
                // are ADR-0002's integer opt-out and have no float meaning at all, since
                // nothing wraps).
                if is_float
                    && matches!(
                        op,
                        BinOp::Rem | BinOp::WrapAdd | BinOp::WrapSub | BinOp::WrapMul
                    )
                {
                    let text = self.describe(result);
                    self.reject_float_operator(op, &text, span);
                    return PoolId::ERROR;
                }
                self.expect(expected, result, span)
            }
            BinOp::Eq | BinOp::Ne => {
                let (left, right) = self.check_operands(scope, lhs, rhs, None);
                let operand = self.unify_operands(left, right, span);
                // A view has no equality (ADR-0044 §5). Refused rather than given one of the
                // two available meanings, because "same storage" and "same contents" are both
                // plausible and the wrong reading would look like working code.
                if matches!(self.pool.item(operand), Item::ViewType { .. }) {
                    let text = self.describe(operand);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("`{}` is not supported for `{text}`", bin_op_text(op)),
                        )
                        .with_code(E0241)
                        .with_note(
                            "two views could compare as the same storage or as the same \
                             contents, and Jairs does not pick one for you",
                        )
                        .with_help("compare `.count`, or compare elements in a loop"),
                    );
                    return PoolId::BOOL;
                }
                // **An aggregate has no equality either** (E0278, ADR-0099 §4), and a `string` is the case
                // that matters: it is `{data, count}` (ADR-0004), so "same storage" and "same contents" are
                // both available and neither is chosen — ADR-0044 §5's argument for a view, one type wider.
                //
                // Found by *probing* this sub-wave's own corpus file rather than by reading: before this,
                // `a == "x"` reached MIR and leaked `expected a scalar, found an aggregate`, an internal
                // error for a program a reader would reasonably expect to compile. `ERROR` falls through so
                // an already-refused operand is not refused twice.
                if operand != PoolId::ERROR && self.is_aggregate(operand) {
                    let text = self.describe(operand);
                    let contents = if operand == PoolId::STRING {
                        "two strings could compare as the same bytes or as the same pointer, and \
                         Jairs does not pick one for you"
                    } else if self.vector_parts(operand).is_some() {
                        // A vector's own reason, because the generic aggregate wording — "field by
                        // field", "compare fields one at a time" — names something a vector has
                        // none of, and a message that describes the wrong construct is worse than a
                        // vague one. Comparing lanes yields a *mask*, which is ADR-0148 §5's
                        // deferred wave: the answer is "which lanes are equal", and `bool` cannot
                        // carry it.
                        "comparing two vectors yields one answer per lane — a mask — and Jairs has \
                         no mask type yet, so there is nothing for `==` to produce"
                    } else {
                        "an aggregate's equality would have to be field by field, and Jairs does \
                         not generate one for you"
                    };
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("`{}` is not supported for `{text}`", bin_op_text(op)),
                        )
                        .with_code(E0278)
                        .with_note(contents)
                        .with_help(
                            if self.vector_parts(operand).is_some() {
                                "compare the lanes you care about: `a[0] == b[0]`"
                            } else {
                                "compare `.count`, or compare fields one at a time"
                            },
                        ),
                    );
                    return PoolId::BOOL;
                }
                self.expect(expected, PoolId::BOOL, span)
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let (left, right) = self.check_operands(scope, lhs, rhs, None);
                let operand = self.unify_operands(left, right, span);
                if operand != PoolId::ERROR && !self.is_numeric(operand) {
                    let text = self.describe(operand);
                    if matches!(self.pool.item(operand), Item::EnumType { .. }) {
                        self.reject_enum_operator(op, &text, span);
                    } else {
                        self.reject_operator(op, &text, span);
                    }
                }
                self.expect(expected, PoolId::BOOL, span)
            }
            BinOp::And | BinOp::Or => {
                self.check_expr(scope, lhs, Some(PoolId::BOOL));
                self.check_expr(scope, rhs, Some(PoolId::BOOL));
                self.expect(expected, PoolId::BOOL, span)
            }
        }
    }

    /// Checks `switch e { case v; … else; … }` (ADR-0067).
    ///
    /// Three jobs, in this order because each depends on the last: type the scrutinee, check every arm's
    /// value *against that type* — which is what lets a bare `.RED` resolve, since the scrutinee's type
    /// is the expected type `check_bare_member` wants (§2) — and then judge the set of arms as a whole.
    ///
    /// The set judgement is where the diagnostics live: a duplicate value or a second `else` is E0259,
    /// an enum `switch` missing members is E0258, and an `else` on one that names them all is E0260. The
    /// last is what makes the first worth having, since otherwise every `switch` could end in `else`.
    fn check_switch(
        &mut self,
        body: BodyId,
        value: ExprId,
        arms: &[jr_hir::SwitchArm],
        span: Span,
    ) {
        let scope = ExprScope::Body(body);
        let scrutinee = self.check_expr(scope, value, None);

        // Which enum this is, if any. Only an enum has a finite member set to be exhaustive over (§3);
        // an `s64` switch is legal and simply needs an `else` to be total.
        let enum_decl = match self.pool.item(scrutinee) {
            Item::EnumType { decl, flags } => Some((*decl, *flags)),
            _ => None,
        };
        // **A variant is exhaustible too**, over its *cases* rather than an enum's members (ADR-0068
        // §5). Its case names come from the struct side table, so the same set-judgement below serves
        // both — which is why this wave adds no diagnostic: E0258 and E0260 already say the right
        // things about "handles every member of".
        let variant_cases: Option<Vec<Symbol>> = match self.pool.item(scrutinee) {
            Item::VariantType { decl, .. } => Some(
                self.pool
                    .struct_fields(*decl)
                    .unwrap_or(&[])
                    .iter()
                    .map(|case| case.name)
                    .collect(),
            ),
            _ => None,
        };

        // Members named so far, and their arms' spans, so a duplicate is reported against the *later*
        // arm — the earlier one is the one that works.
        let mut seen_members: Vec<Symbol> = Vec::new();
        let mut seen_else: Option<Span> = None;

        for arm in arms {
            match arm.value {
                None => {
                    // A second `else` can never run.
                    if seen_else.is_some() {
                        self.diags.push(
                            Diagnostic::error(arm.span, "this `switch` already has an `else`")
                                .with_code(E0259)
                                .with_note("a second catch-all can never run"),
                        );
                    }
                    seen_else = Some(arm.span);
                }
                Some(case) => {
                    // Checked against the scrutinee's type, which is what resolves a bare `.RED` and
                    // what rejects a case of the wrong type through the ordinary mismatch (E0214).
                    let want = (scrutinee != PoolId::ERROR).then_some(scrutinee);
                    self.check_expr(scope, case, want);
                    // For an enum, remember *which* member so exhaustiveness and duplicate detection
                    // have something to compare. A case whose member cannot be named — a computed
                    // value, or an error — contributes nothing rather than a wrong entry.
                    if (enum_decl.is_some() || variant_cases.is_some())
                        && let Some(name) = self.case_member_name(body, case)
                    {
                        if seen_members.contains(&name) {
                            let text = self.interner.resolve(name).to_owned();
                            self.diags.push(
                                Diagnostic::error(
                                    arm.span,
                                    format!("`{text}` is already handled by an earlier `case`"),
                                )
                                .with_code(E0259)
                                .with_note("a duplicate case can never run"),
                            );
                        } else {
                            seen_members.push(name);
                        }
                    }
                }
            }
            self.check_stmt(body, arm.body);
        }

        // A variant's set judgement, the same shape as the enum one below but over its cases
        // (ADR-0068 §5). Written out rather than folded into one generic pass because the two get their
        // names from different tables, and a shared helper taking `Vec<Symbol>` would hide which.
        if let Some(cases) = &variant_cases {
            let missing: Vec<String> = cases
                .iter()
                .filter(|name| !seen_members.contains(name))
                .map(|name| self.interner.resolve(*name).to_owned())
                .collect();
            let text = self.describe(scrutinee);
            match (missing.is_empty(), seen_else) {
                (true, Some(else_span)) => {
                    self.diags.push(
                        Diagnostic::error(
                            else_span,
                            format!("this `else` can never run: every case of `{text}` is handled"),
                        )
                        .with_code(E0260)
                        .with_help("remove the `else`"),
                    );
                }
                (false, None) => {
                    let list = missing.join("`, `");
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("this `switch` does not handle every case of `{text}`"),
                        )
                        .with_code(E0258)
                        .with_note(format!("missing: `{list}`"))
                        .with_help("add a `case` for each, or an `else` arm"),
                    );
                }
                (true, None) | (false, Some(_)) => {}
            }
        }

        // The set judgement. Only for an enum: §3 restricts exhaustiveness to the type whose member set
        // is finite and known, which is what makes the diagnostic true rather than approximate.
        if let Some((decl, flags)) = enum_decl {
            let missing: Vec<String> = self
                .pool
                .enum_members(decl)
                .unwrap_or(&[])
                .iter()
                .filter(|member| !seen_members.contains(&member.name))
                .map(|member| self.interner.resolve(member.name).to_owned())
                .collect();
            let ty = self.pool.enum_type(decl, flags);
            let text = self.describe(ty);

            match (missing.is_empty(), seen_else) {
                // Every member named *and* an `else`: the `else` cannot run (§4).
                (true, Some(else_span)) => {
                    self.diags.push(
                        Diagnostic::error(
                            else_span,
                            format!(
                                "this `else` can never run: every member of `{text}` is handled"
                            ),
                        )
                        .with_code(E0260)
                        .with_help("remove the `else`"),
                    );
                }
                // Members missing and no `else`: not exhaustive. The names *are* the fix, so they are
                // listed rather than counted.
                (false, None) => {
                    let list = missing.join("`, `");
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("this `switch` does not handle every member of `{text}`"),
                        )
                        .with_code(E0258)
                        .with_note(format!("missing: `{list}`"))
                        .with_help("add a `case` for each, or an `else` arm"),
                    );
                }
                // Exhaustive by members, or made total by an `else`.
                (true, None) | (false, Some(_)) => {}
            }
        }
    }

    /// The enum member an arm's `case` value names, if it names one syntactically (ADR-0067 §3).
    ///
    /// Reads the *expression* rather than a folded value, because exhaustiveness is about which members
    /// were written: `case .RED` and `case Colour.RED` are the two spellings, and both carry the name.
    ///
    /// `None` for anything else — a computed value, a variable, an error — which contributes nothing to
    /// the member set rather than a wrong entry. That makes a `switch` whose arms are computed
    /// *non*-exhaustive, which is the honest answer: nothing here can prove it covers the members.
    fn case_member_name(&self, body: BodyId, case: ExprId) -> Option<Symbol> {
        match self.hir.body(body).exprs.get(case.index())? {
            // `case .RED` — a bare member, resolved from the scrutinee's type.
            Expr::Member { name, .. } => Some(*name),
            // `case Colour.RED` — qualified. The receiver is the enum, which the arm's type check
            // already agreed with, so the field name is the member.
            Expr::Field { name, .. } => Some(*name),
            _ => None,
        }
    }

    /// Types `p + n`, `n + p`, `p - n` and `p - q` (ADR-0064), or returns `None` for the numeric path.
    ///
    /// Called only for `+` and `-`, and only *before* the numeric handling, because a pointer operand
    /// must not be unified with an integer one — so each operand is typed with **no shared
    /// expectation** and the shape decided from the pair. `None` means "neither operand is a pointer",
    /// which hands the operation back to the ordinary numeric path unchanged; the hot case (`s64 +
    /// s64`) takes it after two `pointee` checks that both say no.
    ///
    /// `jr-mir` re-derives which of the three forms this is from the operands' recorded types rather
    /// than a side table — the same "read the `TypeMap`, do not recompute" discipline the overload
    /// path uses (ADR-0048 §5).
    fn check_pointer_arithmetic(
        &mut self,
        scope: ExprScope,
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
        span: Span,
    ) -> Option<PoolId> {
        // Typed with no expectation, so an integer operand defaults to `s64` and a pointer keeps its
        // type — the two are never made to match.
        let left = self.check_expr(scope, lhs, None);
        let right = self.check_expr(scope, rhs, None);
        let left_ptr = self.pointee(left);
        let right_ptr = self.pointee(right);

        match (left_ptr, right_ptr) {
            // Neither is a pointer: not our case, back to the numeric path. The operands are already
            // typed, and re-typing them there with a numeric expectation is harmless — `check_expr`
            // overwrites the same `TypeMap` entry with the same or a more specific type.
            (None, None) => None,
            // Both pointers. `p + q` is meaningless; `p - q` (the pointer difference) is deferred to
            // its own wave (ADR-0064 §5), because its element-count result needs the stride, which is
            // layout `jr-mir` does not carry. Both are E0223 — the operator does not fit here.
            (Some(_), Some(_)) => {
                let text = self.describe(left);
                self.reject_operator(op, &text, span);
                Some(PoolId::ERROR)
            }
            // `p + n` or `p - n`: pointer on the left, integer on the right. Result is the pointer.
            (Some(_), None) => {
                if self.int_info(right).is_some() {
                    Some(left)
                } else {
                    let rtext = self.describe(right);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("a pointer can only be offset by an integer, not `{rtext}`"),
                        )
                        .with_code(E0223),
                    );
                    Some(PoolId::ERROR)
                }
            }
            // `n + p`: integer on the left, pointer on the right. Legal only for `+` — `n - p` is
            // "an integer minus a pointer", which has no meaning (the distance is `p - n`, the other
            // order). Result is the pointer.
            (None, Some(_)) => {
                if op == BinOp::Add && self.int_info(left).is_some() {
                    Some(right)
                } else if op == BinOp::Sub {
                    let ltext = self.describe(left);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("cannot subtract a pointer from `{ltext}`"),
                        )
                        .with_code(E0223)
                        .with_note("write `p - n` to move a pointer back, not `n - p`"),
                    );
                    Some(PoolId::ERROR)
                } else {
                    let ltext = self.describe(left);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("a pointer can only be offset by an integer, not `{ltext}`"),
                        )
                        .with_code(E0223),
                    );
                    Some(PoolId::ERROR)
                }
            }
        }
    }

    /// Types both operands of a binary operation.
    ///
    /// With no context of its own, whichever side has a type decides the other's,
    /// so that `ptr.* == 9` and `9 == ptr.*` behave the same and neither forces
    /// the literal to `s64` before the comparison is considered.
    fn check_operands(
        &mut self,
        scope: ExprScope,
        lhs: ExprId,
        rhs: ExprId,
        want: Option<PoolId>,
    ) -> (PoolId, PoolId) {
        if let Some(ty) = want {
            let left = self.check_expr(scope, lhs, Some(ty));
            let right = self.check_expr(scope, rhs, Some(ty));
            return (left, right);
        }
        if self.is_untyped_literal(scope, lhs) && !self.is_untyped_literal(scope, rhs) {
            let right = self.check_expr(scope, rhs, None);
            let left = self.check_expr(scope, lhs, Some(right));
            return (left, right);
        }
        let left = self.check_expr(scope, lhs, None);
        let right = self.check_expr(scope, rhs, Some(left));
        (left, right)
    }

    /// Requires both operands to have the same type, returning it.
    fn unify_operands(&mut self, left: PoolId, right: PoolId, span: Span) -> PoolId {
        if left == PoolId::ERROR || right == PoolId::ERROR {
            return PoolId::ERROR;
        }
        if left == right {
            return left;
        }
        let (left_text, right_text) = (self.describe(left), self.describe(right));
        self.diags.push(
            Diagnostic::error(
                span,
                format!("mismatched operand types: `{left_text}` and `{right_text}`"),
            )
            .with_code(E0214)
            .with_note("Jairs does not convert between types implicitly (ADR-0015)"),
        );
        PoolId::ERROR
    }

    /// Resolves an operator to an overload, typing the operands, or `None` for the builtin path.
    ///
    /// Returns the overload's *return* type. `None` means "no overload applies" — which is the
    /// answer for every operator in every file that declares none, so this is the hot path and it
    /// exits on a `has_operators` check before typing anything.
    ///
    /// The resolved procedure is recorded in [`CheckOutput::operator_calls`] so that `jr-mir` can
    /// lower the call **without re-running resolution**: two implementations of one rule are two
    /// chances to disagree, which is why `jr-mir` reads `TypeMap` rather than recomputing types.
    fn check_operator_overload_call(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
        span: Span,
    ) -> Option<PoolId> {
        // `&&`/`||` are control flow and never reach an overload (ADR-0048 §2); bailing here
        // rather than in the lookup keeps their short-circuit path untouched.
        if matches!(op, BinOp::And | BinOp::Or) {
            return None;
        }
        if !self.any_operators_in_scope() {
            return None;
        }

        // Typed with no expectation, because an overload's operand types are what the *lookup*
        // keys on: imposing a context would decide the answer before asking the question.
        let left = self.check_expr(scope, lhs, None);
        let right = self.check_expr(scope, rhs, None);
        if left == PoolId::ERROR || right == PoolId::ERROR {
            return None;
        }

        let (proc, file) = self.find_operator(op, left, right, span)?;
        let ret = self
            .sigs_for_file(file)
            .and_then(|sigs| sigs.proc_sig(proc).map(|sig| sig.ret))
            .unwrap_or(PoolId::ERROR);
        self.operator_calls.insert((scope, id), (file, proc));
        Some(ret)
    }

    /// Reports an operator that does not apply to its operand type.
    fn reject_operator(&mut self, op: BinOp, ty_text: &str, span: Span) {
        let text = bin_op_text(op);
        self.diags.push(
            Diagnostic::error(
                span,
                format!("operator `{text}` is not supported for `{ty_text}`"),
            )
            .with_code(E0223),
        );
    }

    /// Reports an operator floats do not have, naming why rather than saying "unsupported".
    ///
    /// `%` is undefined on floats because C's `fmod` truncates toward zero while Python's `%`
    /// follows the sign of the divisor, and they disagree on `-1.0 % 3.0` — a language
    /// decision with no forcing constraint yet (ADR-0040 §7). The wrapping operators have no
    /// float meaning at all: they are ADR-0002's opt-out from *integer* overflow, and nothing
    /// about IEEE-754 wraps.
    fn reject_float_operator(&mut self, op: BinOp, ty_text: &str, span: Span) {
        let text = bin_op_text(op);
        let note = match op {
            BinOp::Rem => {
                "the sign of a float remainder is a language decision Jairs has not taken: \
                 C's `fmod` truncates toward zero and Python's `%` follows the divisor"
            }
            _ => {
                "the wrapping operators opt out of ADR-0002's integer overflow trap, and \
                 floating-point arithmetic does not overflow — it saturates to infinity"
            }
        };
        self.diags.push(
            Diagnostic::error(
                span,
                format!("operator `{text}` is not supported for `{ty_text}`"),
            )
            .with_code(E0223)
            .with_note(note),
        );
    }

    /// Reports an operator an enum does not have, and says how to get the number (ADR-0041 §6).
    ///
    /// Ordering is refused because with auto-numbering `Colour.RED < Colour.GREEN` would be
    /// true by an accident of *declaration order* — a fact about the source file rather than
    /// about colours. Arithmetic is refused because `Colour.RED + 1` names no member.
    ///
    /// Both notes end in the same advice, because `cast(s64, c)` genuinely is the answer: it
    /// gives ordering and arithmetic on an `s64`, where they mean something.
    fn reject_enum_operator(&mut self, op: BinOp, ty_text: &str, span: Span) {
        let text = bin_op_text(op);
        let note = if is_arithmetic(op) {
            "an enum's members are named alternatives, not magnitudes, so arithmetic on one \
             has no meaning as a member"
        } else {
            "an enum's members are named alternatives, not magnitudes: with auto-numbering \
             an ordering would be true by an accident of declaration order"
        };
        self.diags.push(
            Diagnostic::error(
                span,
                format!("operator `{text}` is not supported for `{ty_text}`"),
            )
            .with_code(E0223)
            .with_note(note)
            .with_help("compare with `==`, or use `cast(s64, x)` to work with the number"),
        );
    }

    /// Reports a bitwise operator on a type that has no bits to work on (ADR-0042 §5).
    ///
    /// The note distinguishes the two reachable cases, because the *advice* differs: a float
    /// has bits but not meaningful ones, while an enum's members genuinely are combinable —
    /// which is what `enum_flags` will be for.
    fn reject_bitwise(&mut self, op: BinOp, ty_text: &str, span: Span) {
        let text = bin_op_text(op);
        let mut diag = Diagnostic::error(
            span,
            format!("operator `{text}` is not supported for `{ty_text}`"),
        )
        .with_code(E0223)
        .with_note("bitwise operators apply to integers and to `enum_flags`");
        if jr_pool::FloatKind::from_name(ty_text).is_some() {
            diag = diag.with_note(
                "a float's bits are a sign, an exponent and a mantissa, so combining two of \
                 them bitwise is not the combination of anything meaningful",
            );
        }
        self.diags.push(diag);
    }

    /// Reports a bitwise operator on a **plain** enum, naming `enum_flags` (ADR-0043 §4).
    ///
    /// A separate message from [`Ctx::reject_bitwise`] because the answer differs: a plain
    /// enum's members are named alternatives and combining them is meaningless, but the
    /// programmer who tried almost certainly wanted a set — and cannot find `enum_flags` if
    /// nothing mentions it.
    fn reject_bitwise_on_plain_enum(&mut self, op: BinOp, ty_text: &str, span: Span) {
        let text = bin_op_text(op);
        self.diags.push(
            Diagnostic::error(
                span,
                format!("operator `{text}` is not supported for `{ty_text}`"),
            )
            .with_code(E0223)
            .with_note(
                "a plain `enum`'s members are named alternatives, so combining two of them \
                 bitwise names no member",
            )
            .with_help("declare it `enum_flags` if its members are meant to combine"),
        );
    }

    /// Reports a shift on a flags enum, which accepts every other bitwise operator.
    ///
    /// The distinction matters because "bitwise operators apply to integers and to
    /// `enum_flags`" is *true* and yet would leave the reader confused: they used a bitwise
    /// operator on an `enum_flags` and were refused.
    fn reject_shift_on_flags(&mut self, op: BinOp, ty_text: &str, span: Span) {
        let text = bin_op_text(op);
        self.diags.push(
            Diagnostic::error(
                span,
                format!("operator `{text}` is not supported for `{ty_text}`"),
            )
            .with_code(E0223)
            .with_note(
                "shifting a flag set would produce another member by an accident of the \
                 numbering; `& | ^ ~` are the operators a flag set has",
            )
            .with_help("use `cast(s64, x)` if the numeric value is what you want to shift"),
        );
    }

    /// Whether `ty` is an `enum_flags` type.
    fn is_flags(&self, ty: PoolId) -> bool {
        matches!(self.pool.item(ty), Item::EnumType { flags: true, .. })
    }

    /// Types a unary operation.
    fn check_unary(
        &mut self,
        scope: ExprScope,
        op: UnOp,
        operand: ExprId,
        expected: Option<PoolId>,
        span: Span,
    ) -> PoolId {
        match op {
            // `~` is integers only, and refusing it on a `bool` is the point: `!` is the
            // boolean negation, and a `bool`'s complement is 254 — not a `bool` at all
            // (ADR-0042 §4).
            UnOp::BitNot => {
                // A flags enum too (ADR-0043 §3): `~Perm.READ` is the complement of a set and
                // keeps the flags type.
                let want = expected.filter(|ty| self.int_info(*ty).is_some() || self.is_flags(*ty));
                let ty = self.check_expr(scope, operand, want);
                if ty != PoolId::ERROR && self.int_info(ty).is_none() && !self.is_flags(ty) {
                    let text = self.describe(ty);
                    let mut diag = Diagnostic::error(
                        span,
                        format!("operator `~` is not supported for `{text}`"),
                    )
                    .with_code(E0223)
                    .with_note("`~` is a bitwise complement and applies to integers");
                    if ty == PoolId::BOOL {
                        diag = diag.with_help("use `!` to negate a `bool`");
                    }
                    self.diags.push(diag);
                    return PoolId::ERROR;
                }
                self.expect(expected, ty, span)
            }
            UnOp::Neg => {
                let want = expected.filter(|ty| self.is_numeric(*ty));
                let ty = self.check_expr(scope, operand, want);
                // Negation is total on floats — it flips the sign bit — and traps on the most
                // negative integer (ADR-0002). Both are accepted here; the difference lives in
                // the arithmetic, not the type check.
                if ty != PoolId::ERROR && !self.is_numeric(ty) {
                    let text = self.describe(ty);
                    self.diags.push(
                        Diagnostic::error(
                            span,
                            format!("operator `-` is not supported for `{text}`"),
                        )
                        .with_code(E0223),
                    );
                    return PoolId::ERROR;
                }
                self.expect(expected, ty, span)
            }
            UnOp::Not => {
                self.check_expr(scope, operand, Some(PoolId::BOOL));
                self.expect(expected, PoolId::BOOL, span)
            }
            UnOp::AddrOf => {
                if !self.is_place(scope, operand) {
                    self.diags.push(
                        Diagnostic::error(span, "cannot take the address of this expression")
                            .with_code(E0221)
                            .with_note("only variables, fields, and dereferences have an address"),
                    );
                }
                // `f: *s64 = *a;` pushes `s64` into the operand rather than
                // letting it default.
                let want = expected.and_then(|ty| self.pointee(ty));
                let ty = self.check_expr(scope, operand, want);
                if ty == PoolId::ERROR {
                    return PoolId::ERROR;
                }
                let pointer = self.pool.pointer_to(ty);
                self.expect(expected, pointer, span)
            }
        }
    }

    /// Types a call.
    fn check_call(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        callee: ExprId,
        args: &[ExprId],
        arg_names: &[Option<Symbol>],
        span: Span,
    ) -> PoolId {
        // **`type_info(T)` is an intrinsic and is handled before anything else** (ADR-0075 §2),
        // because its argument is a *type* and every path below types an argument as a runtime value —
        // which is exactly the E0261 refusal. Intercepting here rather than teaching the general
        // argument check about a special case keeps that refusal intact everywhere else.
        //
        // A call rather than a directive (`#type_info`) because a directive cannot be passed as a value
        // or composed, and ADR-0071 already makes a type an argument-position value.
        match self.intrinsic_named(scope, callee) {
            Some(Intrinsic::TypeInfo) => {
                return self.check_type_info(scope, id, callee, args, span);
            }
            // `any_of` and `any_as` are intercepted for the same reason one level weaker: `any_as`'s
            // second argument is a *type*, and `any_of`'s pointer needs the erasing conversion §1
            // allows here and nowhere else.
            Some(Intrinsic::AnyOf) => return self.check_any_of(scope, id, callee, args, span),
            Some(Intrinsic::AnyAs) => return self.check_any_as(scope, id, callee, args, span),
            // **A note reader folds here** (ADR-0099 §2), and it is intercepted for a *third* reason: its
            // first argument is a declaration named as a value, and its answer is in the HIR rather than in
            // any type. Nothing below could type `has_note(f, "x")` — `f` is a procedure used as a value,
            // which is legal, but the call would then be an ordinary call to a `bool`-returning procedure
            // that does not exist.
            Some(Intrinsic::HasNote) => {
                return self.check_note_reader(scope, id, callee, args, span, false);
            }
            Some(Intrinsic::NoteValue) => {
                return self.check_note_reader(scope, id, callee, args, span, true);
            }
            // **The query side** (ADR-0100 §1): these ask about the *file* rather than a named
            // declaration, so unlike the reader they take no declaration at all.
            Some(Intrinsic::NotedCount) => {
                return self.check_noted_count(scope, id, callee, args, span);
            }
            Some(Intrinsic::NotedName) => {
                return self.check_noted_name(scope, id, callee, args, span);
            }
            // **The loop, at last** (ADR-0153 §1). `noted_count` and `noted_name` above can only be
            // unrolled to a guessed bound, which ADR-0100 §2 stated as an honest limit and named the
            // mechanism it was waiting for. ADR-0152 built that mechanism, so this folds to a *table* and
            // a program iterates it.
            Some(Intrinsic::NotedDeclarations) => {
                return self.check_noted_declarations(scope, id, callee, args, span);
            }
            // **The loop lives inside the fold** (ADR-0101 §1). ADR-0100 §2 established that folding can
            // never take a `for` variable as an argument; it said nothing about looping *within* the fold,
            // which is what this does — and for code *generation* that is not a workaround but the right
            // shape, since a run-time loop could not declare anything anyway.
            Some(Intrinsic::NotedInsert) => {
                return self.check_noted_insert(scope, id, callee, args, span);
            }
            // **`size_of(T)` folds like `type_info`'s size field**, and for the same reason its argument is
            // intercepted here: it is a *type*, and every path below would type it as a runtime value
            // (ADR-0106 §1).
            Some(Intrinsic::SizeOf) => return self.check_size_of(scope, id, callee, args, span),
            // **`typed`/`untyped` are the allocation boundary** (ADR-0106 §1). Intercepted because
            // `typed`'s first argument is a type, and because the conversion they perform is the one E0232
            // refuses everywhere else — permitted *here* the way ADR-0076 §1 permitted an erasing
            // conversion only at an `Any` boundary.
            Some(Intrinsic::Typed) => return self.check_typed(scope, id, callee, args, span),
            Some(Intrinsic::Untyped) => return self.check_untyped(scope, id, callee, args, span),
            // **`view(p, n)` builds a `[]T` from a pointer and a count** (ADR-0109 §1). Intercepted like the
            // other boundary intrinsics because its *result* type comes from an argument's pointee rather than
            // from anything the ordinary call path could compute.
            Some(Intrinsic::View) => return self.check_view(scope, id, callee, args, span),
            None => {}
        }

        // **A call to an *imported* polymorphic procedure is refused** (E0268, ADR-0104 §2). Cross-file
        // instantiation is deferred (ADR-0082 §5), and `callee_poly` deliberately returns `None` for an
        // imported template — but the claim in its own docs, that the template's signature then "reports an
        // honest mismatch", turned out to be **false**: a `$T` parameter's type is `PoolId::ERROR`, which
        // matches anything, so the call was *accepted* and reached the engines as "no routine for file 2
        // proc 0" — a leaked internal error for a program the module boundary forbids. Refused here, before
        // the ordinary path, so the deferral is a diagnostic rather than an ICE.
        if let Some(name) = self.imported_template_callee(scope, callee) {
            self.check_expr(scope, callee, None);
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "`{}` is polymorphic and declared in another module",
                        self.interner.resolve(name)
                    ),
                )
                .with_code(E0268)
                .with_note(
                    "instantiating a template across a module boundary is not yet supported, so the \
                     compiler cannot build a concrete copy of it here",
                )
                .with_help(
                    "wrap it in a non-polymorphic procedure in the module that declares it, and call that",
                ),
            );
            return PoolId::ERROR;
        }

        // **A call to a polymorphic procedure is instantiated** (ADR-0082 §1): infer each `$T` from the
        // corresponding argument, record `(proc, bound types)` for the expansion pass, and return the
        // concrete return type. Handled before the ordinary call path, whose signature is a template with
        // `ERROR` parameters that a direct type-check would compare `42` against.
        if let Some((proc, sig)) = self.callee_poly(scope, callee) {
            // **A `#modify` predicate now runs** (ADR-0095 §1): the call is instantiated like any other,
            // and the predicate's clone is evaluated in `file_mir` — a `false` there refuses this
            // instantiation with E0275. ADR-0093 §3's E0274 refusal is lifted, exactly as E0268 was for
            // `$T` and E0271's first meaning for `$N`: each such refusal named the sub-wave that removes it.
            return self.check_polymorphic_call(scope, id, callee, proc, &sig, args, span);
        }

        // **A call to a `#expand` macro is refused, by design** (ADR-0090 §3): a macro's body must be
        // *spliced* into this scope, and the splice is the next sub-wave. Refused rather than allowed to
        // fall through to the ordinary call path, which is what happened before this check existed —
        // `#expand` was accepted and silently behaved as an ordinary procedure, the "a directive that is
        // ignored is worse than one that is rejected" failure ADR-0058 §3 names. Arguments are still
        // typed, so an error inside one is reported too.
        // **A call to an *imported* macro is refused** (ADR-0091 §3). A same-file macro call never reaches
        // here: `jr-hir`'s lowering splices it away before sema sees a call at all. What does reach here is
        // a **cross-file** one, because the macro-body map is built per file — and before this refusal it
        // reached the VM as "internal compiler error: no routine for file 1 proc 0", the fifth time
        // compiler internals leaked for a reasonable program. Refused with a sentence a reader can act on.
        if self.callee_is_imported_macro(scope, callee) {
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            self.diags.push(
                Diagnostic::error(
                    span,
                    "a `#expand` macro cannot yet be called from another file",
                )
                .with_code(E0272)
                .with_note(
                    "a macro is spliced from its own file's source text, and that text is not carried \
                     across a module boundary yet (ADR-0091 §3)",
                )
                .with_help("move the macro into this file, or make it an ordinary procedure"),
            );
            return PoolId::ERROR;
        }

        // **A call to a comptime-value-parameterised procedure is instantiated** (ADR-0088 §1): its `$N`
        // arguments are recorded as *expressions* for the `jr-db` pre-pass to evaluate to constants — a
        // value is not known here, because const-eval is downstream (ADR-0018 §3). Handled before the
        // ordinary call path, whose template signature has concrete `$N` parameter types a direct check
        // would accept while leaving `N` with no value — a placeholder miscompile.
        if let Some((proc, sig)) = self.callee_comptime_template(scope, callee) {
            return self.check_comptime_call(scope, id, callee, proc, &sig, args, span);
        }

        // The callee is in **call position**, where a `#foreign` procedure is a legal thing to
        // name — it is only illegal to take one as a *value* (E0256, ADR-0059 §5). This id is
        // recorded so `check_expr`'s `Name` arm skips the E0256 refusal for it, while still typing
        // and `set_expr`-recording the callee exactly as every other expression. Skipping
        // `check_expr` entirely (an earlier attempt) left the callee's type unrecorded, which
        // surfaced as MIR's "an expression was never typed" on `write(…)`.
        self.call_position.insert((scope, callee));
        let callee_ty = self.check_expr(scope, callee, None);
        // Copy the signature out before touching `self` again: the pool borrow
        // and the diagnostic sink cannot both be live.
        let signature = match self.pool.item(callee_ty) {
            Item::ProcType { params, ret, .. } => Some((params.clone(), *ret)),
            _ => None,
        };

        let Some((params, ret)) = signature else {
            if callee_ty != PoolId::ERROR {
                let text = self.describe(callee_ty);
                self.diags.push(
                    Diagnostic::error(span, format!("expected a procedure, found `{text}`"))
                        .with_code(E0215),
                );
            }
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            return PoolId::ERROR;
        };

        // **Named arguments and defaults are resolved into positional order here** (ADR-0053 §1),
        // before the arity check and the type check, so both work on one shape. The result is
        // recorded so `jr-mir` reads it instead of the source order — one pass decides argument
        // order, which is the same split ADR-0048 §5 made for overload resolution.
        let named = arg_names.iter().any(Option::is_some);
        let has_defaults = self
            .callee_sig(scope, callee)
            .is_some_and(|sig| sig.defaults.iter().any(Option::is_some));
        if named || has_defaults {
            if let Some(filled) = self.fill_arguments(scope, callee, args, arg_names, span) {
                for (index, slot) in filled.iter().enumerate() {
                    let want = params.get(index).copied();
                    if let ArgSlot::Given(arg) = slot {
                        self.check_arg(scope, *arg, want);
                    }
                }
                self.filled_calls.insert((scope, id), filled);
                return ret;
            }
            // `fill_arguments` reported the problem; type what was written so a second error in an
            // argument is still found, then poison.
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            return PoolId::ERROR;
        }

        // A variadic last parameter (ADR-0138 §1) accepts *any number* of trailing arguments,
        // and MIR packs them into a stack `[N]T` view (ADR-0139 §1). Two shapes reach here:
        //
        //  * `args.len() == params.len()` — the caller supplied *exactly one* value for the
        //    variadic slot. That is either the packing case with N=1, or a caller passing
        //    an explicit `[]T` view. Sema cannot tell them apart from types alone (a `[]T`
        //    argument to a `[]T` parameter is legal without packing), so the record-vs-pack
        //    decision lives in MIR: it treats a single arg whose type is already the view
        //    type as pass-through, and any other one arg as N=1 packing;
        //  * `args.len() != params.len()` — the fixed prefix plus a run of trailing args.
        //    Packing is unavoidable.
        //
        // The trailing args are typed against the **element type** — the view's `elem` —
        // rather than against the parameter's `[]T`. That is what makes `sum(1, 2, 3)`
        // type-check with `..s64`: each `1`, `2`, `3` is checked against `s64`, not against
        // `[]s64`.
        let callee_sig_v = self.callee_sig(scope, callee);
        let last_variadic = callee_sig_v
            .as_ref()
            .map(|s| variadic_last_param(&s.variadic_params))
            .unwrap_or(false);
        let (fixed_arg_count, variadic_view_ty, variadic_elem) = if last_variadic {
            let last_ty = *params.last().unwrap();
            let elem = match self.pool.item(last_ty) {
                jr_pool::Item::ViewType { elem } => Some(*elem),
                _ => None,
            };
            (params.len() - 1, Some(last_ty), elem)
        } else {
            (params.len(), None, None)
        };

        let arity_ok = if last_variadic {
            args.len() >= fixed_arg_count
        } else {
            args.len() == params.len()
        };
        if !arity_ok {
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "this procedure takes {}{} argument{}, but {} {} supplied",
                        if last_variadic { "at least " } else { "" },
                        if last_variadic {
                            fixed_arg_count
                        } else {
                            params.len()
                        },
                        if (if last_variadic {
                            fixed_arg_count
                        } else {
                            params.len()
                        }) == 1
                        {
                            ""
                        } else {
                            "s"
                        },
                        args.len(),
                        if args.len() == 1 { "was" } else { "were" }
                    ),
                )
                .with_code(E0216),
            );
        }

        // For a variadic call: check the fixed args against their parameters; the trailing
        // args are either (a) a single explicit `[]T` view — pass-through — or (b) a run of
        // one-or-more `T` values, packed into a stack view by MIR (ADR-0139 §1). The two
        // shapes overlap at "exactly one trailing arg", where the natural type distinguishes
        // them: a view type means pass-through, anything else means pack.
        let pack_all_trailing = last_variadic && arity_ok;
        let mut pack_this_call = false;
        for (index, arg) in args.iter().enumerate() {
            if pack_all_trailing && index >= fixed_arg_count {
                let trailing = args.len() - fixed_arg_count;
                if trailing == 1 {
                    // Type with no target so mismatches do not fire against either candidate;
                    // then decide pass-through vs pack based on the natural type.
                    let natural = self.check_expr(scope, *arg, None);
                    if variadic_view_ty.map(|v| v == natural).unwrap_or(false) {
                        // Pass-through: the single arg is the view. No packing.
                        continue;
                    }
                    pack_this_call = true;
                    // Enforce the element type — a mismatch here is the honest error. A `*U` into a
                    // `..Any` coerces exactly as the multi-argument path's `check_arg` does (ADR-0141),
                    // reusing the type already computed above so the argument is not re-checked.
                    if let Some(elem) = variadic_elem
                        && !self.record_any_coercion(scope, *arg, elem, natural)
                        && natural != PoolId::ERROR
                        && natural != elem
                    {
                        let arg_desc = self.describe(natural);
                        let elem_desc = self.describe(elem);
                        let view_desc = variadic_view_ty
                            .map(|v| self.describe(v))
                            .unwrap_or_else(|| String::from("[]T"));
                        self.diags.push(
                            Diagnostic::error(
                                span,
                                format!(
                                    "variadic argument expected `{elem_desc}` (element) or `{view_desc}` (explicit view), found `{arg_desc}`"
                                ),
                            )
                            .with_code(E0214),
                        );
                    }
                    continue;
                }
                // Multiple trailing args: definitely packing, and each must match the
                // element type.
                pack_this_call = true;
                self.check_arg(scope, *arg, variadic_elem);
                continue;
            }
            let want = params.get(index).copied();
            self.check_arg(scope, *arg, want);
        }

        // Record the packing info if any of the trailing args need packing (ADR-0139 §2).
        // The variadic sink for a zero-trailing call is empty but still recorded so MIR's
        // `variadic_call` lookup sees it and packs an empty view — otherwise a call to
        // `sum()` would arity-mismatch, the callee's parameter list expecting one view.
        if pack_all_trailing && (pack_this_call || args.len() == fixed_arg_count) {
            self.variadic_calls.insert(
                (scope, id),
                VariadicCall {
                    fixed_arg_count,
                    element_ty: variadic_elem.unwrap_or(PoolId::ERROR),
                },
            );
        }

        ret
    }

    /// Checks one call argument, erasing a pointer to `Any` where the parameter wants one (ADR-0076 §1).
    ///
    /// The ergonomic half of `any_of`: ADR-0076 §1 promised that "passing a `*T` where an `Any` is
    /// expected" erases, so a reflection procedure reads `takes(*x)` rather than `takes(any_of(*x))`. When
    /// the parameter type is the standard library's `Any` and the argument is a pointer, this records the
    /// same `AnyOp::Of` lowering the explicit call does — keyed by the argument expression — so `jr-mir`
    /// builds the `Any` there. Any other argument is checked normally.
    ///
    /// Deliberately narrow: only a **pointer** coerces, because §4 leaves a bare value (`a: Any = 3;`) for
    /// later — a value has no address, so it would need a materialised temporary this does not create.
    fn check_arg(&mut self, scope: ExprScope, arg: ExprId, want: Option<PoolId>) {
        if let Some(want_ty) = want
            && self.is_any_struct(want_ty)
        {
            let arg_ty = self.check_expr(scope, arg, None);
            if self.record_any_coercion(scope, arg, want_ty, arg_ty) {
                return;
            }
            // Not a coercible pointer: fall through to an ordinary mismatch against `Any`, which is the
            // honest error (`expected Any, found …`).
            let span = self.expr_of(scope, arg).span();
            self.expect(want, arg_ty, span);
            return;
        }
        self.check_expr(scope, arg, want);
    }

    /// Records the `*U`→`Any` coercion for `arg` when `want_ty` is `Any` and its already-computed type
    /// `arg_ty` is a pointer with a laid-out pointee (ADR-0076 §1). Returns whether it recorded one.
    ///
    /// Factored out of [`Self::check_arg`] so the **variadic single-argument** disambiguation (ADR-0141)
    /// reuses the identical decision: that path types the one trailing argument with no target to tell a
    /// pass-through view from a packed element, and must then apply the same coercion the multi-argument
    /// path gets from `check_arg` — otherwise `f(*x)` into a `..Any` reported a mismatch while `f(*x, *y)`
    /// coerced. Taking `arg_ty` as a parameter is what lets the caller reuse the type it already computed,
    /// so the argument is not checked twice and a malformed one is not diagnosed twice.
    ///
    /// The argument keeps its **pointer** type in the `TypeMap`, so `jr-mir`'s coercion wrapper lowers the
    /// pointer through the ordinary value path (`expr_inner`) and then wraps the result into an `Any`.
    fn record_any_coercion(
        &mut self,
        scope: ExprScope,
        arg: ExprId,
        want_ty: PoolId,
        arg_ty: PoolId,
    ) -> bool {
        if self.is_any_struct(want_ty)
            && let Item::PointerType(pointee) = *self.pool.item(arg_ty)
            // The pointee must have a layout, as `any_of` requires — the same E0266 the explicit form
            // raises, reused so the two paths agree.
            && jr_pool::layout_of(self.pool, jr_pool::TargetLayout::LP64, pointee).is_ok()
        {
            self.any_calls.insert((scope, arg), (AnyOp::Of, pointee));
            return true;
        }
        false
    }

    /// Whether a type is the standard library's `Any` (ADR-0076 §3).
    ///
    /// By identity against the looked-up struct, not by name, so a program's own unrelated `Any` is not
    /// mistaken for it. Silent when `Any` is not loaded — the coercion simply does not apply, and an
    /// ordinary mismatch results.
    fn is_any_struct(&mut self, ty: PoolId) -> bool {
        self.any_struct_quiet() == Some(ty)
    }

    /// The `Any` struct type, looked up **without** validating its shape or reporting (ADR-0076 §3).
    ///
    /// A quiet counterpart to `any_struct`: the argument-coercion check only needs to know whether a
    /// parameter's type *is* `Any`, and it runs for every call, so it must not report E0265 nor validate
    /// — a program that never touches reflection should pay nothing and see nothing. The explicit
    /// `any_of`/`any_as` intrinsics still validate through `any_struct`.
    fn any_struct_quiet(&mut self) -> Option<PoolId> {
        if self.imports.is_empty() {
            return None;
        }
        let name = self.interner.intern("Any");
        let entry = self
            .imports
            .iter()
            .find_map(|(_, sigs)| sigs.lookup(name))
            .or_else(|| self.sigs.lookup(name));
        let ty = entry.and_then(|e| e.type_value)?;
        matches!(self.pool.item(ty), Item::StructType { .. }).then_some(ty)
    }

    /// Which compiler intrinsic a callee names, if any.
    ///
    /// **By name, and only when the name resolves to nothing.** None of these is declared anywhere, so a
    /// program that declares its own `any_of` gets its own — the resolution succeeds and this answers
    /// `None`. Reserving the names outright would break a program that already used one, for no gain.
    fn intrinsic_named(&mut self, scope: ExprScope, callee: ExprId) -> Option<Intrinsic> {
        let Expr::Name { name, res, .. } = self.expr_of(scope, callee) else {
            return None;
        };
        let intrinsic = match self.interner.resolve(name) {
            "type_info" => Intrinsic::TypeInfo,
            "any_of" => Intrinsic::AnyOf,
            "any_as" => Intrinsic::AnyAs,
            "has_note" => Intrinsic::HasNote,
            "note_value" => Intrinsic::NoteValue,
            "noted_count" => Intrinsic::NotedCount,
            "noted_declarations" => Intrinsic::NotedDeclarations,
            "noted_name" => Intrinsic::NotedName,
            "noted_insert" => Intrinsic::NotedInsert,
            "size_of" => Intrinsic::SizeOf,
            "typed" => Intrinsic::Typed,
            "untyped" => Intrinsic::Untyped,
            "view" => Intrinsic::View,
            _ => return None,
        };
        match self.resolve.get(scope, callee).unwrap_or(res) {
            Res::Error => Some(intrinsic),
            Res::Item(_)
            | Res::Imported(_, _)
            | Res::Local(_)
            | Res::Param(_)
            | Res::Promoted { .. } => None,
        }
    }

    /// Types and **folds** `has_note(decl, "name")` or `note_value(decl, "name")` (ADR-0099 §1).
    ///
    /// Folded here rather than in `jr-db`, unlike `type_info`: the answer is in the HIR's `Proc::notes`,
    /// which this checker is already holding, so no layout, no VM and no query are involved. `payload`
    /// selects which of the two intrinsics this is — one function because they differ only in what they
    /// read out of the same looked-up note, and two would be two places to keep the lookup consistent.
    fn check_note_reader(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        callee: ExprId,
        args: &[ExprId],
        span: Span,
        payload: bool,
    ) -> PoolId {
        let intrinsic = if payload { "note_value" } else { "has_note" };
        // The callee names no procedure, so it is typed `void` rather than left unrecorded — MIR reports
        // "an expression was never typed" for a hole, and the fold means it is never lowered.
        self.types.set_expr(scope, callee, PoolId::VOID);

        if args.len() != 2 {
            self.wrong_intrinsic_arity(intrinsic, 2, args.len(), span);
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            return PoolId::ERROR;
        }

        let answer_ty = if payload {
            PoolId::STRING
        } else {
            PoolId::BOOL
        };

        // **The note name must be a literal string** (E0277, ADR-0099 §1), read before anything else so a
        // computed name is one diagnostic rather than a cascade.
        let Some(name) = self.string_literal_of(scope, args[1]) else {
            self.types.set_expr(scope, args[1], PoolId::STRING);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("`{intrinsic}`'s note name must be a string literal"),
                )
                .with_code(E0277)
                .with_note(
                    "the answer is folded while checking, so the name has to be readable then",
                )
                .with_help(format!(
                    "write it directly, e.g. `{intrinsic}(f, \"inline\")`"
                )),
            );
            return PoolId::ERROR;
        };
        self.types.set_expr(scope, args[1], PoolId::STRING);

        // **The declaration itself, not its name as text** (ADR-0099 §1): a misspelling is then an
        // ordinary unresolved-name error rather than a silent `false`, which is the failure mode
        // ADR-0098's dropped notes had and is worth not rebuilding in the reader.
        let Some(notes) = self.notes_of(scope, args[0]) else {
            // **The argument is marked a type position before being checked**, so a type name reports only
            // this refusal rather than E0261's "a type is a compile-time value" on top of it: two
            // diagnostics for one mistake, and the second one is about a rule this position does not have.
            // The allowlist gains an entry rather than E0261 gaining an exception — ADR-0071 §3's asymmetry.
            self.type_position.insert((scope, args[0]));
            self.check_expr(scope, args[0], None);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("`{intrinsic}` needs a procedure whose notes to read"),
                )
                .with_code(E0277)
                .with_note("only a procedure carries `@note`s today, since the parser takes them in the procedure attribute loop")
                .with_help(format!("name the declaration itself, e.g. `{intrinsic}(add, \"inline\")`")),
            );
            return PoolId::ERROR;
        };
        // Typed anyway, so the `TypeMap` has no hole where MIR would look. `VOID` rather than the
        // procedure's own type because the argument is never lowered and asking for a proc type here
        // would run the ordinary "a procedure used as a value" path for an expression that is not one.
        self.types.set_expr(scope, args[0], PoolId::VOID);

        // **An absent note answers `false` and `""`, and is not an error** (ADR-0099 §3): asking whether a
        // note is present is the point, so refusing the question when the answer is "no" would make the
        // predicate useless for what it exists for. The opposite call from `any_as`, which traps — and the
        // difference is that `any_as` would otherwise return garbage, while this returns the truth.
        let wanted = self.interner.intern(&name);
        let found = notes.iter().find(|(n, _)| *n == wanted);
        let value = if payload {
            // `""` for both "no such note" and "a bare `@name`", deliberately conflated: a caller
            // wanting a payload wants the payload or nothing, and telling the two apart needs an
            // optional return nothing in this wave has a use for.
            let text = found.and_then(|(_, p)| p.clone()).unwrap_or_default();
            self.pool.str_value(&text)
        } else {
            self.pool.bool_value(found.is_some())
        };
        self.record_fold(scope, id, span, value);

        self.expect(None, answer_ty, span)
    }

    /// Types and folds `noted_count("name")` (ADR-0100 §1).
    ///
    /// Asks about **the file**, not a named declaration, which is what makes it a *query* rather than a
    /// reader: it is the half a build script needs to act on declarations it was not written knowing about.
    fn check_noted_count(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        callee: ExprId,
        args: &[ExprId],
        span: Span,
    ) -> PoolId {
        self.types.set_expr(scope, callee, PoolId::VOID);
        if args.len() != 1 {
            self.wrong_intrinsic_arity("noted_count", 1, args.len(), span);
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            return PoolId::ERROR;
        }
        let Some(name) = self.note_name_argument(scope, args[0], "noted_count", span) else {
            return PoolId::ERROR;
        };
        let count = self.noted_declarations(name).len() as u64;
        // `s64` rather than an untyped literal, because the count is a real quantity a caller will compare
        // and index with, and ADR-0016 §1's context typing has no context to read at a folded call.
        let value = self.pool.int_value(PoolId::S64, count);
        self.record_fold(scope, id, span, value);
        self.expect(None, PoolId::S64, span)
    }

    /// Types and folds `noted_declarations("name")` to a `[]Declaration` table (ADR-0153 §1).
    ///
    /// # Why this is W6's headline claim rather than a convenience
    ///
    /// `noted_count` and `noted_name` let a script *unroll*: ask the count, then ask for name 0, name 1,
    /// and so on to a bound written into the script. ADR-0100 §2 recorded that as the boundary of folding
    /// itself rather than a spelling problem — a fold is answered while checking, and a `for` variable does
    /// not exist then — and named what it was waiting for: a compiler-emitted table. ADR-0152 built one.
    ///
    /// So this returns a view over a table the compiler emitted, and a metaprogram writes an ordinary
    /// `for` over it. The count is the table's, not a number the script guessed.
    ///
    /// # What is in it, and what is not
    ///
    /// A name and the note's value. Not a `Type_Info`, and not a procedure pointer: both would make this
    /// the *inspection* half of a message loop that also wants to *change* what it inspects, and ADR-0153
    /// §2 keeps those apart deliberately.
    fn check_noted_declarations(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        callee: ExprId,
        args: &[ExprId],
        span: Span,
    ) -> PoolId {
        self.types.set_expr(scope, callee, PoolId::VOID);
        if args.len() != 1 {
            self.wrong_intrinsic_arity("noted_declarations", 1, args.len(), span);
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            return PoolId::ERROR;
        }
        let Some(note) = self.note_name_argument(scope, args[0], "noted_declarations", span) else {
            return PoolId::ERROR;
        };

        let Some(decl_ty) = self.library_struct(span, "Declaration", DECLARATION_FIELDS) else {
            return PoolId::ERROR;
        };

        // Declaration order, for the reason `noted_declarations` gives: it is the one order a reader can
        // predict from the source.
        let found = self.noted_declarations_with_values(note);
        let mut entries = Vec::with_capacity(found.len());
        for (name, value) in &found {
            let name_text = self.interner.resolve(*name).to_owned();
            let name_value = self.pool.str_value(&name_text);
            let note_value = self.pool.str_value(value);
            entries.push(
                self.pool
                    .aggregate_value(decl_ty, vec![name_value, note_value]),
            );
        }
        let table = self.pool.static_array(decl_ty, entries);
        self.record_fold(scope, id, span, table);
        let view = self.pool.view_of(decl_ty);
        self.expect(None, view, span)
    }

    /// Types and folds `noted_name("name", i)` (ADR-0100 §1).
    ///
    /// **The index must be a literal**, for the reason the note name must be: this is answered while
    /// checking, and a `for` variable does not exist then. That is not a spelling limitation but the
    /// boundary of folding itself — genuine loop-driven iteration needs a compiler-emitted table, which is
    /// the mechanism `Type_Info`'s variable-length field list has been deferred for since ADR-0078
    /// (ADR-0100 §2). An out-of-range index answers `""` rather than being refused, because a script
    /// unrolling to a fixed bound is the intended use and its tail must be quiet.
    fn check_noted_name(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        callee: ExprId,
        args: &[ExprId],
        span: Span,
    ) -> PoolId {
        self.types.set_expr(scope, callee, PoolId::VOID);
        if args.len() != 2 {
            self.wrong_intrinsic_arity("noted_name", 2, args.len(), span);
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            return PoolId::ERROR;
        }
        let Some(name) = self.note_name_argument(scope, args[0], "noted_name", span) else {
            self.check_expr(scope, args[1], None);
            return PoolId::ERROR;
        };

        let index = self.int_literal_of(scope, args[1]);
        self.types.set_expr(scope, args[1], PoolId::S64);
        let Some(index) = index else {
            self.diags.push(
                Diagnostic::error(span, "`noted_name`'s index must be an integer literal")
                    .with_code(E0277)
                    .with_note(
                        "the answer is folded while checking, so the index has to be readable then — a \
                         `for` variable is not",
                    )
                    .with_help("unroll to a fixed bound, e.g. `noted_name(\"serialise\", 0)`"),
            );
            return PoolId::ERROR;
        };

        let found = self.noted_declarations(name);
        let text = usize::try_from(index)
            .ok()
            .and_then(|i| found.get(i).copied())
            .map(|sym| self.interner.resolve(sym).to_owned())
            .unwrap_or_default();
        let value = self.pool.str_value(&text);
        self.record_fold(scope, id, span, value);
        self.expect(None, PoolId::STRING, span)
    }

    /// Types and folds `size_of(T)` (ADR-0106 §1).
    ///
    /// Folded here rather than lowered, because the answer is `layout_of`'s and this crate already calls it —
    /// the same numbers `type_info(T).size` reports, from the same function, so the two cannot disagree about
    /// how large a type is (ADR-0075 §2's argument for sharing `layout_of`).
    ///
    /// It arrives now because **typed allocation asked for it**: a caller allocating `n` elements needs
    /// `n * size_of(T)` bytes, and until this sub-wave nothing could name that number. A facility with no
    /// caller is what ADR-0080 §3 declined to build; this one has one.
    fn check_size_of(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        callee: ExprId,
        args: &[ExprId],
        span: Span,
    ) -> PoolId {
        self.types.set_expr(scope, callee, PoolId::VOID);
        if args.len() != 1 {
            self.wrong_intrinsic_arity("size_of", 1, args.len(), span);
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            return PoolId::ERROR;
        }
        // The argument is a type, so the allowlist gains an entry rather than E0261 gaining an exception —
        // ADR-0071 §3's asymmetry, the same move `type_info` makes.
        self.type_position.insert((scope, args[0]));
        let described = self.described_type(scope, args[0]);
        self.types.set_expr(scope, args[0], PoolId::TYPE);
        let Some(described) = described else {
            // Withheld for an unbound `$T` of the enclosing template, exactly as `type_info`'s is
            // (ADR-0092 §1): `size_of(T)` inside a `$T` body is correct code, and each instantiation
            // resolves `T` for real.
            if let Expr::Name { name, .. } = self.expr_of(scope, args[0])
                && self.poly_var_names.contains(&name)
            {
                return PoolId::ERROR;
            }
            self.diags.push(
                Diagnostic::error(span, "`size_of` needs a type")
                    .with_code(E0261)
                    .with_note("its argument is the type to measure, e.g. `size_of(s64)`"),
            );
            return PoolId::ERROR;
        };
        let Ok(layout) = jr_pool::layout_of(self.pool, jr_pool::TargetLayout::LP64, described)
        else {
            let text = self.describe(described);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("`size_of` cannot measure `{text}`, which has no runtime layout"),
                )
                .with_code(E0266)
                .with_note("a compile-time-only type has no size a program could allocate"),
            );
            return PoolId::ERROR;
        };
        let value = self.pool.int_value(PoolId::S64, layout.size);
        self.record_fold(scope, id, span, value);
        self.expect(None, PoolId::S64, span)
    }

    /// Types `typed(T, p)` — a `*u8` viewed as a `*T` (ADR-0106 §1).
    ///
    /// **This is the one place a pointer's pointee type may change**, and it is deliberately not a `cast`:
    /// E0232 refuses `cast(*s64, p)` because a general pointer cast makes a wrong pointee a *silent wrong
    /// read* (ADR-0045 §1), and that refusal stays. What is permitted here is narrower in the way that
    /// matters — the target type is written as a **type argument**, at a named boundary a reader can search
    /// for, exactly as ADR-0076 §1 permitted an erasing conversion only at an `Any` boundary.
    ///
    /// It does not make the conversion *safe*: `typed(s64, p)` on a `p` that points at four bytes is still
    /// wrong. It makes it **visible and searchable**, which a `cast` buried in an expression is not.
    fn check_typed(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        callee: ExprId,
        args: &[ExprId],
        span: Span,
    ) -> PoolId {
        self.types.set_expr(scope, callee, PoolId::VOID);
        if args.len() != 2 {
            self.wrong_intrinsic_arity("typed", 2, args.len(), span);
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            return PoolId::ERROR;
        }
        self.type_position.insert((scope, args[0]));
        let described = self.described_type(scope, args[0]);
        self.types.set_expr(scope, args[0], PoolId::TYPE);
        let Some(described) = described else {
            self.check_expr(scope, args[1], None);
            self.diags.push(
                Diagnostic::error(span, "`typed` needs a type to view the pointer as")
                    .with_code(E0261)
                    .with_note("its first argument is the pointee type, e.g. `typed(s64, p)`"),
            );
            return PoolId::ERROR;
        };

        let operand = self.check_expr(scope, args[1], None);
        if operand == PoolId::ERROR {
            return PoolId::ERROR;
        }
        // **The operand must be a `*u8`**, not any pointer. `typed` exists to give a *fresh allocation* a
        // type, and an allocator hands back bytes; allowing `*T` → `*U` would be the general cast E0232
        // refuses, reached by another spelling.
        let is_byte_pointer =
            matches!(self.pool.item(operand), Item::PointerType(p) if *p == PoolId::U8);
        if !is_byte_pointer {
            let text = self.describe(operand);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("`typed` needs a `*u8`, but this is `{text}`"),
                )
                .with_code(E0279)
                .with_note(
                    "it gives an untyped allocation a type; converting one typed pointer to another is \
                     the cast E0232 refuses, because a wrong pointee type is a silent wrong read",
                )
                .with_help("allocate with `malloc`, which returns a `*u8`"),
            );
            return PoolId::ERROR;
        }
        if let Err(error) = jr_pool::layout_of(self.pool, jr_pool::TargetLayout::LP64, described) {
            let text = self.describe(described);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("`typed` cannot view memory as `{text}`, which has no runtime layout"),
                )
                .with_code(E0266)
                .with_note(format!("the layout is unavailable because {error}")),
            );
            return PoolId::ERROR;
        }
        let result = self.pool.pointer_to(described);
        self.pointer_views.insert((scope, id), result);
        self.expect(None, result, span)
    }

    /// Types `view(p, count)` — a `[]T` over `count` elements at `p` (ADR-0109 §1).
    ///
    /// **The element type comes from the pointer**, not from an argument, so nothing is asserted: `view` on a
    /// `*s64` is a `[]s64` and cannot be anything else. That is the property that made `typed` acceptable while
    /// `cast` stayed refused (ADR-0106 §1), and it is why this needs no type argument.
    ///
    /// **The count is unchecked, and that is stated rather than hidden.** A pointer's allocation size is not
    /// tracked anywhere — `malloc` returns a bare address and no shadow table records what was asked for — so a
    /// checked `view` would need an allocation registry, which the native back end could not share with the VM.
    /// So this is in the same honest category as `typed`: it does not make the operation safe, it makes it
    /// **visible and searchable**.
    ///
    /// It exists because a growable array could not hand its contents to `Sort` or `String` (ADR-0107's closing
    /// gap): a slice takes an *array*, so nothing could turn a pointer and a count into a view.
    fn check_view(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        callee: ExprId,
        args: &[ExprId],
        span: Span,
    ) -> PoolId {
        self.types.set_expr(scope, callee, PoolId::VOID);
        if args.len() != 2 {
            self.wrong_intrinsic_arity("view", 2, args.len(), span);
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            return PoolId::ERROR;
        }

        let pointer = self.check_expr(scope, args[0], None);
        // The count is an `s64`, pushed into the argument so a literal takes that type rather than defaulting
        // (ADR-0016 §1).
        let count = self.check_expr(scope, args[1], Some(PoolId::S64));
        if pointer == PoolId::ERROR || count == PoolId::ERROR {
            return PoolId::ERROR;
        }

        let Item::PointerType(elem) = *self.pool.item(pointer) else {
            let text = self.describe(pointer);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("`view` needs a pointer, but this is `{text}`"),
                )
                .with_code(E0279)
                .with_note(
                    "the view's element type is the pointer's pointee, so there has to be one",
                )
                .with_help("`view(d, n)` where `d` is a `*T` gives a `[]T`"),
            );
            return PoolId::ERROR;
        };
        // **A `void` pointee has no stride**, so a view over it could not be indexed — refused here rather than
        // producing a `[]void` that every later operation would have to special-case.
        if let Err(error) = jr_pool::layout_of(self.pool, jr_pool::TargetLayout::LP64, elem) {
            let text = self.describe(elem);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("`view` cannot describe elements of type `{text}`"),
                )
                .with_code(E0266)
                .with_note(format!("the layout is unavailable because {error}")),
            );
            return PoolId::ERROR;
        }
        if count != PoolId::S64 {
            let text = self.describe(count);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("`view`'s count must be an `s64`, but this is `{text}`"),
                )
                .with_code(E0279)
                .with_note("a view's count is an `s64`, matching `.count` on an array or a view"),
            );
            return PoolId::ERROR;
        }

        let result = self.pool.view_of(elem);
        self.pointer_views.insert((scope, id), result);
        self.expect(None, result, span)
    }

    /// Types `untyped(p)` — a `*T` viewed as a `*u8` (ADR-0106 §1).
    ///
    /// The reverse of [`Self::check_typed`], and it exists so a caller can **release** what they allocated:
    /// `Basic.free` takes a `*u8`. Symmetric rather than asymmetric on purpose — a facility that can allocate
    /// and not free is one that leaks by construction.
    ///
    /// This direction is the *safe* one — every pointer is a valid `*u8` to read bytes through — but it is
    /// still an intrinsic rather than a relaxation of `cast`, so that both directions are searchable and
    /// neither widens `cast` itself.
    fn check_untyped(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        callee: ExprId,
        args: &[ExprId],
        span: Span,
    ) -> PoolId {
        self.types.set_expr(scope, callee, PoolId::VOID);
        if args.len() != 1 {
            self.wrong_intrinsic_arity("untyped", 1, args.len(), span);
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            return PoolId::ERROR;
        }
        let operand = self.check_expr(scope, args[0], None);
        if operand == PoolId::ERROR {
            return PoolId::ERROR;
        }
        if !matches!(self.pool.item(operand), Item::PointerType(_)) {
            let text = self.describe(operand);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("`untyped` needs a pointer, but this is `{text}`"),
                )
                .with_code(E0279)
                .with_note("it views a pointer's bytes, so there has to be a pointer to view"),
            );
            return PoolId::ERROR;
        }
        let result = self.pool.pointer_to(PoolId::U8);
        self.pointer_views.insert((scope, id), result);
        self.expect(None, result, span)
    }

    /// Types and folds `noted_insert("name", "template")` (ADR-0101 §1).
    ///
    /// The template is emitted **once per noted declaration**, with each `#` replaced by that declaration's
    /// name, and the results concatenated. `#insert` then splices the result through ADR-0073's existing
    /// mechanism, so this adds a *fold* and reuses every other part.
    ///
    /// This is the metaprogram loop for the code-generation case, and it needs no compiler-emitted table:
    /// ADR-0100 §2's limit is that a `for` **variable** cannot be a folded argument, which forbids a loop in
    /// the *program* and says nothing about a loop inside the *fold*. For generation that distinction is not
    /// a workaround — a run-time loop could never declare a procedure or a field, because those are decided
    /// at check time, so generation is inherently a fold.
    fn check_noted_insert(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        callee: ExprId,
        args: &[ExprId],
        span: Span,
    ) -> PoolId {
        self.types.set_expr(scope, callee, PoolId::VOID);
        if args.len() != 2 {
            self.wrong_intrinsic_arity("noted_insert", 2, args.len(), span);
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            return PoolId::ERROR;
        }
        let Some(note) = self.note_name_argument(scope, args[0], "noted_insert", span) else {
            self.check_expr(scope, args[1], None);
            return PoolId::ERROR;
        };

        let template = self.string_literal_of(scope, args[1]);
        self.types.set_expr(scope, args[1], PoolId::STRING);
        let Some(template) = template else {
            self.diags.push(
                Diagnostic::error(span, "`noted_insert`'s template must be a string literal")
                    .with_code(E0277)
                    .with_note(
                        "the text is built while checking, so the template has to be readable then",
                    )
                    .with_help(
                        "write it directly, e.g. `noted_insert(\"serialise\", \"write(#);\")`",
                    ),
            );
            return PoolId::ERROR;
        };

        // `#` stands for the declaration's name: a single character that is **not** valid in a Jairs
        // identifier and is not already an operator, so a template containing one is unambiguous. `$` is
        // taken by polymorphism, `{}` reads as a block, and a word-shaped placeholder could collide with a
        // real name in the generated text.
        let mut text = String::new();
        for name in self.noted_declarations(note) {
            let name = self.interner.resolve(name).to_owned();
            text.push_str(&template.replace('#', &name));
        }
        // **No note matching answers `""`**, which `#insert` accepts as "splice nothing" (ADR-0072 §4) — so a
        // build script's generated section is simply empty in a file with nothing to generate for, rather
        // than a diagnostic about a program that is correct.
        let value = self.pool.str_value(&text);
        self.record_fold(scope, id, span, value);
        self.expect(None, PoolId::STRING, span)
    }

    /// Records a folded call's value under **both** its `(scope, expr)` key and its span (ADR-0101 §3).
    ///
    /// Two keys for one value, because the two consumers see different trees: `file_consts` reads the id in
    /// the tree it checked, and `file_mir` may be looking at an *expanded* tree where a computed `#insert`
    /// has renumbered every id after the splice. The span is the only key that survives that.
    fn record_fold(&mut self, scope: ExprScope, id: ExprId, span: Span, value: PoolId) {
        self.folded_calls.insert((scope, id), value);
        self.folded_call_spans.insert(span, value);
    }

    /// The names of this file's procedures carrying `@name`, in **declaration order** (ADR-0100 §1).
    ///
    /// Declaration order rather than any other, because it is the one order a reader can predict from the
    /// source: sorting by name would make inserting a declaration renumber every index a script had
    /// unrolled, and a hash order would make the same program answer differently between runs.
    /// This file's procedures carrying `@note`, with each note's value, in declaration order.
    ///
    /// Separate from [`Ctx::noted_declarations`] rather than replacing it, because the two have different
    /// consumers: the name-only form answers `noted_count`/`noted_name`, which cannot carry a value, and
    /// changing its return type would make those two build a string they then throw away.
    fn noted_declarations_with_values(&self, note: Symbol) -> Vec<(Symbol, String)> {
        let mut found = Vec::new();
        for item in &self.hir.items {
            let jr_hir::ItemKind::Const {
                value: jr_hir::ConstValue::Proc(proc),
            } = &item.kind
            else {
                continue;
            };
            let Some(name) = item.name else {
                continue;
            };
            // The *first* note with this name, matching what `note_value` answers for the same
            // declaration — a second `@x` on one declaration would otherwise read differently here than
            // there.
            if let Some((_, value)) = self.hir.proc(*proc).notes.iter().find(|(n, _)| *n == note) {
                let text = value.clone().unwrap_or_default();
                found.push((name, text));
            }
        }
        found
    }

    fn noted_declarations(&self, note: Symbol) -> Vec<Symbol> {
        let mut found = Vec::new();
        for item in &self.hir.items {
            let jr_hir::ItemKind::Const {
                value: jr_hir::ConstValue::Proc(proc),
            } = &item.kind
            else {
                continue;
            };
            let Some(name) = item.name else {
                continue;
            };
            if self.hir.proc(*proc).notes.iter().any(|(n, _)| *n == note) {
                found.push(name);
            }
        }
        found
    }

    /// The interned note name `expr` carries, reporting E0277 when it is not a string literal.
    fn note_name_argument(
        &mut self,
        scope: ExprScope,
        expr: ExprId,
        intrinsic: &str,
        span: Span,
    ) -> Option<Symbol> {
        let text = self.string_literal_of(scope, expr);
        self.types.set_expr(scope, expr, PoolId::STRING);
        match text {
            Some(text) => Some(self.interner.intern(&text)),
            None => {
                self.diags.push(
                    Diagnostic::error(
                        span,
                        format!("`{intrinsic}`'s note name must be a string literal"),
                    )
                    .with_code(E0277)
                    .with_note(
                        "the answer is folded while checking, so the name has to be readable then",
                    )
                    .with_help(format!(
                        "write it directly, e.g. `{intrinsic}(\"serialise\")`"
                    )),
                );
                None
            }
        }
    }

    /// The value of `expr` when it is an integer literal, or `None` (ADR-0100 §1).
    fn int_literal_of(&mut self, scope: ExprScope, expr: ExprId) -> Option<i128> {
        match self.expr_of(scope, expr) {
            Expr::Literal(value, _) => match value {
                jr_hir::Literal::Int { value, .. } => Some(value),
                jr_hir::Literal::Str(_)
                | jr_hir::Literal::Float { .. }
                | jr_hir::Literal::Bool(_)
                | jr_hir::Literal::Null => None,
            },
            _ => None,
        }
    }

    /// The notes of the procedure `expr` names, or `None` if it names something else (ADR-0099 §1).
    fn notes_of(
        &mut self,
        scope: ExprScope,
        expr: ExprId,
    ) -> Option<Vec<(Symbol, Option<String>)>> {
        let Expr::Name { res, .. } = self.expr_of(scope, expr) else {
            return None;
        };
        let Res::Item(item) = self.resolve.get(scope, expr).unwrap_or(res) else {
            return None;
        };
        let jr_hir::ItemKind::Const {
            value: jr_hir::ConstValue::Proc(proc),
        } = self.hir.item(item).kind.clone()
        else {
            return None;
        };
        Some(self.hir.proc(proc).notes.clone())
    }

    /// The text of `expr` when it is a string literal, or `None` (ADR-0099 §1).
    fn string_literal_of(&mut self, scope: ExprScope, expr: ExprId) -> Option<String> {
        match self.expr_of(scope, expr) {
            Expr::Literal(value, _) => match value {
                jr_hir::Literal::Str(text) => Some(text),
                jr_hir::Literal::Int { .. }
                | jr_hir::Literal::Float { .. }
                | jr_hir::Literal::Bool(_)
                | jr_hir::Literal::Null => None,
            },
            _ => None,
        }
    }

    /// Types `type_info(T)` and returns `*Type_Info` (ADR-0075 §2).
    ///
    /// The argument is a **type**, so it is marked as a type position before being checked — otherwise
    /// the `Name` arm's E0261 would refuse it for being a type used as a runtime value, which is the
    /// correct refusal in every position but this one.
    fn check_type_info(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        callee: ExprId,
        args: &[ExprId],
        span: Span,
    ) -> PoolId {
        // The callee names no procedure, so it is typed as `void` rather than left unrecorded: MIR
        // reports "an expression was never typed" for a hole, and the callee is never lowered because
        // the call folds to a constant.
        self.types.set_expr(scope, callee, PoolId::VOID);

        if args.len() != 1 {
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "`type_info` takes 1 argument, but {} {} supplied",
                        args.len(),
                        if args.len() == 1 { "was" } else { "were" }
                    ),
                )
                .with_code(E0216),
            );
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            return PoolId::ERROR;
        }

        let arg = args[0];
        // A type is legal *here*, and nowhere new: the allowlist gains one entry rather than E0261
        // gaining an exception (ADR-0071 §3's asymmetry argument).
        self.type_position.insert((scope, arg));

        // **The described type is resolved before the argument is typed as an expression**, because a
        // *builtin* name is not an expression at all: `s64` resolves to no declaration, so
        // `check_expr` yields `ERROR` and bailing on that refused every `type_info(s64)`. Asking what
        // type the name denotes first is what makes a builtin and a declared type take one path.
        let described = self.described_type(scope, arg);

        // Typed anyway, so the argument is recorded in the `TypeMap` — MIR reports "an expression was
        // never typed" for a hole, even one it never lowers. `PoolId::TYPE` is what a name denoting a
        // type has, which is exactly what this is.
        self.types.set_expr(scope, arg, PoolId::TYPE);

        // What it was asked about. A `type`-typed name carries the described type in its
        // `SigEntry::type_value`, which is what `resolve_type_name` reads and what ADR-0071 §1 made a
        // type value out of.
        let Some(described) = described else {
            // **Withheld for an unbound `$T` of the enclosing template** (ADR-0092 §1): `type_info(T)`
            // inside a `$T` procedure is correct code, and it is the *template* that has no binding — each
            // instantiation resolves `T` for real and is checked normally. Reporting E0261 here would be a
            // false error about the very reflection polymorphism exists to enable. The same withholding
            // shape an array length naming a `$N` parameter gets (ADR-0089 §2).
            if let Expr::Name { name, .. } = self.expr_of(scope, arg)
                && self.poly_var_names.contains(&name)
            {
                return PoolId::ERROR;
            }
            self.diags.push(
                Diagnostic::error(span, "`type_info` needs a type")
                    .with_code(E0261)
                    .with_note("its argument is the type to describe, e.g. `type_info(Point)`"),
            );
            return PoolId::ERROR;
        };

        // **A type with no runtime layout has no `size` to report** (E0266, ADR-0075 §4). Refused
        // rather than reported as zero, for `type-errors/063`'s reason: a plausible wrong number cannot
        // be told from a real one downstream.
        if let Err(error) = jr_pool::layout_of(self.pool, jr_pool::TargetLayout::LP64, described) {
            let text = self.describe(described);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("`type_info` cannot describe `{text}`, which has no runtime layout"),
                )
                .with_code(E0266)
                .with_note(format!("the layout is unavailable because {error}"))
                .with_help(
                    "a `Type_Info` reports a size and an alignment, and this type has neither",
                ),
            );
            return PoolId::ERROR;
        }

        // The described type is recorded for lowering, which builds the constant.
        self.type_info_calls.insert((scope, id), described);

        // **By value, not by pointer** (ADR-0075 §2): the folded value is an `Item::AggregateValue`,
        // which is a constant and has no address, so a `*Type_Info` would need a pointee to live
        // somewhere. The MIR verifier caught the pointer version as `deref of a non-pointer`.
        match self.type_info_struct(span) {
            Some(info) => info,
            None => PoolId::ERROR,
        }
    }

    /// Types `any_of(p)` and returns `Any` (ADR-0076 §1).
    ///
    /// The argument must be a **pointer**, and its pointee type is what the resulting `Any` carries. The
    /// erasure to `*u8` happens *here and nowhere else*: a general `cast(*u8, p)` stays refused, because
    /// allowing it would make every pointer type interconvertible and a wrong pointee a silent wrong read
    /// rather than an error.
    ///
    /// Nothing is reinterpreted — a pointer's layout does not depend on its pointee — so this emits no
    /// conversion at all. It is a statement in the type system, which is exactly why it can be safe here
    /// and unsafe in general.
    fn check_any_of(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        callee: ExprId,
        args: &[ExprId],
        span: Span,
    ) -> PoolId {
        self.types.set_expr(scope, callee, PoolId::VOID);

        if args.len() != 1 {
            self.wrong_intrinsic_arity("any_of", 1, args.len(), span);
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            return PoolId::ERROR;
        }

        let arg_ty = self.check_expr(scope, args[0], None);
        if arg_ty == PoolId::ERROR {
            return PoolId::ERROR;
        }

        // The pointee is what the `Any` will say it holds. A non-pointer is refused rather than
        // silently taking the argument's address, because `any_of(x)` and `any_of(*x)` would then mean
        // the same thing and one of them is a lie about lifetime.
        let Item::PointerType(pointee) = *self.pool.item(arg_ty) else {
            let text = self.describe(arg_ty);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("`any_of` needs a pointer, but this is `{text}`"),
                )
                .with_code(E0267)
                .with_note("an `Any` holds a pointer to the value, so the caller decides what it points at")
                .with_help("take the address first, e.g. `any_of(*x)`"),
            );
            return PoolId::ERROR;
        };

        // The pointee needs a `Type_Info`, so it needs a runtime layout — the same refusal
        // `type_info` makes, for the same reason (E0266).
        if let Err(error) = jr_pool::layout_of(self.pool, jr_pool::TargetLayout::LP64, pointee) {
            let text = self.describe(pointee);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("`any_of` cannot erase `{text}`, which has no runtime layout"),
                )
                .with_code(E0266)
                .with_note(format!("the layout is unavailable because {error}")),
            );
            return PoolId::ERROR;
        }

        // Recorded so lowering knows which `Type_Info` to build for the `type` field. A **separate** map
        // from `type_info_calls`, and that separation is load-bearing: that map means "replace this call
        // with a `Type_Info` constant", and folding one here stored a 40-byte `Type_Info` into a 16-byte
        // `Any` — caught immediately as an internal error, but only because the sizes differed.
        self.any_calls.insert((scope, id), (AnyOp::Of, pointee));

        match self.any_struct(span) {
            Some(any) => any,
            None => PoolId::ERROR,
        }
    }

    /// Types `any_as(a, T)` and returns `T` (ADR-0076 §2).
    ///
    /// Two arguments: an `Any` and a **type**. The type argument takes the same treatment
    /// `type_info`'s does — marked a type position so E0261 does not refuse it, and resolved before the
    /// argument is typed as an expression so a builtin works.
    ///
    /// The *check* is at run time and traps on mismatch (ADR-0068's rule for a tagged read, one level
    /// up): sema cannot know which type an `Any` holds, which is the entire reason `Any` exists.
    fn check_any_as(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        callee: ExprId,
        args: &[ExprId],
        span: Span,
    ) -> PoolId {
        self.types.set_expr(scope, callee, PoolId::VOID);

        if args.len() != 2 {
            self.wrong_intrinsic_arity("any_as", 2, args.len(), span);
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            return PoolId::ERROR;
        }

        // The `Any` operand, checked against the declared struct so a mismatch is an ordinary E0214.
        let want = self.any_struct(span);
        let got = self.check_expr(scope, args[0], want);
        if got == PoolId::ERROR || want.is_none() {
            return PoolId::ERROR;
        }

        let type_arg = args[1];
        self.type_position.insert((scope, type_arg));
        let wanted = self.described_type(scope, type_arg);
        self.types.set_expr(scope, type_arg, PoolId::TYPE);

        let Some(wanted) = wanted else {
            self.diags.push(
                Diagnostic::error(span, "`any_as` needs a type as its second argument")
                    .with_code(E0261)
                    .with_note("it is the type to read the `Any` back as, e.g. `any_as(a, Point)`"),
            );
            return PoolId::ERROR;
        };

        if let Err(error) = jr_pool::layout_of(self.pool, jr_pool::TargetLayout::LP64, wanted) {
            let text = self.describe(wanted);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("`any_as` cannot read `{text}`, which has no runtime layout"),
                )
                .with_code(E0266)
                .with_note(format!("the layout is unavailable because {error}")),
            );
            return PoolId::ERROR;
        }

        // Recorded for lowering: it needs the expected type's `Type_Info` to compare against, and the
        // type to give the result.
        self.any_calls.insert((scope, id), (AnyOp::As, wanted));
        wanted
    }

    /// Reports E0216 for an intrinsic called with the wrong number of arguments.
    ///
    /// Shared by the intrinsics so the wording is one sentence in one place: three copies of an arity
    /// message is three chances for them to drift apart.
    fn wrong_intrinsic_arity(&mut self, name: &str, want: usize, got: usize, span: Span) {
        self.diags.push(
            Diagnostic::error(
                span,
                format!(
                    "`{name}` takes {want} argument{}, but {got} {} supplied",
                    if want == 1 { "" } else { "s" },
                    if got == 1 { "was" } else { "were" }
                ),
            )
            .with_code(E0216),
        );
    }

    /// The `Any` struct type, looked up in the imported modules and validated (ADR-0076 §3).
    ///
    /// The second client of ADR-0075 §2's mechanism, which is the first evidence it generalises rather
    /// than being a one-off: same lookup, same E0265, a different field table.
    fn any_struct(&mut self, span: Span) -> Option<PoolId> {
        self.library_struct(span, "Any", ANY_FIELDS)
    }

    /// The type a `type`-valued expression names, if it names one.
    ///
    /// A **builtin** is matched by text, because `s64` is an ordinary identifier that resolves to no
    /// declaration at all (`docs/spec/01-lexical.md` keeps the builtin names out of the lexer), so
    /// `type_info(s64)` would otherwise be an unresolved name. Only a `Res::Error` takes that path: a
    /// name that *did* resolve — to a local, a parameter or a value constant — is not a type, and trying
    /// the builtin table for it would answer the wrong question.
    ///
    /// Matched here rather than by calling `resolve_type_name`, which reports **E0212** as a side effect:
    /// `type_info(x)` for a local `x` then said "unknown type name `x`", which is wrong twice over — `x`
    /// is perfectly well known, and the objection is that it is a value rather than a type. Returning
    /// `None` lets the caller raise E0261, which says exactly that.
    fn described_type(&mut self, scope: ExprScope, arg: ExprId) -> Option<PoolId> {
        // **A parameterised type argument** — `size_of(Slot(s64, s64))` (ADR-0119 §1). In *type* position that is
        // a `TypeRef::Apply`, but an intrinsic's argument is an **expression**, so it parses as a call: the
        // constructor is the callee name and the arguments are ordinary expressions. Recognised here rather than
        // in the parser, because the parser cannot know that this particular call is in a type position — only
        // the intrinsic does, and this is the one function every intrinsic asks.
        //
        // Blocked `Map($K, $V)`'s conversion until now (ADR-0118 §2): its allocation needs
        // `size_of(Slot(K, V))`, and without this the arguments were typed as *values*, so `s64` was an
        // unresolved name.
        if let Expr::Call { callee, args, .. } = self.expr_of(scope, arg) {
            let Expr::Name { name, .. } = self.expr_of(scope, callee) else {
                return None;
            };
            // The callee names no runtime value, so it is typed `void` — the same move every intrinsic makes for
            // its own callee, and for the same reason: MIR reports a hole in the type map, and this is never
            // lowered because the whole expression folds to a type.
            self.types.set_expr(scope, callee, PoolId::VOID);
            // Each argument is itself a **type**, resolved recursively so `Box(Box(s64))` works, and marked a
            // type position so E0261 does not refuse it for being a type used as a runtime value.
            let mut resolved = Vec::with_capacity(args.len());
            for &a in &args {
                self.type_position.insert((scope, a));
                let ty = self.described_type(scope, a)?;
                self.types.set_expr(scope, a, PoolId::TYPE);
                resolved.push(ty);
            }
            let span = self.expr_of(scope, arg).span();
            let instance = self.apply_resolved(name, resolved, span);
            return (instance != PoolId::ERROR).then_some(instance);
        }
        let Expr::Name { name, res, .. } = self.expr_of(scope, arg) else {
            return None;
        };
        // **A bound polymorphic variable wins** (ADR-0092 §1), checked first for the reason
        // `resolve_type_name` checks `type_bindings` first: inside an instantiation, `T` *is* the bound
        // type, and `type_info(T)` must describe it rather than hunting a declaration named `T`. Without
        // this the name resolved to nothing, `builtin_type_named` found no builtin called `T`, and a
        // perfectly reasonable `type_info(T)` inside a `$T` body was E0261 "needs a type" — reflection over
        // the very thing polymorphism binds. Empty outside a polymorphic context, so an ordinary program
        // costs one hash probe.
        if let Some(&bound) = self.type_bindings.get(&name)
            && bound != PoolId::ERROR
        {
            return Some(bound);
        }
        let res = self.resolve.get(scope, arg).unwrap_or(res);
        match res {
            Res::Item(item) => self.entry_for_item(item).and_then(|e| e.type_value),
            Res::Imported(import, name) => self
                .entry_for_import(import, name)
                .and_then(|e| e.type_value),
            // Unresolved: possibly a builtin, which has no declaration to have resolved to.
            Res::Error => self.builtin_type_named(name),
            // Resolved to something that is not a type. `None` here becomes E0261.
            Res::Local(_) | Res::Param(_) | Res::Promoted { .. } => None,
        }
    }

    /// The builtin type a name spells, without reporting anything if it spells none.
    ///
    /// The three lists are the ones `resolve_type_name` consults — `bool`/`string`, `IntKind::NAMES` and
    /// `FloatKind::NAMES` — read here directly so that a miss is silent. `s64` and `u8` keep their
    /// pre-interned ids for the reason `resolve_type_name` gives: the well-known prefix's indices are
    /// pinned by a test and `PTR_U8` depends on them.
    fn builtin_type_named(&mut self, name: Symbol) -> Option<PoolId> {
        let text = self.interner.resolve(name);
        match text {
            "bool" => return Some(PoolId::BOOL),
            "string" => return Some(PoolId::STRING),
            "void" => return Some(PoolId::VOID),
            _ => {}
        }
        if let Some(kind) = jr_pool::FloatKind::from_name(text) {
            return Some(self.pool.intern(Item::FloatType { bits: kind.bits }));
        }
        let kind = jr_pool::IntKind::from_name(text)?;
        Some(match (kind.signed, kind.bits) {
            (true, 64) => PoolId::S64,
            (false, 8) => PoolId::U8,
            (signed, bits) => self.pool.intern(Item::IntType { signed, bits }),
        })
    }

    /// The `Type_Info` struct type, looked up in the imported modules and **validated** (ADR-0075 §2).
    ///
    /// ADR-0075 §2 declares `Type_Info` in `modules/Basic` so that it is *spellable* — no
    /// compiler-declared type is — and the price is this dependency on a declaration the compiler does
    /// not own. The validation is what keeps the price honest: field names, types and order are checked,
    /// so an edit to `Basic` produces E0265 naming the mismatch rather than a read of whatever now sits
    /// at the old offset. A wrong offset would be a silent wrong value, which is the failure mode
    /// ADR-0017 §4 says must refuse instead.
    fn type_info_struct(&mut self, span: Span) -> Option<PoolId> {
        self.library_struct(span, "Type_Info", TYPE_INFO_FIELDS)
    }

    /// Looks a **compiler-known library type** up in `modules/Basic` and validates its shape.
    ///
    /// Shared by `Type_Info` (ADR-0075 §2) and `Any` (ADR-0076 §3), because the mechanism is the same and
    /// the only difference is the field table: one lookup, one E0265, one place the "silent without
    /// imports" rule lives. Two copies would be two chances for the validation to drift.
    fn library_struct(
        &mut self,
        span: Span,
        type_name: &str,
        want_fields: &[(&str, TypeInfoField)],
    ) -> Option<PoolId> {
        // **Silent when no imported signatures were supplied at all**, which is `expect`'s rule about a
        // poisoned context rather than a politeness. `Type_Info` lives in `Basic`, so a checker run
        // *without* module resolution cannot possibly find it — and reporting E0265 there would be
        // inventing a library error out of a missing input. `jr-sema`'s own corpus test runs exactly that
        // way on purpose ("sema must stay silent about them rather than inventing type errors on
        // poison"), and it is what caught this.
        //
        // Nothing is lost: a real program reaches this with `Basic` loaded, and a `type_info` in a file
        // that imports nothing is refused anyway — the call yields `PoolId::ERROR` and MIR never sees a
        // value, so `scan` refuses the body rather than lowering a placeholder.
        if self.imports.is_empty() {
            return None;
        }
        let name = self.interner.intern(type_name);
        let entry = self
            .imports
            .iter()
            .find_map(|(_, sigs)| sigs.lookup(name))
            .or_else(|| self.sigs.lookup(name));
        let Some(ty) = entry.and_then(|e| e.type_value) else {
            self.report_library_shape(span, type_name, "it is not declared, or is not a type");
            return None;
        };
        let Item::StructType { decl, .. } = *self.pool.item(ty) else {
            self.report_library_shape(span, type_name, "it is not a struct");
            return None;
        };
        let Some(fields) = self.pool.struct_fields(decl).map(<[_]>::to_vec) else {
            self.report_library_shape(span, type_name, "its fields are not recorded");
            return None;
        };
        if fields.len() != want_fields.len() {
            self.report_library_shape(
                span,
                type_name,
                &format!(
                    "it has {} field(s), expected {}",
                    fields.len(),
                    want_fields.len()
                ),
            );
            return None;
        }
        for (field, (want_name, want_ty)) in fields.iter().zip(want_fields) {
            let got_name = self.interner.resolve(field.name).to_owned();
            if got_name != *want_name {
                self.report_library_shape(
                    span,
                    type_name,
                    &format!("its field is named `{got_name}`, expected `{want_name}`"),
                );
                return None;
            }
            // `kind` is an enum declared beside it, so its type is checked by *shape* rather than
            // against a fixed id: an enum's `PoolId` depends on its declaration site.
            let ok = match *want_ty {
                TypeInfoField::Enum => matches!(*self.pool.item(field.ty), Item::EnumType { .. }),
                TypeInfoField::PointerToStruct => match *self.pool.item(field.ty) {
                    Item::PointerType(pointee) => {
                        matches!(*self.pool.item(pointee), Item::StructType { .. })
                    }
                    _ => false,
                },
                // A `[]T` over some struct, by shape for the same reason (ADR-0152 §3).
                TypeInfoField::ViewOfStruct => match *self.pool.item(field.ty) {
                    Item::ViewType { elem } => {
                        matches!(*self.pool.item(elem), Item::StructType { .. })
                    }
                    _ => false,
                },
                TypeInfoField::Exact(id) => field.ty == id,
            };
            if !ok {
                let text = self.describe(field.ty);
                self.report_library_shape(
                    span,
                    type_name,
                    &format!(
                        "its field `{want_name}` has type `{text}`, which is not what is expected"
                    ),
                );
                return None;
            }
        }
        Some(ty)
    }

    /// Reports E0265: a compiler-known library type is missing or wrongly shaped (ADR-0075 §2).
    ///
    /// Names the type, because there are two of them now (`Type_Info` and `Any`) and "the standard
    /// library's type is not usable" would leave the reader to guess which.
    fn report_library_shape(&mut self, span: Span, type_name: &str, why: &str) {
        self.diags.push(
            Diagnostic::error(
                span,
                format!("the standard library's `{type_name}` is not usable: {why}"),
            )
            .with_code(E0265)
            .with_note(format!("`{type_name}` is declared in `modules/Basic`"))
            .with_help(format!(
                "import \"Basic\", and keep its `{type_name}` in step with the compiler"
            )),
        );
    }

    /// The callee's per-procedure signature, when the callee names a procedure.
    ///
    /// `Item::ProcType` carries only *types*, so parameter names and defaults have to come from
    /// `ProcSig` — which is keyed by `ProcId` and therefore needs the callee resolved to one
    /// (ADR-0053 §1).
    fn callee_sig(&mut self, scope: ExprScope, callee: ExprId) -> Option<ProcSig> {
        let Expr::Name { res, .. } = self.expr_of(scope, callee) else {
            return None;
        };
        let res = self.resolve.get(scope, callee).unwrap_or(res);
        let item = match res {
            Res::Item(item) => item,
            // A call to an imported procedure resolves through the other file's signatures, which
            // this crate does not hold — so a named argument on a cross-file call is not supported
            // and says so rather than silently ignoring the name.
            Res::Imported(_, _)
            | Res::Local(_)
            | Res::Param(_)
            | Res::Promoted { .. }
            | Res::Error => return None,
        };
        let jr_hir::ItemKind::Const {
            value: jr_hir::ConstValue::Proc(proc),
        } = self.hir.item(item).kind.clone()
        else {
            return None;
        };
        self.sigs.proc_sig(proc).cloned()
    }

    /// The name of an **imported polymorphic** procedure this callee names, or `None` (ADR-0104 §2).
    ///
    /// The counterpart of [`Self::callee_poly`] across a module boundary. It exists because that function's
    /// documented assumption was wrong: an imported template does *not* report an honest mismatch on the
    /// ordinary path, because a `$T` parameter's type is `PoolId::ERROR` and `ERROR` matches anything — so
    /// the call type-checked and the missing instantiation surfaced as an internal error in whichever engine
    /// ran first.
    fn imported_template_callee(&mut self, scope: ExprScope, callee: ExprId) -> Option<Symbol> {
        let Expr::Name { res, name, .. } = self.expr_of(scope, callee) else {
            return None;
        };
        let Res::Imported(_, imported) = self.resolve.get(scope, callee).unwrap_or(res) else {
            return None;
        };
        // Asked of the imported *signatures*, which is what this crate has of another file — the same
        // evidence `callee_is_imported_macro` uses, and recorded by the same pass.
        self.imports
            .iter()
            .any(|(_, sigs)| sigs.is_template_name(imported))
            .then_some(name)
    }

    /// The callee's `(ProcId, ProcSig)` when it names a **local polymorphic** procedure (ADR-0082 §1).
    ///
    /// `None` for an ordinary procedure (no `$T`), and for an *imported* polymorphic one: cross-file
    /// instantiation is deferred (ADR-0082 §5).
    ///
    /// **This used to claim the imported case then "reports an honest mismatch" on the ordinary path, and it
    /// did not** (ADR-0104 §2). A `$T` parameter's type is `PoolId::ERROR`, and `ERROR` matches anything — so
    /// the call type-checked and the missing instantiation leaked out of whichever engine ran first as "no
    /// routine for file N proc M". [`Self::imported_template_callee`] refuses it with E0268 before the
    /// ordinary path is reached, which is what makes the deferral a diagnostic instead of an ICE.
    fn callee_poly(&mut self, scope: ExprScope, callee: ExprId) -> Option<(ProcId, ProcSig)> {
        let Expr::Name { res, .. } = self.expr_of(scope, callee) else {
            return None;
        };
        let Res::Item(item) = self.resolve.get(scope, callee).unwrap_or(res) else {
            return None;
        };
        let jr_hir::ItemKind::Const {
            value: jr_hir::ConstValue::Proc(proc),
        } = self.hir.item(item).kind.clone()
        else {
            return None;
        };
        let sig = self.sigs.proc_sig(proc).cloned()?;
        // A `$T` template — with or without `$N`/`$$T` comptime parameters (ADR-0137). Pure `$T`
        // falls into `check_polymorphic_call`; a template that also has comptime params is
        // routed there too, and it records the comptime arguments alongside the type
        // bindings so the instantiation carries both. This is the mixed case ADR-0088
        // deferred and PLAN §7 named as "wave 7 — `$$T`".
        (!sig.poly_vars.is_empty()).then_some((proc, sig))
    }

    /// The `(proc, sig)` of a **local** procedure with a `$N` comptime-value parameter that `callee`
    /// names, or `None` (ADR-0088 §1).
    ///
    /// Shaped like [`Self::callee_poly`], and separate from it because the two templates key on different
    /// things: a `$T` instantiation keys on the argument's *type* (known here), a `$N` one on the
    /// argument's *value* (known only after const-eval, downstream). An imported callee falls through —
    /// cross-file instantiation is deferred (ADR-0082 §5) — and is refused by
    /// [`Self::imported_template_callee`], since the "honest mismatch" this comment used to promise does not
    /// happen (ADR-0104 §2).
    fn callee_comptime_template(
        &mut self,
        scope: ExprScope,
        callee: ExprId,
    ) -> Option<(ProcId, ProcSig)> {
        let Res::Item(item) = self.resolve.get(scope, callee)? else {
            return None;
        };
        let jr_hir::ItemKind::Const {
            value: jr_hir::ConstValue::Proc(proc),
        } = self.hir.item(item).kind.clone()
        else {
            return None;
        };
        let sig = self.sigs.proc_sig(proc).cloned()?;
        // A `$N` template with **no** `$T` variables. A mixed `$T`+`$N` template is out of scope
        // (see `callee_poly`) and falls through to the ordinary path.
        let has_comptime = sig.comptime_params.iter().any(|&c| c);
        (has_comptime && sig.poly_vars.is_empty()).then_some((proc, sig))
    }

    /// Whether `callee` names a **local** `#expand` macro (ADR-0090 §3).
    ///
    /// Shaped like [`Self::callee_comptime_template`]. An imported macro falls through — a cross-file
    /// splice is deferred with the splice itself — and its ordinary signature makes the call an ordinary
    /// call, which is wrong but *reported* by the same refusal once the splice exists cross-file.
    fn callee_is_imported_macro(&mut self, scope: ExprScope, callee: ExprId) -> bool {
        // A **same-file** macro is spliced by lowering, so its name never resolves to a callable item by
        // the time a call reaches here. An *imported* one resolves through `Res::Imported`, and this crate
        // cannot see the other file's `Proc::expand` — so the signature is the evidence: an imported
        // procedure whose name the importing file cannot splice.
        let Some(res) = self.resolve.get(scope, callee) else {
            return false;
        };
        let jr_hir::Res::Imported(_, name) = res else {
            // A same-file `#expand` that somehow still resolved to an item — belt and braces, since
            // lowering should have spliced it.
            let jr_hir::Res::Item(item) = res else {
                return false;
            };
            let jr_hir::ItemKind::Const {
                value: jr_hir::ConstValue::Proc(proc),
            } = self.hir.item(item).kind.clone()
            else {
                return false;
            };
            return self.hir.procs.get(proc.index()).is_some_and(|p| p.expand);
        };
        // An imported name whose module declares it `#expand`. Asked of the imported *signatures*, which
        // is what this crate has of another file.
        self.imports.iter().any(|(_, sigs)| sigs.is_macro(name))
    }

    /// Types a call to a comptime-value-parameterised procedure and records it for instantiation
    /// (ADR-0088 §1).
    ///
    /// Checks arity, types every argument (each against its parameter's known type — a `$N`'s type is
    /// ordinary, so a comptime argument is checked exactly as a runtime one; only *when* its value is
    /// known differs), and records the comptime **argument expressions** in parameter order for the
    /// `jr-db` pre-pass to evaluate. The return type is the template's, concrete already.
    fn check_comptime_call(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        callee: ExprId,
        proc: ProcId,
        sig: &ProcSig,
        args: &[ExprId],
        span: Span,
    ) -> PoolId {
        // The callee names the template; type it `void` and never lower it, exactly as a `$T` call does —
        // the call is redirected to the instantiation, so MIR never sees the template callee.
        self.types.set_expr(scope, callee, PoolId::VOID);

        if args.len() != sig.params.len() {
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "this procedure takes {} argument{}, but {} {} supplied",
                        sig.params.len(),
                        if sig.params.len() == 1 { "" } else { "s" },
                        args.len(),
                        if args.len() == 1 { "was" } else { "were" }
                    ),
                )
                .with_code(E0216),
            );
            return PoolId::ERROR;
        }

        let mut comptime_args: Vec<ExprId> = Vec::new();
        for (index, &comptime) in sig.comptime_params.iter().enumerate() {
            let Some(&arg) = args.get(index) else {
                continue;
            };
            let want = sig
                .params
                .get(index)
                .copied()
                .filter(|&t| t != PoolId::ERROR);
            self.check_expr(scope, arg, want);
            if comptime {
                comptime_args.push(arg);
            }
        }

        self.comptime_calls
            .insert((scope, id), (proc, comptime_args));
        sig.ret
    }

    /// Types a call to a local polymorphic procedure, recording the instantiation (ADR-0082 §1).
    ///
    /// Infers each `$T` from the corresponding argument's type, binds it, re-resolves the signature to
    /// concrete parameter and return types, checks the arguments against those, and records
    /// `(proc, bound types)` for the expansion pass. The return type is the concrete one, so the call's
    /// value is usable exactly as an ordinary call's.
    ///
    /// Refuses — with E0268 — the cases this sub-wave does not instantiate (ADR-0082 §5): more than one
    /// distinct `$T`, or a `$T` that no argument position pins. A refusal here is by design and named, not
    /// a silent gap.
    fn check_polymorphic_call(
        &mut self,
        scope: ExprScope,
        id: ExprId,
        callee: ExprId,
        proc: ProcId,
        sig: &ProcSig,
        args: &[ExprId],
        span: Span,
    ) -> PoolId {
        // The callee names the template, whose type is a `ProcType` with `ERROR` parameters — recording
        // that would fail `scan`'s error-type check. It is typed `void` and never lowered (the call is
        // redirected to the instantiation, ADR-0082), the same trick `check_type_info` uses for its
        // non-procedure callee: a recorded, harmless type so MIR does not see an untyped hole.
        self.types.set_expr(scope, callee, PoolId::VOID);

        // Arity: the template's parameter count is fixed even though the types are not.
        if args.len() != sig.params.len() {
            for arg in args {
                self.check_expr(scope, *arg, None);
            }
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "this procedure takes {} argument{}, but {} {} supplied",
                        sig.params.len(),
                        if sig.params.len() == 1 { "" } else { "s" },
                        args.len(),
                        if args.len() == 1 { "was" } else { "were" }
                    ),
                )
                .with_code(E0216),
            );
            return PoolId::ERROR;
        }

        // Infer **each** variable from the first parameter whose declared type *is* `$Var` directly (a bare
        // poly variable), typing every argument on the way (ADR-0083 §3). A nested `*$T`/`[]$T` position is
        // not an inference site — that needs a unifier this wave still does not build (ADR-0083 §4).
        let hir_params = self.hir.proc(proc).params.clone();
        let mut bindings: Vec<(Symbol, PoolId)> = Vec::new();
        for (index, param) in hir_params.iter().enumerate() {
            let Some(arg) = args.get(index) else { continue };
            let arg_ty = self.check_expr(scope, *arg, None);
            if arg_ty == PoolId::ERROR {
                continue;
            }
            // Match the parameter's `TypeRef` structure against the argument's resolved type, binding a
            // `$T` wherever a `TypeRef::Poly` meets a concrete type — directly (`$T` ↔ `U`) or one layer
            // deep (`*$T` ↔ `*U`, `[]$T` ↔ `[]U`), ADR-0084 §1. First binding for a variable wins; a later
            // occurrence is a *use*, checked against it below. A shape mismatch binds nothing (§2).
            if let Some(t) = param.ty {
                self.infer_var_in(t, arg_ty, &mut bindings);
            }
        }

        // Every variable the signature introduces must have been pinned by a direct argument. One that was
        // not — because it appears only nested, or an argument did not type — is refused by design
        // (ADR-0083 §3); the missing-type case already reported the argument's own error.
        if sig
            .poly_vars
            .iter()
            .any(|v| !bindings.iter().any(|(b, _)| b == v))
        {
            self.diags.push(
                Diagnostic::error(span, "cannot infer every `$T` from the arguments of this call")
                    .with_code(E0268)
                    .with_note(
                        "each type variable is inferred from an argument that pins it — directly, or through a pointer or view (ADR-0084)",
                    ),
            );
            return PoolId::ERROR;
        }

        // Bind every variable and re-resolve the signature against them, so a bare `A`/`B` parameter is
        // checked against its inferred type and the return type is concrete.
        for (var, ty) in &bindings {
            self.type_bindings.insert(*var, *ty);
        }
        for (index, param) in hir_params.iter().enumerate() {
            if let (Some(arg), Some(t)) = (args.get(index), param.ty) {
                let want = self.resolve_type(ExprScope::TopLevel, t, span);
                if want != PoolId::ERROR {
                    self.check_expr(scope, *arg, Some(want));
                }
            }
        }
        let ret = self.hir.proc(proc).ret.map_or(PoolId::VOID, |t| {
            self.resolve_type(ExprScope::TopLevel, t, span)
        });
        for (var, _) in &bindings {
            self.type_bindings.remove(var);
        }

        // Record the instantiation for the expansion pass, keyed by the call. The bound types are ordered
        // by the variables' first appearance in the signature (`poly_vars`), so the structural key is
        // deterministic (ADR-0083 §1).
        let key: Vec<PoolId> = sig
            .poly_vars
            .iter()
            .map(|v| {
                bindings
                    .iter()
                    .find(|(b, _)| b == v)
                    .map_or(PoolId::ERROR, |(_, t)| *t)
            })
            .collect();
        self.instantiations.insert((scope, id), (proc, key));

        // For a **mixed** `$$T` template (ADR-0137), also record the comptime arguments so the
        // pre-pass evaluates them and the instantiation clone bakes their values. A pure `$T`
        // template has no comptime params and this list is empty; a mixed one records the
        // argument at each `$N`/`$$T` position.
        if sig.comptime_params.iter().any(|&c| c) {
            let comptime_args: Vec<ExprId> = sig
                .comptime_params
                .iter()
                .enumerate()
                .filter_map(|(index, &is_comptime)| {
                    if is_comptime {
                        args.get(index).copied()
                    } else {
                        None
                    }
                })
                .collect();
            self.comptime_calls
                .insert((scope, id), (proc, comptime_args));
        }
        ret
    }

    /// Binds a type variable by matching a parameter's `TypeRef` structure against an argument's resolved
    /// type, one structural layer deep (ADR-0084 §1).
    ///
    /// `$T` against `U` binds `T = U`; `*$T` against `*U` peels both pointers and binds `T = U`; `[]$T`
    /// against `[]U` peels both views. A shape that does not align — `*$T` against a non-pointer — binds
    /// nothing (ADR-0084 §2), leaving the variable for another position to pin or the argument check to
    /// reject. The first binding for a variable wins; a later occurrence is a *use*, checked against it.
    ///
    /// One-directional and not a unifier (ADR-0084 §3): it reads a binding *out of* the argument type,
    /// with no substitution back and no occurs-check.
    fn infer_var_in(
        &self,
        param_ty: TypeRefId,
        arg_ty: PoolId,
        bindings: &mut Vec<(Symbol, PoolId)>,
    ) {
        match self.type_ref(ExprScope::TopLevel, param_ty) {
            TypeRef::Poly(var) => {
                if !bindings.iter().any(|(v, _)| *v == var) {
                    bindings.push((var, arg_ty));
                }
            }
            TypeRef::Pointer(inner) => {
                if let Item::PointerType(pointee) = *self.pool.item(arg_ty) {
                    self.infer_var_in(inner, pointee, bindings);
                }
            }
            TypeRef::View { elem } => {
                if let Item::ViewType { elem: arg_elem } = *self.pool.item(arg_ty) {
                    self.infer_var_in(elem, arg_elem, bindings);
                }
            }
            // A `#simd [4]$T` parameter binds `$T` from a vector argument, and **only** from one: the
            // pattern must match the item, so a `[4]s32` array argument does not bind against a
            // vector parameter. That is the identity distinction doing its job at the one place a
            // mismatch would otherwise be silent (ADR-0148 §1).
            TypeRef::Vector { elem, .. } => {
                if let Item::VectorType { elem: arg_elem, .. } = *self.pool.item(arg_ty) {
                    self.infer_var_in(elem, arg_elem, bindings);
                }
            }
            TypeRef::DynamicArray { elem } => {
                if let Item::DynamicArrayType { elem: arg_elem } = *self.pool.item(arg_ty) {
                    self.infer_var_in(elem, arg_elem, bindings);
                }
            }
            // Any other shape (a name, an array — whose length is part of its identity and not matched
            // here — a struct, a proc type) contains no directly-bindable variable in this sub-wave's
            // model, so it contributes no binding. A later sub-wave that wants `[$N]$T` inference adds
            // arms here.
            // Inferring `$T` through a parameterised struct — `(b: Box($T))` binding `T` from a
            // `Box(s64)` argument — is nested inference through a nominal type, deferred with the rest
            // of that step (ADR-0085 §5). So `Apply` binds nothing here this sub-wave.
            TypeRef::Name(_)
            | TypeRef::Array { .. }
            | TypeRef::Results(_)
            | TypeRef::Proc { .. }
            | TypeRef::Apply { .. }
            | TypeRef::Struct(_)
            | TypeRef::Union(_)
            | TypeRef::Variant(_)
            | TypeRef::Enum(_)
            | TypeRef::Error => {}
        }
    }

    /// Resolves an argument list into one slot per parameter (ADR-0053 §1, §3).
    ///
    /// Returns `None` having reported a diagnostic when any of §3's four rules is broken. The four
    /// are checked in source order so the *first* mistake is the one reported, rather than a cascade.
    fn fill_arguments(
        &mut self,
        scope: ExprScope,
        callee: ExprId,
        args: &[ExprId],
        arg_names: &[Option<Symbol>],
        span: Span,
    ) -> Option<Vec<ArgSlot>> {
        let sig = self.callee_sig(scope, callee)?;
        let mut slots: Vec<Option<ArgSlot>> = vec![None; sig.params.len()];
        let mut seen_named = false;

        for (index, arg) in args.iter().enumerate() {
            match arg_names.get(index).copied().flatten() {
                Some(name) => {
                    seen_named = true;
                    let Some(position) = sig.names.iter().position(|n| *n == name) else {
                        let text = self.interner.resolve(name);
                        let candidates: Vec<&str> = sig
                            .names
                            .iter()
                            .map(|n| self.interner.resolve(*n))
                            .collect();
                        let mut diag = Diagnostic::error(
                            span,
                            format!("this procedure has no parameter named `{text}`"),
                        )
                        .with_code(E0252);
                        // The same near-name machinery E0212 and E0218 use (ADR-0031 §1) — a
                        // misspelled parameter is exactly the case it exists for.
                        if let Some(suggestion) =
                            crate::suggest::nearest(text, candidates.iter().copied())
                        {
                            diag = diag.with_help(format!("did you mean `{suggestion}`?"));
                        }
                        self.diags.push(diag);
                        return None;
                    };
                    if slots[position].is_some() {
                        let text = self.interner.resolve(name);
                        self.diags.push(
                            Diagnostic::error(span, format!("`{text}` is supplied more than once"))
                                .with_code(E0252)
                                .with_note(
                                    "a parameter already filled positionally cannot be named",
                                ),
                        );
                        return None;
                    }
                    slots[position] = Some(ArgSlot::Given(*arg));
                }
                None => {
                    if seen_named {
                        self.diags.push(
                            Diagnostic::error(
                                span,
                                "a positional argument cannot follow a named one",
                            )
                            .with_code(E0252)
                            .with_note(
                                "otherwise a positional argument's meaning would depend on which names came before it",
                            ),
                        );
                        return None;
                    }
                    if index >= slots.len() {
                        self.diags.push(
                            Diagnostic::error(
                                span,
                                format!(
                                    "this procedure takes {} argument{}, but more were supplied",
                                    sig.params.len(),
                                    if sig.params.len() == 1 { "" } else { "s" }
                                ),
                            )
                            .with_code(E0252),
                        );
                        return None;
                    }
                    slots[index] = Some(ArgSlot::Given(*arg));
                }
            }
        }

        // Anything still unfilled must have a default, or the call is incomplete.
        let mut filled = Vec::with_capacity(slots.len());
        for (position, slot) in slots.into_iter().enumerate() {
            match slot {
                Some(slot) => filled.push(slot),
                None => match sig.defaults.get(position).copied().flatten() {
                    Some(value) => filled.push(ArgSlot::Default(value)),
                    None => {
                        let text = self.interner.resolve(sig.names[position]);
                        self.diags.push(
                            Diagnostic::error(
                                span,
                                format!("`{text}` has no argument and no default value"),
                            )
                            .with_code(E0252),
                        );
                        return None;
                    }
                },
            }
        }
        Some(filled)
    }

    /// Types a field access, looking through pointers.
    /// Types `e[i].x` where `e` is an `#soa` struct, or answers `None` if this is not one.
    ///
    /// The result is the field's **element** type, and the access is recorded in
    /// [`CheckOutput::soa_fields`] so that `jr-mir` builds `Field(x)` then `Index(i)` — the same
    /// place it would build for `e.x[i]`. Sema decides and MIR reads, exactly as `operator_calls`,
    /// `any_calls` and `variadic_calls` already work: two crates recognising this pattern
    /// independently would be two chances to disagree, and a disagreement here is a *wrong
    /// address* — sema typing an element while MIR reads a whole array.
    fn check_soa_field(
        &mut self,
        scope: ExprScope,
        receiver: ExprId,
        name: Symbol,
        name_span: Span,
    ) -> Option<PoolId> {
        // **Through `expr_of`, not `hir.exprs`.** Expressions live in per-*body* arenas selected by
        // `scope` (that is what `ExprScope` is for), so indexing the top-level arena would read a
        // different node — which is exactly what the first attempt did: it silently answered `None`
        // for every access inside a procedure, so the sugar appeared not to work at all.
        let Expr::Index { base, index, .. } = self.expr_of(scope, receiver) else {
            return None;
        };
        // The base's type, with pointers stripped as an ordinary field receiver's is: `p[i].x`
        // through a `*Entities` reads the same way `p.x` does.
        let mut base_ty = self.check_expr(scope, base, None);
        while let Some(inner) = self.pointee(base_ty) {
            base_ty = inner;
        }
        let Item::StructType { decl, .. } = *self.pool.item(base_ty) else {
            return None;
        };
        self.pool.soa_count(decl)?;

        // The index is an ordinary `s64`, checked and recorded so MIR can lower it — and so a
        // non-integer index is the same diagnostic it would be on an array.
        let _ = self.check_expr(scope, index, Some(PoolId::S64));
        // **The index expression is recorded with the *receiver's* type.** `e[i]` has no type of
        // its own by design (§2 refuses every other use of it), but it must have *some* recorded
        // type: `jr-mir`'s `scan` refuses a body containing a reachable expression typed `ERROR`,
        // so recording poison here refused every program that used the sugar — which is what the
        // first attempt did. The struct's own type is the honest placeholder: an `#soa` index is a
        // projection *step* rather than a value, and nothing reads this entry, because lowering
        // handles the whole `e[i].x` in one place.
        self.types.set_expr(scope, receiver, base_ty);

        let interner = self.interner;
        let field = interner.resolve(name);
        let fields = self.pool.fields_of(base_ty)?.to_vec();
        let Some(position) = fields.iter().position(|f| f.name == name) else {
            self.diags.push(
                Diagnostic::error(
                    name_span,
                    format!("no field `{field}` on this `#soa` struct"),
                )
                .with_code(E0284),
            );
            return Some(PoolId::ERROR);
        };
        // **Keyed on the receiver — the index expression — rather than on the field access.**
        // `check_field` does not receive the field expression's own id, and the receiver is exactly
        // as unambiguous: an `Expr::Index` is the receiver of at most one field access, and MIR has
        // it in hand when it lowers that access.
        self.soa_fields
            .insert((scope, receiver), u32::try_from(position).unwrap_or(0));
        // The field's type is `[N]T`; the access yields `T`. Reading the element type from the
        // *field* rather than recomputing it is what keeps this in step with the wrapping done at
        // resolution (ADR-0147 §1) — one place decides what `#soa` did to a field.
        match *self.pool.item(fields[position].ty) {
            Item::ArrayType { elem, .. } => Some(elem),
            // A field of a `#soa` struct is an array by construction, so this is a compiler
            // disagreement rather than a program error; typing it as `ERROR` keeps the cascade
            // quiet, and the wrapping site is the thing to fix.
            _ => Some(PoolId::ERROR),
        }
    }

    fn check_field(
        &mut self,
        scope: ExprScope,
        receiver: ExprId,
        name: Symbol,
        name_span: Span,
    ) -> PoolId {
        // **`e[i].x` on an `#soa` struct means `e.x[i]`** (ADR-0147 §2), and it is decided here
        // rather than in the `Index` arm because the index expression alone is not enough: `e[i]`
        // has no type of its own by design, so there is nothing to record against it. Both the
        // index and the field name are in reach only at the field access.
        if let Some(ty) = self.check_soa_field(scope, receiver, name, name_span) {
            return ty;
        }
        // The receiver is a position where a **type** is legal: `Colour.RED` names the enum type used
        // as a value (ADR-0041 §1). Recorded before typing it, the way `check_call` records its callee,
        // so that `check_expr`'s `Name` arm skips E0261 here while still typing and recording the
        // receiver exactly as any other expression (ADR-0071 §3).
        self.type_position.insert((scope, receiver));
        let mut ty = self.check_expr(scope, receiver, None);
        while let Some(inner) = self.pointee(ty) {
            ty = inner;
        }
        if ty == PoolId::ERROR {
            return PoolId::ERROR;
        }

        let interner = self.interner;
        let field = interner.resolve(name);

        let receiver_kind = match self.pool.item(ty) {
            Item::StringType => ReceiverKind::Str,
            // A union's field access *is* a struct's: same field list, same side table, same
            // diagnostics. Only the offsets differ, and those are `jr-pool`'s (ADR-0045 §5). A
            // variant's cases are a field list too (ADR-0068 §1), so it joins them — what differs is
            // the tag check MIR emits on the *read*, which is not a typing question.
            Item::StructType { .. } | Item::UnionType { .. } | Item::VariantType { .. } => {
                ReceiverKind::Struct(ty)
            }
            // The context's fields are the compiler's, not a side table's — there is no `DeclId` to
            // key one on (ADR-0057 §1), so this is its own receiver kind rather than a `Struct`.
            Item::ContextType => ReceiverKind::Context,
            Item::ArrayType { .. } => ReceiverKind::Array,
            // **A vector answers `.count` as an array does** — a constant from the type, nothing
            // loaded (ADR-0148 §1) — so it shares `ReceiverKind::Array` rather than getting a
            // variant. That variant's meaning is precisely "the count is in the type", and it is the
            // distinction MIR needs; a vector's is, so there is nothing here to tell apart. Found by
            // *writing* the corpus file, which asserted `.count` worked before it did.
            Item::VectorType { .. } => ReceiverKind::Array,
            Item::ViewType { .. } => ReceiverKind::View,
            Item::DynamicArrayType { elem } => {
                // The pointer type is `*elem`, which needs its own interning — the pool has
                // one canonical `*s64` and looking it up here is what makes `xs.data` typed
                // as that canonical pointer rather than something new every call.
                let ptr_ty = self.pool.pointer_to(*elem);
                ReceiverKind::DynamicArray(ptr_ty)
            }
            // `Colour.RED`: the *receiver* is the enum type used as a value, so its type is
            // `type` and the enum it denotes has to come from the receiver expression rather
            // than from `ty` (ADR-0041 §1).
            Item::TypeType => match self.denoted_enum(scope, receiver) {
                Some((decl, flags)) => ReceiverKind::Enum(decl, flags),
                None => ReceiverKind::Fieldless,
            },
            _ => ReceiverKind::Fieldless,
        };

        match receiver_kind {
            // ADR-0004 fixes `string`'s layout as `{data: *u8, count: s64}` and
            // makes both directly accessible. They are pseudo-fields rather than
            // real ones because `string` is deliberately *not* the struct of that
            // shape (ADR-0015 §2).
            ReceiverKind::Str => match field {
                "data" => PoolId::PTR_U8,
                "count" => PoolId::S64,
                _ => {
                    self.no_such_field(ty, field, name_span);
                    PoolId::ERROR
                }
            },
            // `[N]T` has exactly one pseudo-field, `.count`, and it is the length from the
            // *type* — nothing is loaded (ADR-0039 §5). There is deliberately no `.data`:
            // it would hand out an unbounded `*T` one wave after adding the bounds check.
            ReceiverKind::Array => match field {
                "count" => PoolId::S64,
                _ => {
                    self.no_such_field(ty, field, name_span);
                    PoolId::ERROR
                }
            },
            // A view answers `.count` with the same type an array does, and by a different
            // route: this one is a *load* of the second word rather than a constant from the
            // type (ADR-0044 §4). `.data` is absent for the array's reason — it would hand
            // out an unbounded `*T` with no pointer arithmetic to use it with.
            ReceiverKind::View => match field {
                "count" => PoolId::S64,
                _ => {
                    self.no_such_field(ty, field, name_span);
                    PoolId::ERROR
                }
            },
            // `[..]T` exposes three pseudo-fields — `.data: *T`, `.count: s64`,
            // `.capacity: s64` (ADR-0136 §1). Unlike a view, `.data` is exposed because a
            // dynamic array *owns* its data; a caller who wants to inspect or free it must
            // reach the pointer.
            ReceiverKind::DynamicArray(ptr_ty) => match field {
                "data" => ptr_ty,
                "count" => PoolId::S64,
                "capacity" => PoolId::S64,
                _ => {
                    self.no_such_field(ty, field, name_span);
                    PoolId::ERROR
                }
            },
            // A member is a *value of the enum type*, not of the backing integer: that is
            // what makes `Colour` and `s64` different types (ADR-0041 §3). The member's
            // number is folded in at MIR; here only its existence and its type matter.
            ReceiverKind::Enum(decl, flags) => {
                let known = self
                    .pool
                    .enum_members(decl)
                    .is_some_and(|members| members.iter().any(|m| m.name == name));
                if known {
                    self.pool.enum_type(decl, flags)
                } else {
                    self.no_such_member(decl, flags, field, name_span);
                    PoolId::ERROR
                }
            }
            ReceiverKind::Struct(instance) => {
                // A direct field first, then — failing that — a field of any `using`-embedded
                // base (ADR-0050 §4). Direct wins, so a struct that declares `x` *and* embeds
                // something declaring `x` means its own, which matches the rule everywhere else
                // in the language: the nearer declaration shadows.
                //
                // Direct fields come from `fields_of` on the *instance*, so `Box(s64).value` is
                // `s64` (ADR-0085 §2); `using`-promotion stays keyed on the `DeclId`, since a
                // parameterised struct with a `using` field is out of this sub-wave's scope
                // (ADR-0085 §5) and an ordinary struct's instance carries its own `DeclId`.
                let embed_decl = match self.pool.item(instance) {
                    Item::StructType { decl, .. }
                    | Item::UnionType { decl, .. }
                    | Item::VariantType { decl, .. } => *decl,
                    _ => unreachable!("ReceiverKind::Struct holds a struct/union/variant instance"),
                };
                let found = self
                    .pool
                    .fields_of(instance)
                    .and_then(|fields| fields.iter().find(|f| f.name == name).map(|f| f.ty))
                    .or_else(|| self.embedded_field_type(embed_decl, name));
                match found {
                    Some(field_ty) => field_ty,
                    None => {
                        self.no_such_field(ty, field, name_span);
                        PoolId::ERROR
                    }
                }
            }
            ReceiverKind::Context => match jr_pool::Pool::context_field(field) {
                Some(index) => jr_pool::Pool::context_field_type(index).unwrap_or(PoolId::ERROR),
                None => {
                    let candidates = jr_pool::CONTEXT_FIELD_NAMES.iter().copied();
                    let mut diag =
                        Diagnostic::error(name_span, format!("the context has no field `{field}`"))
                            .with_code(E0218);
                    // The same near-name machinery every other field lookup uses (ADR-0031 §1).
                    if let Some(suggestion) = crate::suggest::nearest(field, candidates) {
                        diag = diag.with_help(format!("did you mean `{suggestion}`?"));
                    }
                    self.diags.push(diag);
                    PoolId::ERROR
                }
            },
            ReceiverKind::Fieldless => {
                let text = self.describe(ty);
                self.diags.push(
                    Diagnostic::error(name_span, format!("type `{text}` has no fields"))
                        .with_code(E0218),
                );
                PoolId::ERROR
            }
        }
    }

    /// Types `base[index]` (ADR-0039 §5).
    ///
    /// Three separate refusals, each pointing at the thing that is wrong: the base if it
    /// is not an array, the index if it is not an integer, and the index again if it is a
    /// literal that cannot be in range. Reporting all three against the whole `a[i]` span
    /// would make the reader look at the wrong end of the expression.
    fn check_index(
        &mut self,
        scope: ExprScope,
        base: ExprId,
        index: ExprId,
        index_span: Span,
        span: Span,
    ) -> PoolId {
        let mut base_ty = self.check_expr(scope, base, None);
        // Auto-deref, exactly as field access does: `p: *[4]u8` indexes through the
        // pointer. Same loop, so the two cannot disagree about how many levels.
        while let Some(inner) = self.pointee(base_ty) {
            base_ty = inner;
        }

        // The index is checked whatever the base turned out to be, so that `notarray[bad]`
        // reports both problems rather than hiding the second behind the first.
        //
        // `Some(PoolId::S64)` is the context an untyped literal takes (ADR-0016 §1), which
        // makes `buf[0]` an `s64` index rather than an unconstrained one.
        let index_ty = self.check_expr(scope, index, Some(PoolId::S64));

        // A view indexes like an array and has no compile-time length, so `len` is `None`
        // and the literal-index check below is skipped. That is not a weaker check: a view's
        // length is unknown at compile time by definition, and `Statement::BoundsCheck` still
        // guards every access at run time (ADR-0044 §4).
        let Some((elem, len)) = self.indexable_parts(base_ty) else {
            // **An `#soa` struct indexed anywhere but as a field receiver** (ADR-0147 §2). It
            // reaches here because `check_soa_field` is the only path that accepts one, so
            // everything else lands in the general "not indexable" arm — where E0234's "only
            // arrays and views can be indexed" would be true and unhelpful to someone who just
            // wrote `#soa` and expected exactly this to work.
            if let Item::StructType { decl, .. } = *self.pool.item(base_ty)
                && self.pool.soa_count(decl).is_some()
            {
                self.diags.push(
                    Diagnostic::error(
                        span,
                        "an `#soa` struct can only be indexed as the receiver of a field access",
                    )
                    .with_code(E0284)
                    .with_note(
                        "`e[i]` on its own has no type: the fields live in separate arrays, so \
                         there is no single element to name",
                    )
                    .with_help("write `e[i].field`, or `e.field[i]`, which mean the same thing"),
                );
                return PoolId::ERROR;
            }
            // Poison propagates silently: the base's own error was already reported.
            if base_ty != PoolId::ERROR {
                let text = self.describe(base_ty);
                self.diags.push(
                    Diagnostic::error(span, format!("cannot index a value of type `{text}`"))
                        .with_code(E0234)
                        .with_note("only a fixed-size array `[N]T` and a view `[]T` can be indexed")
                        .with_help("dynamic arrays `[..]T` arrive in a later wave"),
                );
            }
            return PoolId::ERROR;
        };

        // An index must be an integer. `bool` and `string` are the reachable mistakes; a
        // pointer is deliberately included, because allowing one would be the first half of
        // pointer arithmetic.
        let index_is_integer =
            index_ty == PoolId::ERROR || matches!(self.pool.item(index_ty), Item::IntType { .. });
        if !index_is_integer {
            let text = self.describe(index_ty);
            self.diags.push(
                Diagnostic::error(
                    index_span,
                    format!("an index must be an integer, not `{text}`"),
                )
                .with_code(E0235),
            );
            return elem;
        }

        // A literal index is decidable now, and a program that can only ever trap is
        // better refused. This does **not** replace the runtime check: it is one shape of
        // index out of many, and ADR-0039 §2's `BoundsCheck` still guards the rest.
        // **Withheld for a template's placeholder length** (ADR-0089 §2): a `[N]s64` inside a `$N`
        // template resolves to `[0]s64` so the body can be typed at all, and every index would then be
        // "out of range" — a false error about a correct program. The instantiations carry real lengths
        // and are checked here normally, which is where a genuinely bad index is caught.
        if let Some(len) = len
            && !self.placeholder_arrays.contains(&base_ty)
            && let Expr::Literal(Literal::Int { value, .. }, _) = self.expr_of(scope, index)
            && (value < 0 || u128::try_from(value).is_ok_and(|v| v >= u128::from(len)))
        {
            let text = self.describe(base_ty);
            let mut diag = Diagnostic::error(
                index_span,
                format!("index {value} is out of range for `{text}`"),
            )
            .with_code(E0236);
            diag = if len == 0 {
                diag.with_note("this array has no elements, so no index is in range")
            } else {
                diag.with_note(format!("valid indices are 0 to {}", len - 1))
            };
            self.diags.push(diag);
            return elem;
        }

        elem
    }

    /// The element type of something indexable, and its length when that is known.
    ///
    /// `Some((elem, Some(n)))` for `[N]T` and `Some((elem, None))` for `[]T`. The `None` is
    /// not a failure — it says the length is runtime data, which is the whole difference
    /// between an array and a view (ADR-0044 §1) — so a caller must not treat it as one.
    fn indexable_parts(&self, ty: PoolId) -> Option<(PoolId, Option<u64>)> {
        match self.pool.item(ty) {
            Item::ArrayType { elem, len } => Some((*elem, Some(*len))),
            // **A vector indexes exactly as the array of the same bytes does** (ADR-0148 §1), which
            // is why this is here rather than in a vector-specific path: the lane count is a
            // compile-time length, so a literal index out of range is the same E0236 an array's is,
            // and `Statement::BoundsCheck` guards a dynamic one. That falls out of the layouts being
            // identical, and it is the reason reading a lane needed no MIR change at all.
            Item::VectorType { elem, lanes } => Some((*elem, Some(*lanes))),
            Item::ViewType { elem } => Some((*elem, None)),
            _ => None,
        }
    }

    /// Types `base[]` — the slice operator (ADR-0044 §2).
    ///
    /// Only a `[N]T` may be sliced, and only into a `[]T` of the same element type. A view
    /// may **not** be sliced again: `xs[]` would be an identity, and an operator that
    /// silently does nothing is one a reader concludes did something (ADR-0044 §6).
    fn check_slice(&mut self, scope: ExprScope, base: ExprId, span: Span) -> PoolId {
        let mut base_ty = self.check_expr(scope, base, None);
        // Auto-deref, matching `check_index` and `check_field`: `p: *[4]u8` slices through
        // the pointer. The same loop in all three, so they cannot disagree about depth.
        while let Some(inner) = self.pointee(base_ty) {
            base_ty = inner;
        }
        if base_ty == PoolId::ERROR {
            return PoolId::ERROR;
        }

        let Some((elem, _)) = self.array_parts(base_ty) else {
            let text = self.describe(base_ty);
            let mut diag =
                Diagnostic::error(span, format!("cannot slice a value of type `{text}`"))
                    .with_code(E0239)
                    .with_note("`[]` makes a view over a fixed-size array `[N]T`");
            // A view sliced again is the mistake worth naming specifically, because the
            // expression *looks* harmless and the fix is to delete the operator.
            if matches!(self.pool.item(base_ty), Item::ViewType { .. }) {
                diag = diag.with_help("this is already a view — drop the `[]`");
            }
            self.diags.push(diag);
            return PoolId::ERROR;
        };

        // A view of a *constant* array would point at storage that has no address, so the
        // base must be a place. `is_place` is the same predicate assignment uses, which is
        // what keeps "can I take its address" one question with one answer.
        if !self.is_place(scope, base) {
            self.diags.push(
                Diagnostic::error(span, "cannot slice this expression")
                    .with_code(E0239)
                    .with_note("`[]` takes the address of its operand, so it needs storage")
                    .with_help("assign it to a variable first, then slice that"),
            );
            return self.pool.view_of(elem);
        }

        self.pool.view_of(elem)
    }

    /// The type an expression *denotes*, when it is a name bound to one (ADR-0071 §2).
    ///
    /// This is what makes `T :: Point;` an alias usable in a type annotation: `resolve_type_name`
    /// reads a `SigEntry::type_value`, so a type-valued constant has to carry the type it denotes
    /// rather than only `PoolId::TYPE`.
    ///
    /// **Reads the aliased name's own entry rather than re-resolving what `Point` means**, for the
    /// reason `jr-mir` reads `TypeMap` instead of typing expressions: two implementations of one rule
    /// are two chances to disagree. The signature phase already computed it.
    ///
    /// `None` for anything that is not a bare name — including a *chain* (`B :: A` where `A :: Point`),
    /// because `A`'s entry is a `SigKind::Const` and following it would need a fixpoint and a cycle
    /// check (ADR-0071 §5, the line ADR-0070 §4 drew for an array length).
    pub(crate) fn aliased_type(&mut self, scope: ExprScope, expr: ExprId) -> Option<PoolId> {
        let Expr::Name { res, .. } = self.expr_of(scope, expr) else {
            return None;
        };
        let res = self.resolve.get(scope, expr).unwrap_or(res);
        let entry = match res {
            Res::Item(item) => self.entry_for_item(item)?,
            Res::Imported(import, name) => self.entry_for_import(import, name)?,
            // A local, a parameter, or a promoted field is never a type: Jairs has no nested type
            // declarations, so none of them can put a type name in scope.
            Res::Local(_) | Res::Param(_) | Res::Promoted { .. } | Res::Error => return None,
        };
        // Only a *nominal declaration* is followed. A `SigKind::Const` whose own `type_value` is set is
        // exactly the alias chain §5 defers, so it is excluded by kind rather than by whether the field
        // happens to be populated — which keeps the refusal true if a later wave populates more of them.
        match entry.kind {
            SigKind::Struct | SigKind::Union | SigKind::Variant | SigKind::Enum => entry.type_value,
            SigKind::Const | SigKind::Var | SigKind::Proc | SigKind::Operator => None,
        }
    }

    /// The enum an expression *denotes*, when it is a name bound to an enum type.
    ///
    /// A receiver like `Colour` has type `type` (ADR-0012), so the type alone cannot say
    /// which enum — the *name* must be resolved to its declaration. Returns `None` for any
    /// other type-valued expression, including a struct name, which is what makes
    /// `Point.x` report "no field" rather than being mistaken for a member lookup.
    fn denoted_enum(
        &mut self,
        scope: ExprScope,
        receiver: ExprId,
    ) -> Option<(jr_pool::DeclId, bool)> {
        let Expr::Name { res, .. } = self.expr_of(scope, receiver) else {
            return None;
        };
        let res = self.resolve.get(scope, receiver).unwrap_or(res);
        let entry = match res {
            Res::Item(item) => self.entry_for_item(item)?,
            Res::Imported(import, name) => self.entry_for_import(import, name)?,
            // A promoted name is a *field*, and a field never denotes a type — Jairs has no
            // nested type declarations, so `using p: Point` cannot put a type name in scope.
            Res::Local(_) | Res::Param(_) | Res::Promoted { .. } | Res::Error => return None,
        };
        let denoted = entry.type_value?;
        match self.pool.item(denoted) {
            Item::EnumType { decl, flags } => Some((*decl, *flags)),
            _ => None,
        }
    }

    /// Reports a name that is not a member of the enum it was looked up in.
    ///
    /// The candidate set is the enum's own members, which is why the suggestion is computed
    /// here rather than in an editor: nothing outside this crate knows them (ADR-0031 §1).
    fn no_such_member(&mut self, decl: jr_pool::DeclId, flags: bool, member: &str, span: Span) {
        let candidates: Vec<String> = self
            .pool
            .enum_members(decl)
            .unwrap_or(&[])
            .iter()
            .map(|m| self.interner.resolve(m.name).to_owned())
            .collect();
        let ty = self.pool.enum_type(decl, flags);
        let text = self.describe(ty);
        let mut diag =
            Diagnostic::error(span, format!("`{text}` has no member `{member}`")).with_code(E0238);
        if let Some(near) = crate::suggest::nearest(member, candidates.iter().map(String::as_str)) {
            diag = diag.with_help(format!("did you mean `{near}`?"));
        }
        self.diags.push(diag);
    }

    /// Reports a field the receiver's type does not have, suggesting a near one.
    ///
    /// The candidate list is the receiver's own fields, which is why the suggestion is
    /// computed here rather than in an editor: nothing outside this crate knows them
    /// (ADR-0031 §1). Field order is declaration order, so a tie resolves to the field
    /// declared first rather than to whatever the pool iterated over.
    fn no_such_field(&mut self, ty: PoolId, field: &str, span: Span) {
        let text = self.describe(ty);
        let candidates: Vec<String> = match self.pool.item(ty) {
            // ADR-0004's two pseudo-fields, spelled out because `string` is not the
            // struct of its own layout and the pool has no field list for it.
            Item::StringType => vec![String::from("data"), String::from("count")],
            // Only `count`. Listing `data` would suggest a pseudo-field arrays do not have
            // (ADR-0039 §5), which is worse than no suggestion.
            Item::ArrayType { .. } | Item::VectorType { .. } | Item::ViewType { .. } => {
                vec![String::from("count")]
            }
            // A `[..]T` exposes three (ADR-0136 §1); listing them helps the suggestion pick
            // the right one for a near miss like `xs.cout`.
            Item::DynamicArrayType { .. } => vec![
                String::from("data"),
                String::from("count"),
                String::from("capacity"),
            ],
            Item::StructType { decl, .. }
            | Item::UnionType { decl, .. }
            | Item::VariantType { decl, .. } => self
                .pool
                .struct_fields(*decl)
                .unwrap_or(&[])
                .iter()
                .map(|f| self.interner.resolve(f.name).to_owned())
                .collect(),
            _ => Vec::new(),
        };

        let mut diag = Diagnostic::error(span, format!("no field `{field}` on type `{text}`"))
            .with_code(E0218);
        if let Some(near) = crate::suggest::nearest(field, candidates.iter().map(String::as_str)) {
            diag = diag.with_help(format!("did you mean `{near}`?"));
        }
        self.diags.push(diag);
    }

    /// Types a dereference.
    fn check_deref(&mut self, scope: ExprScope, pointer: ExprId, span: Span) -> PoolId {
        let ty = self.check_expr(scope, pointer, None);
        if ty == PoolId::ERROR {
            return PoolId::ERROR;
        }
        match self.pointee(ty) {
            Some(inner) => inner,
            None => {
                let text = self.describe(ty);
                self.diags.push(
                    Diagnostic::error(span, format!("cannot dereference `{text}`"))
                        .with_code(E0219)
                        .with_note("`.*` applies to a pointer"),
                );
                PoolId::ERROR
            }
        }
    }

    /// Types a directive used as an expression.
    fn check_directive(
        &mut self,
        name: Symbol,
        arg: Option<&str>,
        expected: Option<PoolId>,
        span: Span,
    ) -> PoolId {
        let interner = self.interner;
        let ty = match interner.resolve(name) {
            // ADR-0016 §3. The value is interned as well as the type, so that the
            // FFI boundary has an identity and not merely a shape.
            "system_library" | "library" => {
                if let Some(library) = arg {
                    let _ = self.pool.foreign_library_value(library);
                }
                PoolId::FOREIGN_LIBRARY
            }
            // Every other directive in expression position was already rejected
            // by lowering (E0209); a second complaint would be noise.
            _ => PoolId::ERROR,
        };
        self.expect(expected, ty, span)
    }

    // -----------------------------------------------------------------------
    // Expression predicates
    // -----------------------------------------------------------------------

    /// Returns `true` if this expression denotes a location, not just a value.
    ///
    /// Called after the expression has been typed, because a field access on a
    /// pointer is assignable and deciding that needs the receiver's type.
    fn is_place(&mut self, scope: ExprScope, id: ExprId) -> bool {
        match self.expr_of(scope, id) {
            // **`context` itself is not a place** — it is the pointer value, not storage — but
            // `context.allocator` is, because `Expr::Field` on a pointer receiver is assignable and
            // that arm decides it from the receiver's *type*. So writing the field works and
            // rebinding `context` wholesale does not, which is ADR-0057 §2's shape exactly.
            Expr::Context(_) => false,
            Expr::Name { res, .. } => {
                let res = self.resolve.get(scope, id).unwrap_or(res);
                match res {
                    Res::Local(_) | Res::Param(_) => true,
                    // A promoted name **is** a place: `x` where `using p: Point` is in scope means
                    // `p.x`, and an ordinary `p.x` is assignable. Answering `false` here would
                    // silently make `x = 1` a "cannot assign" error inside any procedure taking a
                    // `using` parameter — the promotion would look read-only for no stated reason.
                    Res::Promoted { .. } => true,
                    Res::Item(item) => self
                        .entry_for_item(item)
                        .is_some_and(|entry| entry.is_assignable()),
                    Res::Imported(import, name) => self
                        .entry_for_import(import, name)
                        .is_some_and(|entry| entry.is_assignable()),
                    // Poison: an unresolved name is already an error, and calling
                    // it unassignable as well would report it twice.
                    Res::Error => true,
                }
            }
            Expr::Field { receiver, .. } => {
                // An enum member is a compile-time constant with no storage, so
                // `Colour.RED = 2` is not assignable and `*Colour.RED` has no address to
                // take (ADR-0041 §5). Checked on the *receiver*: a type-valued receiver is
                // never a place, which is also the right answer for a hypothetical
                // `Point.x`.
                let receiver_is_type = self
                    .types
                    .expr_type(scope, receiver)
                    .is_some_and(|ty| ty == PoolId::TYPE);
                if receiver_is_type {
                    return false;
                }
                let through_pointer = self
                    .types
                    .expr_type(scope, receiver)
                    .is_some_and(|ty| self.pointee(ty).is_some());
                through_pointer || self.is_place(scope, receiver)
            }
            // Indexing names a location whenever the thing indexed does. `a[i] = x` is
            // legal for a local array; a hypothetical array-valued *constant* is not
            // assignable, and this defers to the base for exactly that reason rather than
            // answering `true` outright the way `Deref` can.
            Expr::Index { base, .. } => self.is_place(scope, base),
            // A view *is* a pointer to storage, so indexing one always names a location —
            // there is nothing to defer to the base about. But `xs[]` itself produces a
            // two-word value, so slicing is not a place (ADR-0044 §4).
            Expr::Slice { .. } => false,
            // A dereference always names a location.
            Expr::Deref(..) => true,
            // A cast produces a *value*, never a location: `cast(u8, n) = 1` is not
            // assignable even though `n` is.
            Expr::Literal(..)
            | Expr::Binary { .. }
            | Expr::Unary { .. }
            | Expr::Call { .. }
            | Expr::Uninit(_)
            | Expr::Cast { .. }
            // Both produce values. A bare `.RED` is a constant with no storage, exactly as
            // `Colour.RED` is (ADR-0041 §5), and `xx n` is a conversion's result.
            | Expr::Autocast { .. }
            | Expr::Member { .. }
            | Expr::Run(..)
            | Expr::Directive { .. } => false,
            // Error recovery: stay quiet.
            Expr::Error(_) => true,
        }
    }

    /// Returns `true` if this expression is built only out of integer literals.
    ///
    /// Such an expression has no type of its own and takes the other operand's,
    /// which is what makes `1 + 2 == count` compare `s64`s rather than reporting
    /// a mismatch against a defaulted `s64`.
    fn is_untyped_literal(&self, scope: ExprScope, id: ExprId) -> bool {
        match self.expr_of(scope, id) {
            Expr::Literal(literal, _) => match literal {
                // A float literal is untyped for the same reason an integer one is
                // (ADR-0040 §5): it takes its type from context, so `1.5 + x` where `x` is a
                // `float32` must make the literal a `float32` rather than defaulting it to
                // `float64` and then reporting a mismatch.
                //
                // Note this does *not* let `1 + some_float64` through: `1` is an *integer*
                // literal, and its context typing gives it the integer interpretation, so the
                // operands still disagree. ADR-0040 §6 keeps that asymmetry deliberately —
                // `1` and `1.0` are different literals.
                Literal::Int { .. } | Literal::Float { .. } => true,
                // `null` takes its type from context too (ADR-0060 §1), so `p == null` types the
                // `null` as `p`'s pointer type rather than reporting a mismatch — the same reason
                // an integer literal is untyped here.
                Literal::Null => true,
                Literal::Str(_) | Literal::Bool(_) => false,
            },
            Expr::Unary { op, operand, .. } => match op {
                // `~1` is untyped for the same reason `-1` is: the complement of an untyped
                // literal is still an untyped literal, so `x: u8 = ~0;` must take `u8` from
                // its context rather than defaulting to `s64` and then mismatching.
                UnOp::Neg | UnOp::BitNot => self.is_untyped_literal(scope, operand),
                UnOp::Not | UnOp::AddrOf => false,
            },
            Expr::Binary { op, lhs, rhs, .. } => {
                is_arithmetic(op)
                    && self.is_untyped_literal(scope, lhs)
                    && self.is_untyped_literal(scope, rhs)
            }
            Expr::Run(inner, _) => self.is_untyped_literal(scope, inner),
            // An element of an array has the element's type, which is a real type: `buf[0]`
            // is a `u8` and must not take the other operand's type the way `1` does.
            // A view is a real type for the same reason.
            Expr::Index { .. } | Expr::Slice { .. } => false,
            // A cast is emphatically **not** untyped: naming a type is the whole point, so
            // `cast(u8, 1) + big_s64` must be a type error rather than quietly taking `s64`.
            // Answering `true` here would make the cast advisory.
            // Neither is untyped: an `xx` has the context's type and a `.RED` has its enum's.
            // Answering `true` would make them take the *other* operand's type in a binary
            // expression — a second context-typing rule fighting the first (ADR-0046 §1).
            Expr::Cast { .. }
            | Expr::Autocast { .. }
            | Expr::Member { .. }
            | Expr::Name { .. }
            | Expr::Call { .. }
            | Expr::Context(_)
            | Expr::Field { .. }
            | Expr::Deref(..)
            | Expr::Uninit(_)
            | Expr::Directive { .. }
            | Expr::Error(_) => false,
        }
    }

    // -----------------------------------------------------------------------
    // Arena access
    // -----------------------------------------------------------------------

    /// Returns the expression `id` names in the arena `scope` selects.
    ///
    /// An index outside that arena yields `Expr::Error`, so a mismatched
    /// arena degrades to poison instead of silently reading another node.
    fn expr_of(&self, scope: ExprScope, id: ExprId) -> Expr {
        let hir = self.hir;
        let arena = match scope {
            ExprScope::TopLevel => &hir.exprs,
            ExprScope::Body(body) => &hir.body(body).exprs,
        };
        arena
            .get(id.index())
            .cloned()
            .unwrap_or(Expr::Error(self.nowhere()))
    }

    /// A span for a node that has none, used only in error recovery.
    fn nowhere(&self) -> Span {
        Span::new(self.file, TextRange::default())
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// Which field lookup a receiver's type calls for.
enum ReceiverKind {
    /// The builtin `string`, whose `.data`/`.count` are pseudo-fields.
    Str,
    /// A nominal struct, whose fields the pool holds for this **instance** type.
    ///
    /// Carries the instance `PoolId` rather than a bare `DeclId` (ADR-0085 §2): a parameterised
    /// `Box(s64)` and `Box(bool)` share a declaration but have substituted field types, so the
    /// lookup must be through `Pool::fields_of` on the instance. An ordinary struct's instance is
    /// its `DeclId`-keyed type, so its fields are found unchanged.
    Struct(PoolId),
    /// A fixed-size array, whose `.count` is a pseudo-field read from the type.
    Array,
    /// The implicit context, whose fields are the compiler's (ADR-0057 §1).
    ///
    /// Its own variant rather than a [`ReceiverKind::Struct`] because a context has no `DeclId` — a
    /// compiler-declared type has no declaration site — so its fields cannot be in the struct side
    /// table that variant reads.
    Context,
    /// A view, whose `.count` is a pseudo-field **loaded** from the value (ADR-0044 §4).
    ///
    /// Distinct from [`ReceiverKind::Array`] even though both answer `.count` with an `s64`,
    /// because the two differ in *where the answer comes from*: an array's is a constant from
    /// the type and a view's is a load. MIR needs that difference and a shared variant would
    /// hide it.
    View,
    /// A dynamic array `[..]T`, whose pseudo-fields are `.data: *T`, `.count: s64`,
    /// `.capacity: s64` — all loaded from the value (ADR-0136 §1). Distinct from
    /// [`ReceiverKind::View`] because the field layout is one word longer and `.data` is
    /// exposed here — a dynamic array *owns* its data and a caller who wants to inspect the
    /// pointer must be able to.
    DynamicArray(PoolId),
    /// An enum type used as a receiver, whose "fields" are its members (ADR-0041 §1).
    ///
    /// Carries `flags` as well as the declaration, because rebuilding the type needs it
    /// (ADR-0043 §2) and a second lookup is a second chance to disagree.
    Enum(jr_pool::DeclId, bool),
    /// Anything else: no fields at all.
    Fieldless,
}

/// Returns `true` for the arithmetic operators.
fn is_arithmetic(op: BinOp) -> bool {
    match op {
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Div
        | BinOp::Rem
        | BinOp::WrapAdd
        | BinOp::WrapSub
        | BinOp::WrapMul => true,
        // Bitwise operators are *not* "arithmetic" for this predicate's purpose: its only
        // caller distinguishes an arithmetic message from an ordering one when rejecting an
        // enum operator (ADR-0041 §6), and a bitwise operator on an enum gets its own message
        // from `reject_bitwise` before reaching that path.
        BinOp::Eq
        | BinOp::Ne
        | BinOp::Lt
        | BinOp::Le
        | BinOp::Gt
        | BinOp::Ge
        | BinOp::And
        | BinOp::Or
        | BinOp::BitAnd
        | BinOp::BitOr
        | BinOp::BitXor
        | BinOp::Shl
        | BinOp::Shr => false,
    }
}

/// The source spelling of a binary operator, for diagnostics.
pub(crate) fn bin_op_text(op: BinOp) -> &'static str {
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
        BinOp::BitAnd => "&",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
    }
}

/// Returns `true` if an integer literal of magnitude `value` fits the type.
///
/// The HIR stores a literal's magnitude, never a sign — `-1` is negation applied
/// to `1` — so this is a bound on the magnitude. The consequence, stated because
/// it is a real limitation: the most negative value of a signed type cannot be
/// written as a literal, because its magnitude is one past the positive bound.
///
/// Written against `(signed, bits)` rather than against `s64` and `u8` so that W1's
/// full numeric tower would not have to rewrite it — which it did not (ADR-0037).
fn literal_fits(signed: bool, bits: u16, value: i128) -> bool {
    // Against the type's **range**, not its maximum magnitude. The old test compared a
    // magnitude, so `-128` was 128 tested against `s8`'s 127 and every signed minimum was
    // unwritable (ADR-0038). `IntKind` already computes both bounds, and using it here means
    // the fit check and the arithmetic cannot disagree about what a type holds.
    let kind = jr_pool::IntKind { signed, bits };
    value >= kind.min() && value <= kind.max()
}

/// A human-readable range for an integer type, for the E0204 note.
///
/// From `IntKind`, the same source `literal_fits` tests against — so the note cannot print a
/// range the check does not enforce. It used to derive both bounds from the maximum magnitude,
/// which is how it came to print "the range of `s8` is -128 to 127" while rejecting `-128`.
fn int_range(signed: bool, bits: u16) -> String {
    let kind = jr_pool::IntKind { signed, bits };
    format!("{} to {}", kind.min(), kind.max())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_bounds_are_per_type_not_per_s64() {
        assert!(literal_fits(false, 8, 255));
        assert!(!literal_fits(false, 8, 256));
        assert!(literal_fits(true, 64, i128::from(i64::MAX)));
        assert!(!literal_fits(true, 64, i128::from(i64::MAX) + 1));
        assert!(literal_fits(false, 64, i128::from(u64::MAX)));
    }

    #[test]
    fn a_signed_minimum_fits_its_own_type() {
        // The bug ADR-0038 fixed. `literal_fits` compared a *magnitude* against the maximum,
        // so 128 was tested against `s8`'s 127 and every signed minimum was rejected — by a
        // diagnostic that printed the range the value sits inside.
        for bits in [8u16, 16, 32, 64] {
            let kind = jr_pool::IntKind { signed: true, bits };
            assert!(
                literal_fits(true, bits, kind.min()),
                "s{bits}'s minimum must fit s{bits}"
            );
            assert!(
                !literal_fits(true, bits, kind.min() - 1),
                "one below s{bits}'s minimum must not"
            );
            assert!(literal_fits(true, bits, kind.max()));
            assert!(!literal_fits(true, bits, kind.max() + 1));
        }
    }

    #[test]
    fn a_negative_literal_never_fits_an_unsigned_type() {
        // Free with a signed comparison, and *not* free with a magnitude one: the old test
        // would have accepted `u8 = -1` as the magnitude 1.
        for bits in [8u16, 16, 32, 64] {
            assert!(!literal_fits(false, bits, -1));
        }
    }

    #[test]
    fn ranges_read_the_way_a_user_expects() {
        assert_eq!(int_range(false, 8), "0 to 255");
        assert_eq!(int_range(true, 8), "-128 to 127");
    }

    #[test]
    fn every_printed_range_is_a_range_the_check_accepts() {
        // The note and the check now read the same `IntKind`, which is what stops the
        // diagnostic printing a bound it then rejects — the shape of the ADR-0038 bug.
        for bits in [8u16, 16, 32, 64] {
            for signed in [true, false] {
                let kind = jr_pool::IntKind { signed, bits };
                let printed = int_range(signed, bits);
                assert!(printed.starts_with(&kind.min().to_string()), "{printed}");
                assert!(printed.ends_with(&kind.max().to_string()), "{printed}");
                assert!(literal_fits(signed, bits, kind.min()));
                assert!(literal_fits(signed, bits, kind.max()));
            }
        }
    }
}
