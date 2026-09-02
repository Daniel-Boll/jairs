//! The analysis context shared by both phases.
//!
//! # One context, two phases
//!
//! [`file_signatures`](crate::file_signatures) and [`check_file`](crate::check_file)
//! are the same machine in two configurations, not two implementations. They
//! share expression typing, type resolution, and diagnostic wording, and they
//! differ in exactly one thing: where a file-level name's type comes from. In
//! [`Mode::Signatures`] it is computed on demand, recursively, with a cycle
//! guard; in [`Mode::Check`] it is read out of the already-computed
//! [`FileSignatures`].
//!
//! The alternative — a separate, smaller typer for constant initialisers — was
//! tried on paper and rejected: two typers that agree today are two typers that
//! disagree later, and the disagreement would be silent, because a constant's
//! type is not written down anywhere a test could compare.
//!
//! # The arena trap
//!
//! Neither an [`ExprId`](jr_hir::ExprId) nor a [`TypeRefId`] is unique within a
//! file. `FileHir::exprs` and every `Body::exprs` are independent arenas that
//! start at 0, and the same is true of `FileHir::type_refs` and
//! `Body::type_refs`. Which arena an id indexes depends on *where it came from*,
//! not on the id. Every accessor here therefore takes an [`ExprScope`] saying
//! which arena is meant, and an index that falls outside that arena yields a
//! poison value rather than panicking or — worse — silently reading a different
//! node.
//!
//! `Proc::type_refs` and `Struct::type_refs` exist but are always empty:
//! `jr-hir`'s lowering puts parameter, return and field types in
//! `FileHir::type_refs`. Only a local's annotation lives in `Body::type_refs`.

use jr_base::{FileId, Interner, Span, Symbol};
use jr_diag::{Diagnostic, Diagnostics};
use jr_hir::{
    BodyId, ConstValue, ExprId, ExprScope, FileHir, ItemId, ItemKind, LocalId, ResolveMap,
    StructId, TypeRef, TypeRefId,
};
use jr_pool::{ContextKind, DeclId, IntKind, Item, Pool, PoolId};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::code::{
    E0211, E0212, E0213, E0214, E0233, E0237, E0240, E0269, E0270, E0282, E0283, E0284, E0285,
};
use crate::map::TypeMap;
use crate::sigs::{FileSignatures, SigEntry, SigKind};

/// The largest alignment `#align` may request, in bytes (ADR-0144 §3).
///
/// One page. Past it a stack slot cannot promise the alignment it was asked for, and a request
/// silently not met is worse than a refusal — so the ceiling is where the promise stops being
/// keepable rather than an arbitrary round number.
const MAX_FIELD_ALIGN: u32 = 4096;

// ---------------------------------------------------------------------------
// Mode
// ---------------------------------------------------------------------------

/// Which phase the context is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Computing this file's signatures. File-level names are typed on demand.
    Signatures,
    /// Checking this file's bodies. File-level names are already typed.
    Check,
}

// ---------------------------------------------------------------------------
// BodyEnv
// ---------------------------------------------------------------------------

/// The procedure context a body is being checked in.
///
/// Carries the parameter and return types by value rather than borrowing them
/// out of [`FileSignatures`], because the checker needs `&mut self` throughout
/// and a signature borrow would conflict with every diagnostic it pushes.
///
/// It exists at all because **`Body` has no back-pointer to its `Proc`**: a
/// `Res::Param(ParamId)` indexes `Proc::params`, and nothing in the body says
/// which `Proc` that is. The mapping is recovered by scanning `FileHir::procs`
/// for the proc whose `body` is this one.
pub(crate) struct BodyEnv {
    /// The body being checked.
    pub(crate) id: BodyId,
    /// The enclosing procedure's parameter types, in order.
    pub(crate) params: Vec<PoolId>,
    /// The enclosing procedure's return type.
    pub(crate) ret: PoolId,
}

// ---------------------------------------------------------------------------
// Ctx
// ---------------------------------------------------------------------------

/// The state shared by type resolution, signature computation, and checking.
pub(crate) struct Ctx<'a> {
    /// The file under analysis.
    pub(crate) hir: &'a FileHir,
    /// Its stable id, which is half of every nominal type's identity.
    pub(crate) file: FileId,
    /// Name resolution for this file.
    pub(crate) resolve: &'a ResolveMap,
    /// The shared string interner.
    pub(crate) interner: &'a Interner,
    /// The shared type and value pool.
    pub(crate) pool: &'a mut Pool,
    /// The signatures of each imported module, in source order.
    ///
    /// A `Vec` rather than a map because import order decides which module a
    /// flat-merged name comes from when reporting ambiguity, and iteration order
    /// over a hash map is not stable.
    pub(crate) imports: Vec<(&'a str, &'a FileSignatures)>,
    /// Each imported module's **HIR and `FileId`**, for resolving a *parameterised* imported struct's fields
    /// (ADR-0117 §1).
    ///
    /// Signatures are not enough. A parameterised struct's fields must be resolved **per instance, under the
    /// caller's type arguments** (ADR-0085 §2) — and its own file cannot do that, because it does not know what
    /// arguments an importer will supply and records its body with the variables bound to `PoolId::ERROR`. So the
    /// *importer* resolves them, which needs the field `TypeRef` tree — and a `TypeRef` is a `TypeRefId` into the
    /// **declaring** file's arena.
    ///
    /// The HIR rather than a flattened copy of those `TypeRef`s on the signatures (ADR-0117 §1): a copy would be
    /// a second representation of the same tree, re-indexed into a private arena — a second thing to keep
    /// correct, which is the drift ADR-0022 §2 refuses for arithmetic. The HIR is already loaded and is what the
    /// *signature* phase uses for the same job.
    ///
    /// Empty in every test harness that checks a file alone, which is why every reader must tolerate a miss.
    pub(crate) imported_hirs: Vec<(FileId, &'a FileHir)>,
    /// While resolving an **imported** parameterised struct's fields, that module's signatures (ADR-0117 §2).
    ///
    /// `None` in the ordinary case. Set for the duration of `resolve_instance_fields_in`, because a field naming
    /// a type — `Wrapper($T) { helper: Helper; }` — must find the **declaring** module's `Helper`, not the
    /// importer's. `self.hir` is swapped so `hir.scope` is right, but the *type value* comes from a
    /// `FileSignatures`, and this is which one to ask.
    ///
    /// A field's type cannot depend on who imported the struct, so consulting the importer's signatures here
    /// would be wrong even when it happened to find a same-named type — the worse failure, because it would
    /// silently resolve to a *different* type rather than not resolving at all.
    pub(crate) resolving_in_module: Option<&'a FileSignatures>,
    /// This file's signatures: under construction in [`Mode::Signatures`],
    /// already complete in [`Mode::Check`].
    pub(crate) sigs: FileSignatures,
    /// Which phase this is.
    pub(crate) mode: Mode,
    /// Diagnostics produced so far.
    pub(crate) diags: Diagnostics,
    /// Types learned so far.
    pub(crate) types: TypeMap,
    /// Items whose signature is currently being computed, innermost last.
    pub(crate) in_progress: Vec<ItemId>,
    /// Items whose signature is finished.
    pub(crate) finished: FxHashSet<ItemId>,
    /// The body being checked, if any.
    pub(crate) body: Option<BodyEnv>,
    /// The resolved type of every local seen so far.
    pub(crate) locals: FxHashMap<(BodyId, LocalId), PoolId>,
    /// Which overload each operator expression resolved to (ADR-0048 §5).
    ///
    /// Collected here and handed to `CheckOutput` when the context is dropped, exactly as
    /// `types` and `diags` are.
    pub(crate) operator_calls: FxHashMap<(ExprScope, jr_hir::ExprId), (FileId, jr_hir::ProcId)>,
    /// Positional argument lists for calls using a named argument or a default (ADR-0053 §1).
    pub(crate) filled_calls: FxHashMap<(ExprScope, jr_hir::ExprId), Vec<crate::check::ArgSlot>>,
    /// Callee expressions in *call* position (ADR-0059 §5).
    ///
    /// A `#foreign` procedure is legal to call and illegal to take as a value (E0256). The `Name`
    /// arm of `check_expr` cannot tell the two apart on its own — a callee is a `Name` too — so
    /// `check_call` records the callee's id here first, and the arm skips the refusal for one it
    /// finds. The same `(ExprScope, ExprId)` keying `operator_calls` and `filled_calls` use, for
    /// the same reason: an id alone does not say which arena it indexes.
    pub(crate) call_position: FxHashSet<(ExprScope, jr_hir::ExprId)>,
    /// Expressions where a **type** is a legal thing to name (ADR-0071 §3).
    ///
    /// Exactly two positions, and both are recorded by the code that creates them rather than
    /// inferred from the expression's shape:
    ///
    /// * the **receiver of a field access** — `Colour.RED`, whose receiver is the enum type used as a
    ///   value (ADR-0041 §1);
    /// * the **initialiser of a `::` constant** — `T :: Point;`, the one place a type value is bound.
    ///
    /// Everywhere else a `type`-typed name is E0261. An allowlist rather than a denylist because the
    /// failure directions are not symmetric: a missed *legal* position is a false error the reader can
    /// see and report, while a missed *illegal* one is the silent placeholder this wave exists to
    /// remove. `call_position` above is the same mechanism for the same kind of reason.
    pub(crate) type_position: FxHashSet<(ExprScope, jr_hir::ExprId)>,
    /// Which type each `type_info(T)` call describes (ADR-0075 §2).
    ///
    /// Recorded here because the argument is a *type*, and a type is not an operand: by the time
    /// lowering sees the call there is nothing in the expression tree that carries a `PoolId`. Keyed the
    /// way `operator_calls` and `filled_calls` are, for the same reason — an `ExprId` alone does not say
    /// which arena it indexes.
    pub(crate) type_info_calls: FxHashMap<(ExprScope, jr_hir::ExprId), PoolId>,
    /// Each `typed`/`untyped` call and the pointer type it produces (ADR-0106 §1).
    ///
    /// Carried to `jr-mir` because the conversion is **real code** rather than a fold: a pointer's bits do not
    /// depend on its pointee, so retyping is a store-then-load through a slot (the mechanism ADR-0076 §1
    /// already uses), and lowering needs to know the target type to make the slot.
    pub(crate) pointer_views: FxHashMap<(ExprScope, jr_hir::ExprId), PoolId>,
    /// Which atomic operation each `atomic_*` call performs, as an `AtomicOp` code (ADR-0176 §3).
    pub(crate) atomics: FxHashMap<(ExprScope, jr_hir::ExprId), u8>,
    /// Calls folded to a value here rather than downstream — `has_note`, `note_value` (ADR-0099 §2).
    pub(crate) folded_calls: FxHashMap<(ExprScope, jr_hir::ExprId), PoolId>,
    /// The same values keyed by span, so an *expanded* tree can still find them (ADR-0101 §3).
    pub(crate) folded_call_spans: FxHashMap<jr_base::Span, PoolId>,
    /// Which `Any` operation each `any_of`/`any_as` call is, and the type it concerns (ADR-0076).
    ///
    /// Deliberately **not** merged with `type_info_calls`: that map replaces a call with a constant, and
    /// these lower to real code.
    pub(crate) any_calls: FxHashMap<(ExprScope, jr_hir::ExprId), (crate::check::AnyOp, PoolId)>,
    /// The polymorphic type variables currently in scope, and the type each is bound to (ADR-0081 §1).
    ///
    /// Empty except while checking a polymorphic procedure's signature or an instantiation of one. A `$T`
    /// **binds** its name here (to the inferred type during an instantiation, or to a fresh placeholder
    /// while the uninstantiated signature is walked), and a bare `T` elsewhere in the same signature
    /// **reads** it — which is what distinguishes a use of a type variable from an unknown type name.
    ///
    /// A map rather than a stack because a signature's variables are flat: `$T` binds once, and every
    /// later `T` in that signature is the same variable. It is cleared when the signature or
    /// instantiation is left.
    pub(crate) type_bindings: FxHashMap<Symbol, PoolId>,
    /// Each polymorphic call and the instantiation it requires: `(proc, bound type)` (ADR-0082 §1).
    ///
    /// Recorded by `check_polymorphic_call` and read by the expansion pass in `jr-db`, the way
    /// `type_info_calls` is — one type inference, reused, rather than a second walk. Keyed by the call
    /// expression's `(scope, id)`, so the pass can rewrite that exact call to target the instantiated
    /// procedure. Empty for a file with no polymorphic calls, which is every ordinary program.
    pub(crate) instantiations:
        FxHashMap<(ExprScope, jr_hir::ExprId), (jr_hir::ProcId, Vec<PoolId>)>,
    /// Each **comptime-value** call and the argument expressions its `$N` parameters need
    /// (ADR-0088 §1): `(proc, [arg ExprId per comptime parameter])`.
    ///
    /// Recorded by `check_comptime_call` for a `$N`-templated callee, and read by `jr-db`'s
    /// `comptime_call_values` pre-pass — which evaluates each argument to a constant, because a value is
    /// not known at check time (const-eval is downstream, ADR-0018 §3). The *expressions* are recorded
    /// here, not values; keyed by the call's `(scope, id)` like `instantiations`. Empty for a program
    /// with no comptime-value calls.
    pub(crate) comptime_calls:
        FxHashMap<(ExprScope, jr_hir::ExprId), (jr_hir::ProcId, Vec<jr_hir::ExprId>)>,
    /// Each variadic call, keyed on the call expression. Recorded by `check_call` when the
    /// callee's last parameter is `..T` and the arity is satisfied; consumed by `jr-mir` to
    /// pack the trailing arguments into a stack view (ADR-0138 §2). Empty for a file with no
    /// variadic calls.
    pub(crate) variadic_calls: FxHashMap<(ExprScope, jr_hir::ExprId), crate::check::VariadicCall>,
    /// Each `#soa` field access, keyed on the **index** expression that is its receiver, holding the
    /// field's position (ADR-0147 §2).
    ///
    /// `jr-mir` reads this to build `Field(position)` then `Index(i)` for a place whose HIR says
    /// `Index` then `Field` — the one place the two crates must agree, so one of them decides.
    pub(crate) soa_fields: FxHashMap<(ExprScope, jr_hir::ExprId), u32>,
    /// The **baked value** of each `$N` parameter of the procedure currently being resolved
    /// (ADR-0089 §1).
    ///
    /// The value-side counterpart of `type_bindings`: set from `FileHir::param_values` around an
    /// instantiation's signature and body, so an array length that *names* a comptime parameter —
    /// `buf: [N]s64` — resolves to the value the const-eval pre-pass produced. Empty outside an
    /// instantiation of a `$N` template, so an ordinary program costs one hash probe and changes nothing.
    pub(crate) value_bindings: FxHashMap<Symbol, PoolId>,
    /// The names of the `$N` comptime parameters of the procedure currently being resolved, whether or
    /// not their values are known (ADR-0089 §2).
    ///
    /// A **template**'s `$N` has no value — only its instantiations do — so an array length naming one
    /// cannot resolve while the template's own body is checked. Rather than report E0233 there (the
    /// program is correct; it is the template that has no value yet), the length resolves to a
    /// placeholder and the refusal is withheld. This set is what distinguishes "names a comptime
    /// parameter, so wait for the instantiation" from "names nothing usable, so refuse" — the same
    /// shape as `jr-hir`'s withheld E0201 inside a pending `#insert` (ADR-0073 §1).
    pub(crate) comptime_param_names: FxHashSet<Symbol>,
    /// The `$T` type-variable names of the procedure currently being checked, bound or not (ADR-0092 §1).
    ///
    /// A **template**'s `$T` has no binding — only its instantiations do — so `type_info(T)` in the
    /// template's own body cannot resolve to a type. Rather than report E0261 there (the program is correct;
    /// it is the template that has no binding yet), the call is withheld and yields a poisoned type, the
    /// same discipline `comptime_param_names` gives an array length whose value is not yet known. Each
    /// instantiation resolves `T` for real and is checked normally.
    pub(crate) poly_var_names: FxHashSet<Symbol>,
    /// Array types whose length is a **placeholder** because it named a `$N` comptime parameter of a
    /// template (ADR-0089 §2).
    ///
    /// A template's `[N]s64` resolves to `[0]s64` so the body can still be typed, and that placeholder
    /// must not produce *length-dependent* diagnostics: `buf[0]` would be "index 0 out of range for
    /// `[0]s64`", a false error about a correct program. Every check that reads a length consults this set
    /// and withholds. The instantiations resolve real lengths and are checked normally, which is where a
    /// genuinely out-of-range index *is* caught.
    pub(crate) placeholder_arrays: FxHashSet<PoolId>,
}

impl<'a> Ctx<'a> {
    /// Creates a context.
    pub(crate) fn new(
        hir: &'a FileHir,
        file: FileId,
        resolve: &'a ResolveMap,
        interner: &'a Interner,
        pool: &'a mut Pool,
        imports: Vec<(&'a str, &'a FileSignatures)>,
        imported_hirs: Vec<(FileId, &'a FileHir)>,
        mode: Mode,
    ) -> Self {
        Self {
            call_position: FxHashSet::default(),
            type_position: FxHashSet::default(),
            type_info_calls: FxHashMap::default(),
            folded_calls: FxHashMap::default(),
            pointer_views: FxHashMap::default(),
            atomics: FxHashMap::default(),
            folded_call_spans: FxHashMap::default(),
            type_bindings: FxHashMap::default(),
            instantiations: FxHashMap::default(),
            comptime_calls: FxHashMap::default(),
            variadic_calls: FxHashMap::default(),
            soa_fields: FxHashMap::default(),
            value_bindings: FxHashMap::default(),
            comptime_param_names: FxHashSet::default(),
            poly_var_names: FxHashSet::default(),
            placeholder_arrays: FxHashSet::default(),
            any_calls: FxHashMap::default(),
            hir,
            file,
            resolve,
            interner,
            pool,
            imports,
            imported_hirs,
            resolving_in_module: None,
            sigs: FileSignatures::new(),
            mode,
            diags: Diagnostics::new(),
            types: TypeMap::new(),
            in_progress: Vec::new(),
            finished: FxHashSet::default(),
            body: None,
            locals: FxHashMap::default(),
            operator_calls: FxHashMap::default(),
            filled_calls: FxHashMap::default(),
        }
    }

    // -----------------------------------------------------------------------
    // Operator overloads (ADR-0048)
    // -----------------------------------------------------------------------

    /// Whether *any* overload is reachable from this file.
    ///
    /// The early exit that makes the feature free for a file that uses none: every operator
    /// expression asks this first, and for the overwhelmingly common file the answer is one
    /// boolean per import rather than a map lookup per operand pair.
    pub(crate) fn any_operators_in_scope(&self) -> bool {
        self.sigs.has_operators() || self.imports.iter().any(|(_, s)| s.has_operators())
    }

    /// The overload for `op` on this exact operand pair, and the file that declares it.
    ///
    /// Resolution order is **ADR-0014 §3's, unchanged**: this file first, then imports, and two
    /// *different* imports offering the same overload is E0211 at the use site. Reusing that rule
    /// rather than inventing one is the whole reason ADR-0048 §1 put an overload in the ordinary
    /// name map.
    pub(crate) fn find_operator(
        &mut self,
        op: jr_hir::BinOp,
        lhs: PoolId,
        rhs: PoolId,
        span: Span,
    ) -> Option<(jr_hir::ProcId, FileId)> {
        // A local declaration shadows an imported one silently, which is ADR-0014 §3's first
        // rule: adding an overload to a module can never break an importer that has its own.
        if let Some(proc) = self.sigs.operator(op, lhs, rhs) {
            return Some((proc, self.file));
        }

        let mut found: Option<(jr_hir::ProcId, FileId, &str)> = None;
        let mut ambiguous: Vec<&str> = Vec::new();
        for (name, sigs) in &self.imports {
            if let Some(proc) = sigs.operator(op, lhs, rhs) {
                match found {
                    None => found = Some((proc, sigs.file(), name)),
                    Some((_, _, first)) => {
                        if ambiguous.is_empty() {
                            ambiguous.push(first);
                        }
                        ambiguous.push(name);
                    }
                }
            }
        }

        if !ambiguous.is_empty() {
            let modules = ambiguous.join("` and `");
            self.diags.push(
                Diagnostic::error(
                    span,
                    "operator overload is provided by more than one module",
                )
                .with_code(E0211)
                .with_note(format!("`{modules}` each declare this overload"))
                .with_help("declare one in this file, which shadows both (ADR-0014 §3)"),
            );
            return None;
        }

        found.map(|(proc, file, _)| (proc, file))
    }

    /// The signatures of `file`, whether it is this one or an import.
    pub(crate) fn sigs_for_file(&self, file: FileId) -> Option<&FileSignatures> {
        if file == self.file {
            return Some(&self.sigs);
        }
        self.imports
            .iter()
            .map(|(_, s)| *s)
            .find(|s| s.file() == file)
    }

    // -----------------------------------------------------------------------
    // Arena access
    // -----------------------------------------------------------------------

    /// Returns the type reference `id` names in the arena `scope` selects.
    ///
    /// An index outside that arena yields [`TypeRef::Error`], which poisons
    /// quietly rather than reading a node from the wrong arena.
    pub(crate) fn type_ref(&self, scope: ExprScope, id: TypeRefId) -> TypeRef {
        let hir = self.hir;
        let arena = match scope {
            ExprScope::TopLevel => &hir.type_refs,
            ExprScope::Body(body) => &hir.body(body).type_refs,
        };
        arena.get(id.index()).cloned().unwrap_or(TypeRef::Error)
    }

    // -----------------------------------------------------------------------
    // Type resolution
    // -----------------------------------------------------------------------

    /// Resolves a syntactic type reference to an interned type.
    ///
    /// `span` is the span of the *declaration* the annotation belongs to, because
    /// a [`TypeRef`] carries no span of its own.
    pub(crate) fn resolve_type(&mut self, scope: ExprScope, id: TypeRefId, span: Span) -> PoolId {
        match self.type_ref(scope, id) {
            TypeRef::Error => PoolId::ERROR,
            TypeRef::Name(sym) => self.resolve_type_name(sym, span),
            // `Window.Event` (ADR-0179 §5) — looked up in one module's signatures rather than in the
            // flat merge of every bare import, so it cannot be ambiguous and reaches a module that
            // merged nothing.
            TypeRef::Qualified { module, name } => {
                self.resolve_qualified_type_name(module, name, span)
            }
            // `$T` (ADR-0081 §1). When `T` is already bound — this is an instantiation, or a later `$T`
            // for the same variable — resolve to the bound type. Otherwise the variable is being
            // introduced: without an instantiation there is no type yet, so it resolves to `ERROR`, which
            // is the "not concrete" state the signature phase records rather than a wrong answer. The bare
            // `T` case is handled in `resolve_type_name`, which consults the same map.
            TypeRef::Poly(sym) => self
                .type_bindings
                .get(&sym)
                .copied()
                .unwrap_or(PoolId::ERROR),
            TypeRef::Pointer(inner) => {
                let pointee = self.resolve_type(scope, inner, span);
                // `*<unknown>` is not a more useful type than `<unknown>`, and
                // keeping the poison flat means one comparison recognises it.
                if pointee == PoolId::ERROR {
                    PoolId::ERROR
                } else {
                    self.pool.pointer_to(pointee)
                }
            }
            TypeRef::Array {
                elem,
                len,
                len_name,
                len_span,
            } => {
                let element = self.resolve_type(scope, elem, span);
                // A literal length is already known; otherwise the length may still be a **name** that
                // resolves to a literal-valued constant (ADR-0070 §1), which needs a HIR lookup rather
                // than an evaluation — so it happens here without inverting ADR-0018 §3's phase order.
                let resolved_len = match len {
                    Some(n) => Some(n),
                    None => len_name.and_then(|name| self.constant_array_length(name)),
                };
                let Some(n) = resolved_len else {
                    // **A length naming a `$N` comptime parameter of a template is withheld, not
                    // refused** (ADR-0089 §2): the program is correct and the *template* simply has no
                    // value for `N` yet — each instantiation resolves its own. Length 0 is a placeholder
                    // for the template's own type only; the template is never lowered (`is_template`
                    // skips its body and its native declaration), so nothing runs against it. The same
                    // withholding shape as `jr-hir`'s E0201 inside a pending `#insert`.
                    if len_name.is_some_and(|name| self.comptime_param_names.contains(&name)) {
                        if element == PoolId::ERROR {
                            return PoolId::ERROR;
                        }
                        let placeholder = self.pool.array_of(element, 0);
                        // Remembered so every *length-dependent* check withholds on it — a literal index
                        // against `[0]s64` would otherwise be a false E0236 about correct code.
                        self.placeholder_arrays.insert(placeholder);
                        return placeholder;
                    }
                    // Lowering reached the length *token* and found it was neither a usable
                    // literal nor a name resolving to one, but says nothing (ADR-0039 §3a).
                    // This is where it is reported, because rejecting a type is a semantic
                    // judgement and because `type-errors/` files must lower cleanly.
                    self.array_length_not_literal(len_name.is_some(), len_span);
                    return PoolId::ERROR;
                };
                // Flat poison, for the same reason `*<unknown>` is flat above: one
                // comparison recognises it, and `[4]<unknown>` is no more useful than
                // `<unknown>`.
                if element == PoolId::ERROR {
                    PoolId::ERROR
                } else {
                    self.pool.array_of(element, n)
                }
            }
            // `#simd [N]T` — the lane count resolves exactly as an array length does, sharing
            // `constant_array_length` and the comptime-parameter withholding, and *then* the width
            // is checked (ADR-0148 §2). The order matters: a `$N` template must withhold before the
            // width test, because a template genuinely has no count yet and refusing it would reject
            // correct code.
            TypeRef::Vector {
                elem,
                lanes,
                lanes_name,
                lanes_span,
            } => {
                let element = self.resolve_type(scope, elem, span);
                let resolved = match lanes {
                    Some(n) => Some(n),
                    None => lanes_name.and_then(|name| self.constant_array_length(name)),
                };
                let Some(n) = resolved else {
                    if lanes_name.is_some_and(|name| self.comptime_param_names.contains(&name)) {
                        if element == PoolId::ERROR {
                            return PoolId::ERROR;
                        }
                        // A placeholder *vector*, not a placeholder array: the two are different
                        // types and a template's own type must be the shape it will instantiate to,
                        // or every use inside the template body would be checked against the wrong
                        // operator set.
                        let placeholder = self.pool.vector_of(element, 0);
                        self.placeholder_arrays.insert(placeholder);
                        return placeholder;
                    }
                    self.array_length_not_literal(lanes_name.is_some(), lanes_span);
                    return PoolId::ERROR;
                };
                if element == PoolId::ERROR {
                    return PoolId::ERROR;
                }
                if !self.check_vector_shape(element, n, span) {
                    return PoolId::ERROR;
                }
                self.pool.vector_of(element, n)
            }
            TypeRef::View { elem } => {
                let element = self.resolve_type(scope, elem, span);
                // Flat poison, as `*<unknown>` and `[4]<unknown>` both are: one comparison
                // recognises it.
                if element == PoolId::ERROR {
                    PoolId::ERROR
                } else {
                    self.pool.view_of(element)
                }
            }
            TypeRef::DynamicArray { elem } => {
                let element = self.resolve_type(scope, elem, span);
                if element == PoolId::ERROR {
                    PoolId::ERROR
                } else {
                    self.pool.dynamic_array_of(element)
                }
            }
            // `-> (s64, bool)` (ADR-0052 §1). Interned structurally, and normalised: a one-element
            // list becomes the element itself, so `-> (T)` and `-> T` are one type.
            //
            // Poison is flat here as it is for a view: one `PoolId::ERROR` comparison recognises a
            // results list with an unresolvable element, rather than a results type *containing*
            // poison that every consumer would have to look inside.
            TypeRef::Results(elems) => {
                let mut resolved = Vec::with_capacity(elems.len());
                let mut poisoned = false;
                for elem in elems {
                    let ty = self.resolve_type(scope, elem, span);
                    poisoned |= ty == PoolId::ERROR;
                    resolved.push(ty);
                }
                if poisoned {
                    PoolId::ERROR
                } else {
                    self.pool.results_type(resolved)
                }
            }
            // A procedure-pointer type resolves to the **same** `Item::ProcType` a declared procedure
            // has, so passing a procedure to a parameter of this type is an ordinary type match.
            //
            // **The convention comes from the type expression** (ADR-0175 §1). It used to be
            // `ContextKind::Jairs` always, on the grounds that "the type syntax carries no `#c_call`" —
            // which was true and made a `#c_call` procedure *unpassable*, since its `CCall` type could
            // never be named. ADR-0059 §5's refusal of a `#foreign` procedure value still falls out of
            // the type system, because a `#foreign` declaration is refused as a *value* before its type
            // is compared; what changed is that a **local** `#c_call` procedure now has a spellable type.
            TypeRef::Proc {
                params,
                ret,
                c_call,
            } => {
                let mut resolved = Vec::with_capacity(params.len());
                let mut poisoned = false;
                for param in params {
                    let ty = self.resolve_type(scope, param, span);
                    poisoned |= ty == PoolId::ERROR;
                    resolved.push(ty);
                }
                // A missing return arrow is `void`, exactly as a declared procedure's is
                // (`signature.rs`), and `void` has no spelling so there is no name to have lowered.
                let ret_ty = match ret {
                    Some(r) => self.resolve_type(scope, r, span),
                    None => PoolId::VOID,
                };
                poisoned |= ret_ty == PoolId::ERROR;
                if poisoned {
                    PoolId::ERROR
                } else {
                    let context = if c_call {
                        ContextKind::CCall
                    } else {
                        ContextKind::Jairs
                    };
                    self.pool.proc_type(resolved, ret_ty, context)
                }
            }
            TypeRef::Struct(sid) => {
                let ty = self.struct_type(sid);
                self.resolve_struct_body(sid, ty, span);
                ty
            }
            TypeRef::Union(sid) => {
                let ty = self.union_type(sid);
                // The *same* body resolution: a union's fields are a struct's fields and live
                // in the same side table (ADR-0045 §4, §5).
                self.resolve_struct_body(sid, ty, span);
                ty
            }
            TypeRef::Variant(sid) => {
                let ty = self.variant_type(sid);
                // The same body resolution again: a variant's *cases* are a field list, so they live
                // in the same side table (ADR-0068 §1). Only the layout and the read check differ.
                self.resolve_struct_body(sid, ty, span);
                ty
            }
            TypeRef::Enum(eid) => {
                let ty = self.enum_type(eid);
                self.resolve_enum_body(eid, span);
                ty
            }
            // `Box(s64)` — a parameterised type reference (ADR-0085 §3).
            TypeRef::Apply { name, args } => self.resolve_apply(scope, name, &args, span),
        }
    }

    /// Resolves a parameterised type reference `Box(s64)` to its interned instance (ADR-0085 §3).
    ///
    /// Looks the constructor name up to a `struct($T) { … }` declared in this file, resolves each
    /// argument to a type, binds the struct's type variables to them, interns the instance
    /// `StructType { decl, args }`, and resolves the field list *under those bindings* — so
    /// `Box(s64)` records `value: s64` and `Box(bool)` records `value: bool` from the one
    /// declaration. The bindings are saved and restored around the call, so a nested `Box(Box(s64))`
    /// and a sibling reference each see only their own.
    /// A parameterised instance from a **name and already-resolved arguments** (ADR-0119 §1).
    ///
    /// The expression-position counterpart of [`Ctx::resolve_apply`]: an intrinsic's type argument —
    /// `size_of(Slot(s64, s64))` — parses as a *call*, so its arguments are `ExprId`s the caller has already
    /// turned into types, not `TypeRefId`s in a type-reference tree. Everything after that point is identical,
    /// so the shared work lives in [`Ctx::instantiate_parameterised`] and both entry points call it.
    pub(crate) fn apply_resolved(&mut self, name: Symbol, args: Vec<PoolId>, span: Span) -> PoolId {
        let Some((decl_file, decl_hir, sid, poly_vars)) = self.parameterised_struct_anywhere(name)
        else {
            self.not_a_parameterised_struct(name, span);
            return PoolId::ERROR;
        };
        if args.len() != poly_vars.len() {
            self.wrong_type_argument_count(name, poly_vars.len(), args.len(), span);
            return PoolId::ERROR;
        }
        if args.contains(&PoolId::ERROR) {
            return PoolId::ERROR;
        }
        self.instantiate_parameterised(decl_file, decl_hir, sid, &poly_vars, args)
    }

    fn resolve_apply(
        &mut self,
        scope: ExprScope,
        name: Symbol,
        arg_refs: &[TypeRefId],
        span: Span,
    ) -> PoolId {
        // Resolve the arguments first, in the *caller's* bindings — an argument may itself be a bound
        // `$T` (`Pair(T)` inside a polymorphic body) or another `Apply`.
        let mut args = Vec::with_capacity(arg_refs.len());
        let mut poisoned = false;
        for &arg in arg_refs {
            let ty = self.resolve_type(scope, arg, span);
            poisoned |= ty == PoolId::ERROR;
            args.push(ty);
        }

        // The constructor must be a parameterised struct — **in this file or any imported one** (ADR-0117 §2).
        // A non-struct constructor, a bad arity, or an argument that failed to resolve each poison, reported
        // once, here, at the reference.
        let Some((decl_file, decl_hir, sid, poly_vars)) = self.parameterised_struct_anywhere(name)
        else {
            self.not_a_parameterised_struct(name, span);
            return PoolId::ERROR;
        };
        // **A type-argument reference marks its import used** (ADR-0117 §2). `ResolveMap` covers `Expr::Name`
        // only, and an ordinary imported type annotation is recorded by `resolve_type_name` — but `Box(s64)`
        // never reaches that function, because the constructor is looked up here. Without this, a file importing
        // a module *solely* for a parameterised struct reads as an unused import (E0231), and the quick fix
        // beside that warning would break the build — ADR-0031 §2's rule, and the same trap the ordinary
        // annotation path already had to close.
        if decl_file != self.file
            && let Some((module, _)) = self
                .imports
                .iter()
                .find(|(_, sigs)| sigs.file() == decl_file)
        {
            let module = *module;
            self.sigs.insert_type_name_import(name, module);
        }
        if args.len() != poly_vars.len() {
            self.wrong_type_argument_count(name, poly_vars.len(), args.len(), span);
            return PoolId::ERROR;
        }
        if poisoned {
            return PoolId::ERROR;
        }

        // **The declaring file's id**, not this one (ADR-0117 §2): a nominal type's identity is its declaration
        // site (ADR-0015 §1), so `Box(s64)` must be the *same* type in two importers rather than one per
        // importer — which is what makes a value of it pass between them.
        self.instantiate_parameterised(decl_file, decl_hir, sid, &poly_vars, args)
    }

    /// Interns a parameterised instance and resolves its fields, once per instance (ADR-0086 §3).
    ///
    /// Shared by the two entry points — a type-position `Apply` and an intrinsic's call-shaped type argument
    /// (ADR-0119 §1) — because everything from "the constructor and its arguments are known" onward is identical.
    /// Two copies would be two chances for the recursion guard or the binding save/restore to drift.
    fn instantiate_parameterised(
        &mut self,
        decl_file: FileId,
        decl_hir: &'a FileHir,
        sid: StructId,
        poly_vars: &[Symbol],
        args: Vec<PoolId>,
    ) -> PoolId {
        let decl = DeclId::new(decl_file, sid.as_u32());
        let instance = self.pool.struct_instance(decl, args.clone());

        // Resolve the field list under the argument bindings, once per instance. Guarded so a
        // recursive `List(s64)` — whose field mentions `List(s64)` again — does not re-enter and
        // loop: the instance's identity exists before its fields, exactly as ADR-0015 §1's fixpoint
        // for an ordinary recursive struct.
        if self.pool.fields_of(instance).is_none() {
            // Reserve the slot so the recursion guard sees "in progress" as a non-empty field list.
            self.pool.set_instance_fields(instance, Vec::new());
            let saved: Vec<(Symbol, Option<PoolId>)> = poly_vars
                .iter()
                .map(|&var| (var, self.type_bindings.get(&var).copied()))
                .collect();
            for (&var, &arg) in poly_vars.iter().zip(&args) {
                self.type_bindings.insert(var, arg);
            }
            let fields = self.resolve_instance_fields_in(decl_hir, sid);
            self.pool.set_instance_fields(instance, fields);
            for (var, prev) in saved {
                match prev {
                    Some(ty) => self.type_bindings.insert(var, ty),
                    None => self.type_bindings.remove(&var),
                };
            }
        }
        instance
    }

    /// The declaring file, `StructId` and type parameters of the parameterised struct `name` names — in **this
    /// file or any imported one** (ADR-0117 §2).
    ///
    /// `None` for a name that is nowhere, is not a struct, or has no type parameters — each of which means a
    /// `Name(args)` reference is malformed, reported by the caller.
    ///
    /// This file is searched **first**, which is ADR-0014 §3's resolution order unchanged: a local declaration
    /// shadows an imported one of the same name, and this must not be the one place that differs.
    fn parameterised_struct_anywhere(
        &self,
        name: Symbol,
    ) -> Option<(FileId, &'a FileHir, StructId, Vec<Symbol>)> {
        if let Some((sid, poly_vars)) = self.parameterised_struct(name) {
            return Some((self.file, self.hir, sid, poly_vars));
        }
        // An **imported** parameterised struct. Its fields are resolved from *its* HIR (ADR-0117 §1), because a
        // `TypeRef` is an index into the declaring file's arena — which is the whole reason the check phase now
        // receives these.
        //
        // `export_scope` rather than `scope`, so a `#scope_module` struct stays private (ADR-0054 §1): an
        // importer must not reach a name its module hides, and using the wrong scope here would be a hole in
        // that filter reachable only through a type argument.
        for &(file, hir) in &self.imported_hirs {
            let Some(item) = hir.export_scope().get(name) else {
                continue;
            };
            let ItemKind::Const {
                value: ConstValue::Struct(sid),
            } = hir.item(item).kind
            else {
                continue;
            };
            let poly_vars = hir.struct_def(sid).poly_vars.clone();
            if poly_vars.is_empty() {
                continue;
            }
            return Some((file, hir, sid, poly_vars));
        }
        None
    }

    /// The `StructId` and type parameters of `name` if it is a parameterised struct in this file.
    ///
    /// `None` for a name that is not declared here, is not a struct, or has no type parameters —
    /// each of which means a `Name(args)` reference is malformed, reported by the caller.
    fn parameterised_struct(&self, name: Symbol) -> Option<(StructId, Vec<Symbol>)> {
        let item = self.hir.scope.get(name)?;
        let ItemKind::Const {
            value: ConstValue::Struct(sid),
        } = self.hir.item(item).kind
        else {
            return None;
        };
        let poly_vars = self.hir.struct_def(sid).poly_vars.clone();
        if poly_vars.is_empty() {
            return None;
        }
        Some((sid, poly_vars))
    }

    /// Resolves the field list of struct `sid` under the currently-bound type variables.
    ///
    /// The same field loop as [`Ctx::resolve_struct_body`], but returning the fields rather than
    /// recording them under the `DeclId` — because a parameterised instance's fields key on the
    /// instance `PoolId`, not the declaration (ADR-0085 §2).
    /// Resolves struct `sid`'s fields **from `hir`**, under the currently-bound type variables (ADR-0117 §2).
    ///
    /// The declaring file's HIR is passed explicitly because a field's `TypeRef` is a `TypeRefId` into *its*
    /// arena — so resolving an imported parameterised struct's fields means reading that file's arena while this
    /// file's bindings are in force. `self.hir` is swapped for the duration and restored, which is the smallest
    /// correct move: `resolve_type` walks `self.hir.type_refs`, and giving it a second source would mean a second
    /// path through every type-resolution arm.
    ///
    /// A **name** inside those fields still resolves in the *declaring* file's scope — `Box(T) { value: Helper; }`
    /// means that file's `Helper` — which the swap gets right for free, and which is the only correct reading: a
    /// struct's fields cannot depend on who imported it.
    ///
    /// **`type_bindings` is narrowed to the instance's own arguments** for the duration, which is what makes the
    /// paragraph above true rather than nearly true. `resolve_type_name` consults the bindings *before* the
    /// declaring module's signatures (ADR-0081 §1's "a bound variable wins over everything"), and the caller
    /// saves and restores only the struct's own `poly_vars` — so any *other* binding in scope leaked in. A field
    /// whose type names something the declaring module declares, where that name collides with a type variable
    /// the **importer** has bound, resolved to the importer's type; `set_instance_fields` then cached it for
    /// every later user of that instance. Silent wrong type and wrong layout, with no diagnostic.
    ///
    /// The audit at `354d900` found this **latent rather than live** (`docs/assessment-2026-08-07.md` §4): the
    /// only way to make an instance resolve while a foreign binding is in scope is to give it a type argument
    /// that depends on one, and `Box(T)` for a bound `T` is E0212 — inference through a parameterised struct is
    /// deferred (ADR-0085 §5). So the invariant held by accident of an unrelated refusal, and would have broken
    /// the day that refusal lifted. It is cheaper to make it structural now than to rediscover it then.
    fn resolve_instance_fields_in(
        &mut self,
        hir: &'a FileHir,
        sid: StructId,
    ) -> Vec<jr_pool::Field> {
        let saved_hir = self.hir;
        let saved_file = self.file;
        let saved_module = self.resolving_in_module;
        // **Only the instance's own arguments are in scope.** The caller bound this struct's `poly_vars` just
        // before calling, so those are the bindings to keep; everything else belongs to whoever is *using* the
        // instance and must not reach its fields. Taken by `mem::take` and put back below, so the narrowing is
        // exact rather than a filter that has to know which names matter.
        //
        // Read from `hir`, the **declaring** file's, and not from `self.hir`: `sid` indexes the declaring file's
        // arena, so asking the importer's would index a different one — which panics outright when the importer
        // has fewer structs, and would silently read the wrong declaration when it has more.
        let vars: Vec<Symbol> = hir.struct_def(sid).poly_vars.to_vec();
        let saved_bindings = core::mem::take(&mut self.type_bindings);
        for var in vars {
            if let Some(&bound) = saved_bindings.get(&var) {
                self.type_bindings.insert(var, bound);
            }
        }
        self.hir = hir;
        // The `FileId` moves too, because a nominal type named in those fields must intern with the *declaring*
        // file's identity (ADR-0015 §1) — a `Point` field in an imported `Box` is that module's `Point`.
        if let Some(&(file, _)) = self
            .imported_hirs
            .iter()
            .find(|(_, h)| core::ptr::eq(*h, hir))
        {
            self.file = file;
        }
        // The declaring module's signatures, so a field naming one of *its* types resolves there (ADR-0117 §2).
        // Found by matching the HIR pointer against the imports, the same way the `FileId` is.
        if let Some(&(_, sigs)) = self
            .imports
            .iter()
            .find(|(_, sigs)| sigs.file() == self.file)
        {
            self.resolving_in_module = Some(sigs);
        }
        let resolved = self.resolve_instance_fields_inner(sid);
        self.hir = saved_hir;
        self.file = saved_file;
        self.resolving_in_module = saved_module;
        self.type_bindings = saved_bindings;
        resolved
    }

    /// The field loop itself, reading whatever `self.hir` currently is.
    fn resolve_instance_fields_inner(&mut self, sid: StructId) -> Vec<jr_pool::Field> {
        let fields = self.hir.struct_def(sid).fields.clone();
        let mut resolved = Vec::with_capacity(fields.len());
        for field in &fields {
            let field_ty = match field.ty {
                Some(id) => self.resolve_type(ExprScope::TopLevel, id, field.name_span),
                None => PoolId::ERROR,
            };
            // The layout attributes travel with the field so that `jr-pool`'s fold can apply
            // them (ADR-0144 §5). Read through the *same* helper the ordinary struct body uses.
            let (align, place) = self.field_placement(field);
            resolved.push(
                if field.using {
                    jr_pool::Field::embedded(field.name, field_ty)
                } else {
                    jr_pool::Field::new(field.name, field_ty)
                }
                .placed(align, place),
            );
        }
        resolved
    }

    /// Resolves `Alias.Name` against the one module the alias names (ADR-0179 §5).
    ///
    /// Deliberately **not** a path through [`Self::resolve_type_name`]: none of the earlier steps that
    /// function takes apply. A qualified name is never a builtin, never a bound `$T`, and never a
    /// file-level declaration — the alias says which scope to look in, and that is the whole lookup.
    /// It reaches the same `FileSignatures` the bare form does, so `Window.Event` and a bare `Event`
    /// from the same module intern to one `PoolId` rather than two.
    ///
    /// The refusal is E0212, sema's unknown-type code, rather than resolution's E0292: the two
    /// positions are answered by different crates — a type annotation is invisible to `ResolveMap`
    /// (see `jr-db`'s `imports` module docs) — so this is the same asymmetry the unused-import warning
    /// already lives with, not a second answer to one question.
    pub(crate) fn resolve_qualified_type_name(
        &mut self,
        module: Symbol,
        name: Symbol,
        span: Span,
    ) -> PoolId {
        let Some(path) = self.module_path_of_alias(module) else {
            // The alias names no import in this file. Either E0210 already reported that the module
            // could not be found, or the receiver is not an alias at all — in which case lowering
            // would not have produced a qualified type ref. Silent, so one mistake is one message.
            return PoolId::ERROR;
        };
        let Some(sigs) = self
            .imports
            .iter()
            .find(|(n, _)| *n == path.as_str())
            .map(|(_, sigs)| *sigs)
        else {
            return PoolId::ERROR;
        };
        let Some(entry) = sigs.lookup(name) else {
            let text = self.interner.resolve(name);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("no exported type `{text}` in module `{path}`"),
                )
                .with_code(E0212)
                .with_help("check the spelling, or the module's exports"),
            );
            return PoolId::ERROR;
        };
        match entry.type_value {
            Some(ty) => {
                // Recorded for the same reason the bare path records it (ADR-0031 §2): a module used
                // *only* for a type would otherwise read as unused, and the quick fix beside that
                // warning would break the build.
                self.sigs.insert_type_name_import(name, path.as_str());
                ty
            }
            None => {
                self.not_a_type(name, entry.kind, span);
                PoolId::ERROR
            }
        }
    }

    /// The module path an aliased `#import` binds to `alias` (ADR-0179 §5).
    ///
    /// Read from the file's own `#import` items, which is the only place the alias exists: the
    /// signature tables this context holds are keyed by module *path*, because that is what the
    /// module loader can answer.
    fn module_path_of_alias(&self, alias: Symbol) -> Option<String> {
        self.hir.items.iter().find_map(|item| match &item.kind {
            jr_hir::ItemKind::Import {
                path,
                alias: Some(bound),
                ..
            } if *bound == alias => Some(path.clone()),
            _ => None,
        })
    }

    /// Resolves a named type: builtins, then this file, then imports.
    ///
    /// The order is the same one name resolution uses for expressions
    /// (ADR-0014 §3): a file-level declaration silently shadows an imported name.
    pub(crate) fn resolve_type_name(&mut self, sym: Symbol, span: Span) -> PoolId {
        // A **bound polymorphic variable** wins over everything (ADR-0081 §1): inside a `$T` signature or
        // its instantiation, a bare `T` is a use of the variable, not a lookup of a type named `T`. Checked
        // first so a program cannot shadow a bound variable with a same-named type — within the signature,
        // `T` *is* the variable. Empty outside a polymorphic context, so this costs an ordinary program
        // one hash probe and changes nothing.
        if let Some(&bound) = self.type_bindings.get(&sym) {
            return bound;
        }
        let interner = self.interner;
        // Builtin type names are ordinary identifiers, not keywords
        // (`docs/spec/01-lexical.md`), so they are matched here by text rather
        // than recognised by the lexer.
        let text = interner.resolve(sym);
        match text {
            "bool" => return PoolId::BOOL,
            "string" => return PoolId::STRING,
            _ => {}
        }
        // The integer tower, from the one list of names (ADR-0037 §1). `s64` and `u8` keep
        // their pre-interned ids because the well-known prefix's indices are pinned by a test
        // and `PTR_U8` depends on them; every other width is interned on first use, which is
        // all `Pool::intern` needs since it dedupes structurally.
        // `float32`/`float64`, from `FloatKind`'s own list of names — the counterpart to
        // `IntKind::NAMES` and, like it, the one place the names are written down (ADR-0040
        // §2). No pre-interned `PoolId`: the well-known prefix is for types reached before
        // user code, and no float is (ADR-0037 §1).
        if let Some(kind) = jr_pool::FloatKind::from_name(text) {
            return self.pool.intern(Item::FloatType { bits: kind.bits });
        }

        if let Some(kind) = IntKind::from_name(text) {
            return match (kind.signed, kind.bits) {
                (true, 64) => PoolId::S64,
                (false, 8) => PoolId::U8,
                (signed, bits) => self.pool.intern(Item::IntType { signed, bits }),
            };
        }

        // **Inside an imported struct's fields, that module's signatures answer** (ADR-0117 §2). Checked before
        // the local scope, because `self.hir` is already the module's — so `hir.scope.get` would find the
        // module's item while `self.sigs.lookup` asked the *importer's* signatures about it, and the mismatch is
        // what made an imported `Wrapper($T)` whose field names its own `Helper` fail to resolve.
        if let Some(module_sigs) = self.resolving_in_module
            && let Some(entry) = module_sigs.lookup(sym)
            && let Some(ty) = entry.type_value
        {
            return ty;
        }

        if let Some(item) = self.hir.scope.get(sym) {
            // A struct's *identity* is registered before any field type is
            // resolved (ADR-0015 §1 makes identity the declaration site, not the
            // fields), so a struct that points at itself — or at a struct that
            // points back — resolves here without re-entering signature
            // computation and tripping the constant-cycle guard.
            if let Some(entry) = self.sigs.lookup(sym)
                && let Some(ty) = entry.type_value
            {
                return ty;
            }
            let Some(entry) = self.entry_for_item(item) else {
                return PoolId::ERROR;
            };
            return match entry.type_value {
                Some(ty) => ty,
                None => {
                    self.not_a_type(sym, entry.kind, span);
                    PoolId::ERROR
                }
            };
        }

        let providers: Vec<(&str, SigEntry)> = self
            .imports
            .iter()
            .filter_map(|(name, sigs)| sigs.lookup(sym).map(|entry| (*name, entry)))
            .collect();

        match providers.as_slice() {
            [] => {
                self.unknown_type(sym, span);
                PoolId::ERROR
            }
            [(module, entry)] => match entry.type_value {
                Some(ty) => {
                    // Recorded so that "is this import used" can see a type annotation at
                    // all. `ResolveMap` covers `Expr::Name` only, so without this an
                    // import used *solely* for a type — which
                    // `tests/corpus/imports/valid/001-import-directory-module.jr` is —
                    // reads as unused, and the quick fix beside the warning breaks the
                    // build (ADR-0031 §2).
                    self.sigs.insert_type_name_import(sym, module);
                    ty
                }
                None => {
                    self.not_a_type(sym, entry.kind, span);
                    PoolId::ERROR
                }
            },
            many => {
                let name = interner.resolve(sym);
                let modules: Vec<String> = many.iter().map(|(m, _)| format!("`{m}`")).collect();
                self.diags.push(
                    Diagnostic::error(
                        span,
                        format!(
                            "ambiguous type name `{name}`: provided by multiple imported modules: {}",
                            modules.join(", ")
                        ),
                    )
                    .with_code(E0211)
                    .with_help("declare the type in this file to shadow both, or rename one"),
                );
                PoolId::ERROR
            }
        }
    }

    /// Interns the nominal type of the struct declared at `sid`.
    ///
    /// Identity is the declaration site (ADR-0015 §1), so this is safe to call
    /// before the field list is known — which is what lets a struct hold a
    /// pointer to itself.
    pub(crate) fn struct_type(&mut self, sid: StructId) -> PoolId {
        let decl = DeclId::new(self.file, sid.as_u32());
        self.pool.struct_type(decl)
    }

    /// Interns the nominal type of the union declared at `sid` (ADR-0045 §4).
    ///
    /// Takes a [`StructId`] because unions share the struct arena — see `Struct::is_union` for
    /// why that is load-bearing rather than convenient.
    pub(crate) fn union_type(&mut self, sid: StructId) -> PoolId {
        let decl = DeclId::new(self.file, sid.as_u32());
        self.pool.union_type(decl)
    }

    /// The interned type of the variant declared at `sid` (ADR-0068 §1).
    ///
    /// Takes a [`StructId`] because all three aggregate forms share the struct arena — see
    /// `Struct::kind` for why that sharing is load-bearing.
    pub(crate) fn variant_type(&mut self, sid: StructId) -> PoolId {
        let decl = DeclId::new(self.file, sid.as_u32());
        self.pool.variant_type(decl)
    }

    /// The interned type of the enum declared at `eid`.
    pub(crate) fn enum_type(&mut self, eid: jr_hir::EnumId) -> PoolId {
        let decl = DeclId::new(self.file, eid.as_u32());
        let flags = self.hir.enum_def(eid).flags;
        self.pool.enum_type(decl, flags)
    }

    /// Resolves and records the member list of the enum declared at `eid` (ADR-0041 §3).
    ///
    /// Auto-numbering: the first member is 0, and each subsequent one is **one past the
    /// previous value** — so an explicit value makes later members continue from it rather
    /// than resetting to their index. That is C's rule and Jai's, and it is the part that is
    /// easy to get wrong.
    pub(crate) fn resolve_enum_body(&mut self, eid: jr_hir::EnumId, span: Span) {
        let members = self.hir.enum_def(eid).members.clone();
        let flags = self.hir.enum_def(eid).flags;
        let mut resolved = Vec::with_capacity(members.len());
        // A plain enum counts from 0; a flags enum from 1, because 0 is not a flag
        // (ADR-0043 §2). Zero is never *auto-created* for a flags enum — a program that wants
        // it writes `NONE :: 0;`.
        let mut next: i64 = if flags { 1 } else { 0 };
        for member in &members {
            let value = match member.value {
                // An explicit value must be an integer *literal*, for the same reason an
                // array length must be (ADR-0039 §3a): evaluating an arbitrary constant
                // expression needs the const-evaluator, which ADR-0018 §3 puts in `jr-db`,
                // downstream of this phase.
                Some(expr) => match self.enum_member_literal(expr, member.name_span) {
                    Some(value) => value,
                    None => next,
                },
                None => next,
            };
            resolved.push(jr_pool::EnumMember::new(member.name, value));
            next = if flags {
                next_power_of_two_above(value)
            } else {
                // Saturating, so a member at `i64::MAX` does not wrap round to a negative
                // value for the next one. A saturated duplicate is legal (ADR-0041 §3 allows
                // duplicates), where a wrapped one would be a silently wrong number.
                value.saturating_add(1)
            };
        }
        let decl = DeclId::new(self.file, eid.as_u32());
        self.pool.set_enum_members(decl, resolved);
        let _ = span;
    }

    /// Reads an enum member's explicit value, or reports why it is not usable.
    ///
    /// Accepts an integer literal, or a name for a constant whose initialiser is one — the same rule
    /// ADR-0070 gave an array length, generalised here (ADR-0129 §1). The asymmetry between the two was
    /// never a limit of the evaluator; only one of them had learnt the trick.
    fn enum_member_literal(&mut self, expr: jr_hir::ExprId, span: Span) -> Option<i64> {
        // Read straight from the top-level arena: `expr_of` lives on the checking half of
        // this context and a member value is resolved during *signatures*, which runs first.
        match self.hir.exprs.get(expr.index()) {
            Some(jr_hir::Expr::Literal(jr_hir::Literal::Int { value, .. }, _)) => {
                if let Ok(value) = i64::try_from(*value) {
                    return Some(value);
                }
                // A literal too wide for `i64` *is* a literal, so "must be a literal" would
                // misdescribe it. Take the named-value wording, which names the real fault.
                self.enum_member_not_constant(true, span);
            }
            // **A name resolves through the same helper an array length uses**, so "what counts as a
            // readable constant" has one definition rather than two (ADR-0129 §3).
            //
            // **`res` is deliberately ignored, and it is a trap.** It is `Res::Error` for *every* named
            // member value, including one that resolves perfectly well — resolution visits these
            // expressions and reports on them, but never writes the field back. Suppressing this
            // diagnostic on `Res::Error` therefore silences it for the valid case too, which was
            // measured rather than reasoned about: it made a working file compile to the wrong enum
            // values with no error at all. That is the exact failure shape AGENTS.md names — a
            // well-typed placeholder standing in for a missing answer — so the field is not consulted
            // and the scope lookup below is the only authority (ADR-0129 §3).
            Some(jr_hir::Expr::Name { name, .. }) => {
                let name = *name;
                match self
                    .named_constant_int(name)
                    .and_then(|v| i64::try_from(v).ok())
                {
                    Some(value) => return Some(value),
                    None => self.enum_member_not_constant(true, span),
                }
            }
            _ => self.enum_member_not_constant(false, span),
        }
        None
    }

    /// Reports an enum member value that is not a usable integer (ADR-0041 §3, ADR-0129 §3).
    ///
    /// Splits the message the way ADR-0070 §3 split E0233's: a reader who named something learns the
    /// *name* was not usable, and a reader who wrote arithmetic learns that evaluation is what is
    /// missing. Telling the first reader "must be an integer literal" would be false now that a
    /// literal-valued constant is accepted, and a reader given a rule that is no longer true cannot act
    /// on it.
    fn enum_member_not_constant(&mut self, was_a_name: bool, span: Span) {
        let diag = if was_a_name {
            Diagnostic::error(span, "this enum member's value is not a usable constant")
                .with_code(E0237)
                .with_note(
                    "a member's value may be an integer literal, or a name for a constant whose \
                     value is one — a computed constant, a `#run`, or one from another file needs \
                     the compile-time evaluator, which sema runs before (ADR-0018 §3)",
                )
                .with_help("give the constant a literal value, e.g. `NOT_FOUND :: 404;`")
        } else {
            Diagnostic::error(
                span,
                "an enum member's value must be a literal or a named constant",
            )
            .with_code(E0237)
            // **Not "arrives with full `#run` in wave W4".** W4 is complete and the evaluator
            // exists, so that note described a capability the compiler has had for waves
            // (ADR-0127 §2). The real constraint is *ordering*, exactly as E0233 states for an
            // array length: signatures are typed before const-eval runs (ADR-0018 §3), so no
            // computed value is available at this point.
            .with_note(
                "an enum's members are typed with its declaration, before the compile-time \
                 evaluator runs (ADR-0018 §3), so an arithmetic or `#run` value is not available \
                 here",
            )
            .with_help("write the value as a literal, e.g. `NOT_FOUND :: 404;`, or name a constant")
        };
        self.diags.push(diag);
    }

    /// Resolves a field's `#align` and `#place` operands, reporting what is unusable (ADR-0144).
    ///
    /// **One helper for both field-resolution sites** — an ordinary struct body and a
    /// parameterised instance's — because a placement read one way in one and another way in the
    /// other would be a *wrong offset* in exactly one of them, which no verifier catches. That is
    /// the same argument `named_constant_int` itself is shared under (ADR-0129 §3).
    ///
    /// The operands are read through `named_constant_int`, so `#align ALIGNMENT` works exactly as
    /// `[N]s64` with a named `N` does. This is that helper's **third** caller and it needed no
    /// change to serve one, which is the return on ADR-0129's generalisation.
    fn field_placement(&mut self, field: &jr_hir::Field) -> (Option<u32>, Option<u64>) {
        let align = field
            .align
            .and_then(|expr| self.layout_attr_value(expr, field.name_span, true))
            .and_then(|value| self.checked_align(value, field.name_span));
        let place = field
            .place
            .and_then(|expr| self.layout_attr_value(expr, field.name_span, false))
            .and_then(|value| match u64::try_from(value) {
                Ok(offset) => Some(offset),
                Err(_) => {
                    self.diags.push(
                        Diagnostic::error(field.name_span, "a `#place` offset cannot be negative")
                            .with_code(E0283)
                            .with_help(
                                "a field's offset is measured in bytes from the start of \
                                        its aggregate, so the smallest is `#place 0`",
                            ),
                    );
                    None
                }
            });
        (align, place)
    }

    /// The integer a layout attribute's operand denotes, or `None` with a diagnostic.
    ///
    /// `is_align` picks the code and the wording only: the *reading* is identical, which is why one
    /// function does both. A literal or a name that resolves to a literal-valued constant, through
    /// the same helper an array length uses — arithmetic is refused for ADR-0018 §3's ordering
    /// reason, since signatures are typed before the compile-time evaluator runs.
    fn layout_attr_value(&mut self, expr: ExprId, span: Span, is_align: bool) -> Option<i128> {
        let value = match self.hir.exprs.get(expr.index()) {
            Some(jr_hir::Expr::Literal(jr_hir::Literal::Int { value, .. }, _)) => Some(*value),
            Some(jr_hir::Expr::Name { name, .. }) => {
                let name = *name;
                self.named_constant_int(name)
            }
            _ => None,
        };
        if value.is_none() {
            let (code, what) = if is_align {
                (E0282, "an `#align`")
            } else {
                (E0283, "a `#place`")
            };
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("{what} value must be an integer literal or a named constant"),
                )
                .with_code(code)
                .with_note(
                    "a struct's fields are laid out with its declaration, before the compile-time \
                     evaluator runs (ADR-0018 §3), so an arithmetic or `#run` value is not \
                     available here",
                ),
            );
        }
        value
    }

    /// Checks that an `#align` value is a usable alignment (ADR-0144 §3).
    fn checked_align(&mut self, value: i128, span: Span) -> Option<u32> {
        let refuse = |ctx: &mut Self, message: &str, help: &str| {
            ctx.diags.push(
                Diagnostic::error(span, message)
                    .with_code(E0282)
                    .with_help(help.to_owned()),
            );
            None
        };
        let Ok(align) = u32::try_from(value) else {
            return refuse(
                self,
                "this `#align` value is not a usable alignment",
                "an alignment is a power of two between 1 and 4096",
            );
        };
        if align == 0 || !align.is_power_of_two() {
            return refuse(
                self,
                "an `#align` value must be a power of two",
                "write 1, 2, 4, 8, 16, 32 and so on up to 4096",
            );
        }
        if align > MAX_FIELD_ALIGN {
            return refuse(
                self,
                "this `#align` value is larger than the compiler can honour",
                "the maximum is 4096, one page: past that a stack slot cannot promise the \
                 alignment, and a request silently not met is worse than a refusal",
            );
        }
        Some(align)
    }

    /// The `#soa(N)` count of the struct declared at `sid`, if it has one (ADR-0147 §1).
    ///
    /// `None` for an ordinary struct *and* for one whose count is unusable, the second having
    /// reported E0284 — so a bad count degrades to an ordinary struct rather than to a struct of
    /// zero-length arrays, which would lay out as nothing and read as a wrong answer.
    ///
    /// Read through `named_constant_int`, its **fourth** caller (ADR-0070 wrote it, ADR-0129
    /// generalised it, ADR-0144 §2 was the third) and again needing no change to serve one.
    pub(crate) fn soa_count(&mut self, sid: StructId) -> Option<u64> {
        let expr = self.hir.struct_def(sid).soa?;
        let span = self.hir.struct_def(sid).span;
        let value = match self.hir.exprs.get(expr.index()) {
            Some(jr_hir::Expr::Literal(jr_hir::Literal::Int { value, .. }, _)) => Some(*value),
            Some(jr_hir::Expr::Name { name, .. }) => {
                let name = *name;
                self.named_constant_int(name)
            }
            _ => None,
        };
        match value.and_then(|v| u64::try_from(v).ok()) {
            // Zero is refused rather than accepted as an empty struct: `#soa(0)` is far more
            // likely a mistake than an intent, and a struct of zero-length arrays lays out as
            // nothing while every access is out of range.
            Some(count) if count > 0 => Some(count),
            _ => {
                self.diags.push(
                    Diagnostic::error(span, "this `#soa` count is not a usable array length")
                        .with_code(E0284)
                        .with_note(
                            "a struct's fields are laid out with its declaration, before the \
                             compile-time evaluator runs (ADR-0018 §3), so an arithmetic or `#run` \
                             value is not available here",
                        )
                        .with_help(
                            "write a positive integer literal, as in `#soa(64)`, or name a \
                             constant whose value is one",
                        ),
                );
                None
            }
        }
    }

    /// Resolves and records the field list of the struct declared at `sid`.
    pub(crate) fn resolve_struct_body(&mut self, sid: StructId, ty: PoolId, span: Span) {
        let hir = self.hir;
        let fields = hir.struct_def(sid).fields.clone();
        // **The `#soa` transformation happens here, before layout** (ADR-0147 §1): each field's
        // resolved type is wrapped in `[N]T`, so every consumer downstream — layout, field offsets,
        // `type_info`, the VM and both back ends — sees an ordinary struct of arrays and needed no
        // change at all. That is the same leverage ADR-0144 took from the layout fold, one level up.
        let soa = self.soa_count(sid);
        let mut resolved = Vec::with_capacity(fields.len());
        for field in &fields {
            let field_ty = match field.ty {
                Some(id) => self.resolve_type(ExprScope::TopLevel, id, field.name_span),
                None => PoolId::ERROR,
            };
            // `using` inside an `#soa` struct is refused (ADR-0147 §3): promotion is a *lookup*
            // feature, and under `#soa` the promoted names would have to mean "the array of that
            // subfield" — a second transformation with no spelling for the index.
            let field_ty = match soa {
                Some(count) if field.using => {
                    self.diags.push(
                        Diagnostic::error(
                            field.name_span,
                            "a `using` field is not allowed in an `#soa` struct",
                        )
                        .with_code(E0284)
                        .with_note(
                            "`using` promotes a field's own fields for lookup, and under `#soa` \
                             each of those would be an array — a second transformation with no \
                             spelling for the index",
                        )
                        .with_help("drop the `using`, or drop the `#soa`"),
                    );
                    let _ = count;
                    field_ty
                }
                Some(count) => self.pool.array_of(field_ty, count),
                None => field_ty,
            };
            // The `using` flag travels with the field so that *field lookup* can follow an
            // embedded base (ADR-0050 §4). It changes no offset: `field_offset` never reads it.
            // `#align` and `#place` (ADR-0144), read here and applied by `jr-pool`'s fold —
            // nothing else in the compiler computes an offset, which is why a layout feature is
            // one change (ADR-0018 §2).
            let (align, place) = self.field_placement(field);
            resolved.push(
                if field.using {
                    jr_pool::Field::embedded(field.name, field_ty)
                } else {
                    jr_pool::Field::new(field.name, field_ty)
                }
                .placed(align, place),
            );
        }
        let decl = DeclId::new(self.file, sid.as_u32());
        // Recorded beside the fields, because the two facts belong together: the fields are already
        // `[N]T` by now, and this records *why* rather than a fact anything recomputes (ADR-0147 §1).
        if let Some(count) = soa {
            self.pool.set_soa_count(decl, count);
        }
        self.pool.set_struct_fields(decl, resolved.clone());
        self.sigs.insert_struct_body(decl, resolved);
        // `span` is unused for well-formed input; keeping the parameter means a
        // future per-field diagnostic has somewhere to point without changing
        // every caller.
        let _ = (ty, span);
    }

    // -----------------------------------------------------------------------
    // The name environment
    // -----------------------------------------------------------------------

    /// Returns the signature of a file-level item.
    ///
    /// In [`Mode::Signatures`] this computes it on demand; in [`Mode::Check`] it
    /// is a lookup. `None` means the item has no name (a bare `#import` or
    /// `#run`) or its signature failed, and callers must poison rather than
    /// report.
    pub(crate) fn entry_for_item(&mut self, item: ItemId) -> Option<SigEntry> {
        match self.mode {
            Mode::Signatures => self.item_signature(item),
            Mode::Check => {
                let name = self.hir.item(item).name?;
                self.sigs.lookup(name)
            }
        }
    }

    /// Returns the signature of a name reached through an `#import`.
    ///
    /// `import` is the `#import` item in *this* file — that is what
    /// `Res::Imported` names — so the module has to be recovered from its path
    /// string before the name can be looked up.
    pub(crate) fn entry_for_import(&mut self, import: ItemId, sym: Symbol) -> Option<SigEntry> {
        let hir = self.hir;
        let ItemKind::Import { path, .. } = &hir.item(import).kind else {
            return None;
        };
        self.imports
            .iter()
            .find(|(name, _)| *name == path.as_str())
            .and_then(|(_, sigs)| sigs.lookup(sym))
    }

    // -----------------------------------------------------------------------
    // Type predicates and rendering
    // -----------------------------------------------------------------------

    /// Returns `(signed, bits)` if `ty` is an integer type.
    pub(crate) fn int_info(&self, ty: PoolId) -> Option<(bool, u16)> {
        match self.pool.item(ty) {
            Item::IntType { signed, bits } => Some((*signed, *bits)),
            _ => None,
        }
    }

    /// Returns the pointee if `ty` is a pointer.
    pub(crate) fn pointee(&self, ty: PoolId) -> Option<PoolId> {
        match self.pool.item(ty) {
            Item::PointerType(inner) => Some(*inner),
            _ => None,
        }
    }

    /// The integer a name denotes, when it names a constant whose initialiser is an integer literal
    /// (ADR-0070 §1, generalised by ADR-0129 §1).
    ///
    /// **No evaluation happens here**, which is the whole reason this is available a sub-wave before
    /// `[2 + 2]u8` is: the literal is already in the HIR, and this crate depends on neither `jr-db` nor
    /// `jr-vm` (ADR-0039 §3a's constraint, still honoured). A value that needs *computing* — arithmetic,
    /// a `#run`, or a constant in another file — answers `None` here and is refused by the caller.
    ///
    /// One level of indirection only: `B :: A` where `A :: 4` answers `None` rather than following the
    /// chain, because a chain needs a fixpoint and a cycle check, which is the evaluation machinery this
    /// deliberately avoids (ADR-0070 §4).
    ///
    /// Answers `i128` — the widest thing a `Literal::Int` can hold — and leaves the range check to the
    /// caller, because the two callers disagree about range: an array length is a `u64` and rejects a
    /// negative, while an enum member is an `i64` and accepts one (ADR-0129 §2). Returning the raw value
    /// is what lets both share this without either inheriting the other's bounds.
    fn named_constant_int(&self, name: Symbol) -> Option<i128> {
        // **A `$N` comptime parameter's baked value wins** (ADR-0089 §1), checked first for the reason
        // `resolve_type_name` checks `type_bindings` first: inside an instantiation, `N` *is* that
        // parameter, and a same-named file constant must not shadow it. Still no evaluation here — the
        // value was interned by the const-eval pre-pass and carried in on `FileHir::param_values`.
        if let Some(&value) = self.value_bindings.get(&name) {
            return match *self.pool.item(value) {
                Item::IntValue { ty, bits } => Some(match *self.pool.item(ty) {
                    Item::IntType {
                        signed,
                        bits: width,
                    } => IntKind {
                        signed,
                        bits: width,
                    }
                    .decode(bits),
                    _ => i128::from(bits as i64),
                }),
                // A non-integer comptime parameter is not an integer; refused by the caller.
                _ => None,
            };
        }
        let item = self.hir.scope.get(name)?;
        let jr_hir::ItemKind::Const {
            value: jr_hir::ConstValue::Expr(expr),
        } = &self.hir.items.get(item.index())?.kind
        else {
            return None;
        };
        let jr_hir::Expr::Literal(jr_hir::Literal::Int { value, .. }, _) =
            self.hir.exprs.get(expr.index())?
        else {
            return None;
        };
        Some(*value)
    }

    /// The length a name denotes, when it names a constant whose initialiser is an integer literal
    /// (ADR-0070 §1).
    ///
    /// A negative length, or one past `u64`, fails here exactly as a negative *literal* length does —
    /// the value takes the same path once known, so ADR-0039 §3's checks are unchanged.
    fn constant_array_length(&self, name: Symbol) -> Option<u64> {
        u64::try_from(self.named_constant_int(name)?).ok()
    }

    /// Reports an array length that is not a usable integer literal (ADR-0039 §3a).
    ///
    /// The message does not name the offending text: a `TypeRef` carries no way back to
    /// the source, and the span already points at it. Naming the *reason* is what matters,
    /// because "write a literal" is not obvious advice unless you know why.
    /// The bytes a vector must total, which is one machine register (ADR-0148 §2).
    ///
    /// A constant rather than a literal at the two sites that need it, because the number is a *fact
    /// about the target* and the day a back end carries a 256-bit vector this is the one place that
    /// learns it.
    const VECTOR_BYTES: u64 = 16;

    /// Whether `element` and `lanes` name one of the six legal vector shapes (ADR-0148 §2).
    ///
    /// Reports E0285 and answers `false` when they do not. **Two separate refusals**, because the two
    /// mistakes are different and a reader can act on exactly one of them: a `#simd [4]s64` wrote a
    /// legal element and the wrong count, while a `#simd [4]string` wrote something a register cannot
    /// hold at all.
    fn check_vector_shape(&mut self, element: PoolId, lanes: u64, span: Span) -> bool {
        // A numeric scalar only: an integer or a float. Not a `bool` (one byte, but comparisons are
        // its only arithmetic and a mask is ADR-0148 §5's deferred wave), not a pointer (a vector of
        // addresses is a real thing and a much larger one — gather/scatter), and not an aggregate.
        let numeric = matches!(
            self.pool.item(element),
            Item::IntType { .. } | Item::FloatType { .. }
        );
        if !numeric {
            let text = self.describe(element);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("`#simd` needs a numeric element type, and `{text}` is not one"),
                )
                .with_code(E0285)
                .with_note(
                    "a vector lane is an integer or a float: a pointer, a `bool`, a `string` or an \
                     aggregate has no vector arithmetic",
                )
                .with_help("use an integer or float element, or a plain array for storage"),
            );
            return false;
        }

        // The element's own layout, from the pool — never `size_of` re-derived here (ADR-0018 §2).
        // `LP64` explicitly, matching every other `layout_of` call in this crate: a vector's width
        // is the *element's* width times the lanes, and no numeric scalar's size differs by target.
        let Ok(layout) = jr_pool::layout_of(self.pool, jr_pool::TargetLayout::LP64, element) else {
            // Unreachable for a numeric scalar, whose layout is its width. Refusing rather than
            // assuming keeps this from becoming a place that invents a size.
            return false;
        };
        let total = layout.size.saturating_mul(lanes);
        if total != Self::VECTOR_BYTES {
            let text = self.describe(element);
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!(
                        "`#simd` needs a vector exactly {} bytes wide, and `[{lanes}]{text}` is {total}",
                        Self::VECTOR_BYTES
                    ),
                )
                .with_code(E0285)
                // The six shapes rather than the rule, because the rule is the *reason* and these are
                // the answer — a reader who wrote `[4]s64` wants to be told `[2]s64` exists.
                .with_note(
                    "a vector is one machine register: 16×s8, 8×s16, 4×s32, 2×s64, 4×float32, or \
                     2×float64 (and the unsigned integer forms)",
                )
                .with_help(
                    "use one of those shapes, or a plain array if you want storage rather than \
                     arithmetic",
                ),
            );
            return false;
        }
        true
    }

    fn array_length_not_literal(&mut self, was_a_name: bool, span: Span) {
        // **The message names which side of the line the reader is on** (ADR-0070 §3). A
        // literal-valued constant is accepted now, so "must be an integer literal" would be simply
        // false — and a reader told a rule that is no longer true cannot act on it.
        let diag = if was_a_name {
            Diagnostic::error(span, "this array length is not a usable constant")
                .with_code(E0233)
                .with_note(
                    "a length may be an integer literal, or a name for a constant whose value is \
                     one — a computed constant, a `#run`, or one from another file needs the \
                     compile-time evaluator, which sema runs before",
                )
                .with_help("give the constant a literal value, e.g. `N :: 20;`")
        } else {
            Diagnostic::error(
                span,
                "an array length must be a literal or a named constant",
            )
            .with_code(E0233)
            .with_note(
                "an arithmetic or `#run` length needs the compile-time evaluator, which sema \
                     runs before (ADR-0018 §3)",
            )
            .with_help("write the length as a literal, e.g. `[20]u8`, or name a constant")
        };
        self.diags.push(diag);
    }

    /// The element type and length of `ty`, if it is an array.
    ///
    /// Shaped like [`Ctx::pointee`] above so that indexing and dereferencing read the same
    /// way at their call sites.
    pub(crate) fn array_parts(&self, ty: PoolId) -> Option<(PoolId, u64)> {
        match self.pool.item(ty) {
            Item::ArrayType { elem, len } => Some((*elem, *len)),
            _ => None,
        }
    }

    /// The element type and lane count of `ty`, if it is a vector (ADR-0148 §1).
    ///
    /// Deliberately **not** folded into [`Ctx::array_parts`], even though the two return the same
    /// pair and the layouts are identical: every caller of `array_parts` wants "can I index this and
    /// how long is it", and a vector answers both — while the callers that must *not* see a vector
    /// are the arithmetic ones, which is exactly the distinction a merged helper would erase.
    pub(crate) fn vector_parts(&self, ty: PoolId) -> Option<(PoolId, u64)> {
        match self.pool.item(ty) {
            Item::VectorType { elem, lanes } => Some((*elem, *lanes)),
            _ => None,
        }
    }

    /// Renders a type the way a diagnostic should spell it.
    pub(crate) fn describe(&self, ty: PoolId) -> String {
        match self.pool.item(ty) {
            Item::VoidType => "void".to_owned(),
            Item::BoolType => "bool".to_owned(),
            Item::IntType { signed, bits } => {
                format!("{}{bits}", if *signed { 's' } else { 'u' })
            }
            Item::StringType => "string".to_owned(),
            Item::TypeType => "type".to_owned(),
            // Never spelled as a real type: poison exists so that one error does
            // not become five, and naming it would invite the user to look for a
            // type called `<unknown>`.
            Item::ErrorType => "<unknown>".to_owned(),
            Item::ForeignLibraryType => "foreign library".to_owned(),
            Item::FloatType { bits } => format!("float{bits}"),
            Item::PointerType(inner) => format!("*{}", self.describe(*inner)),
            Item::ArrayType { elem, len } => format!("[{len}]{}", self.describe(*elem)),
            // Spelled the way it is written, `#simd` included, because the whole point of the type
            // is that it is *not* the array with the same bytes: a message saying `[4]s32` when the
            // program said `#simd [4]s32` would name a type the program does not have.
            Item::VectorType { elem, lanes } => {
                format!("#simd [{lanes}]{}", self.describe(*elem))
            }
            Item::ViewType { elem } => format!("[]{}", self.describe(*elem)),
            Item::DynamicArrayType { elem } => format!("[..]{}", self.describe(*elem)),
            // Spelled the way the source spells it (ADR-0052 §1), so an arity diagnostic can say
            // "`(s64, bool)` returns 2 values" rather than naming an internal type nobody wrote.
            Item::ContextType => "Context".to_owned(),
            Item::ResultsType { elems } => {
                let parts: Vec<String> = elems.iter().map(|ty| self.describe(*ty)).collect();
                format!("({})", parts.join(", "))
            }
            Item::StructType { .. } => self
                .type_name(ty)
                .map_or_else(|| "struct".to_owned(), str::to_owned),
            Item::UnionType { .. } => self
                .type_name(ty)
                .map_or_else(|| "union".to_owned(), str::to_owned),
            Item::VariantType { .. } => self
                .type_name(ty)
                .map_or_else(|| "variant".to_owned(), str::to_owned),
            // The declared name, from the same source the struct case reads, so a hover and
            // a diagnostic cannot disagree about what a nominal type is called.
            Item::EnumType { .. } => self
                .type_name(ty)
                .map_or_else(|| "enum".to_owned(), str::to_owned),
            // **The convention is rendered** (ADR-0175 §3), because without it two procedure types that
            // differ *only* in it print identically — and the mismatch between them read
            // "expected `(s64) -> s64`, found `(s64) -> s64`", which tells a reader nothing and looks
            // like a compiler bug. ADR-0001 made the two different types; this makes them different
            // *words*.
            Item::ProcType {
                params,
                ret,
                context,
                ..
            } => {
                let rendered: Vec<String> = params.iter().map(|p| self.describe(*p)).collect();
                let convention = if *context == jr_pool::ContextKind::CCall {
                    " #c_call"
                } else {
                    ""
                };
                format!(
                    "({}) -> {}{}",
                    rendered.join(", "),
                    self.describe(*ret),
                    convention
                )
            }
            // A value is never what a "type" diagnostic means to name, but
            // `describe` must be total, so fall through to the value's type.
            Item::VoidValue
            | Item::BoolValue(_)
            | Item::IntValue { .. }
            | Item::FloatValue { .. }
            | Item::StaticArray { .. }
            | Item::StrValue(_)
            | Item::TypeValue(_)
            | Item::ProcValue { .. }
            | Item::ForeignLibraryValue(_)
            // An aggregate constant falls through like every other value: a diagnostic naming a "type"
            // wants `Point`, not a rendering of the constant's contents (ADR-0074 §1).
            | Item::AggregateValue { .. } => self.describe(self.pool.type_of(ty)),
        }
    }

    /// Returns the source-level name of a nominal type, if one is known.
    fn type_name(&self, ty: PoolId) -> Option<&str> {
        self.sigs
            .type_name(ty)
            .or_else(|| self.imports.iter().find_map(|(_, s)| s.type_name(ty)))
    }

    // -----------------------------------------------------------------------
    // Diagnostics
    // -----------------------------------------------------------------------

    /// Requires `actual` where `expected` was wanted, reporting a mismatch.
    ///
    /// Returns the type the caller should carry on with: the expected type on a
    /// mismatch, so that one wrong expression does not produce an error at every
    /// enclosing node.
    ///
    /// Poison propagates silently in both directions. This is not politeness —
    /// `file_diagnostics` does not gate later phases on earlier ones, so without
    /// it every parse error would arrive here as a second, invented type error.
    pub(crate) fn expect(
        &mut self,
        expected: Option<PoolId>,
        actual: PoolId,
        span: Span,
    ) -> PoolId {
        let Some(want) = expected else { return actual };
        if want == PoolId::ERROR {
            return actual;
        }
        if actual == PoolId::ERROR {
            return PoolId::ERROR;
        }
        if want == actual {
            return actual;
        }
        let (want_text, actual_text) = (self.describe(want), self.describe(actual));
        // An array where a view was wanted is the one mismatch with a *specific* fix, and
        // ADR-0044 §2 committed to naming it: Jairs has no implicit array-to-view conversion
        // on purpose, so a reader who knows Jai will write `sum(buf)` and get this. A generic
        // "expected `[]s64`, found `[4]s64`" is accurate and gives them nothing — the
        // ADR-0043 lesson about a diagnostic that is true and useless.
        if let (Item::ViewType { elem: want_elem }, Item::ArrayType { elem: got_elem, .. }) =
            (self.pool.item(want), self.pool.item(actual))
            && want_elem == got_elem
        {
            self.diags.push(
                Diagnostic::error(
                    span,
                    format!("mismatched types: expected `{want_text}`, found `{actual_text}`"),
                )
                .with_code(E0240)
                .with_note("Jairs has no implicit conversion from an array to a view")
                .with_help("write `[]` to make a view of it, e.g. `buf[]`"),
            );
            return want;
        }
        self.diags.push(
            Diagnostic::error(
                span,
                format!("mismatched types: expected `{want_text}`, found `{actual_text}`"),
            )
            .with_code(E0214),
        );
        want
    }

    /// Reports a type annotation that names nothing.
    fn unknown_type(&mut self, sym: Symbol, span: Span) {
        let interner = self.interner;
        let name = interner.resolve(sym);
        // `void` is the one name reaching here that denotes a type which genuinely
        // **exists**: it is `PoolId::VOID`, it is storable (a zero-sized value still gets a
        // distinct address, `Memory`'s own docs), and `size_of(void)` folds to 0 (ADR-0106).
        //
        // So the generic "unknown type name" is false for it, and the note used to go
        // further and say "`void` is not a type name in Jairs" — which `size_of(void)`
        // contradicts outright, while `size_of` refuses a genuinely unresolvable name with
        // E0261. Two diagnostics disagreeing about whether a type exists is worse than
        // either being terse. What is actually true is narrower: the type has **no spelling
        // in type position**, so that is all this says.
        if name == "void" {
            self.diags.push(
                Diagnostic::error(span, "`void` cannot be used in type position")
                    .with_code(E0212)
                    .with_note(
                        "`void` is a real type and `size_of(void)` is 0, but it has no \
                         spelling in a type annotation",
                    )
                    .with_help(
                        "a procedure that returns nothing omits the `->` entirely; there is \
                         no `x: void` and no `*void`",
                    ),
            );
            return;
        }
        let mut diag =
            Diagnostic::error(span, format!("unknown type name `{name}`")).with_code(E0212);
        {
            // Built from `IntKind::NAMES` rather than written out, so the note cannot fall
            // behind the tower it describes (ADR-0037 §1).
            let ints = IntKind::NAMES
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ");
            diag = diag.with_note(format!("the builtin types are {ints}, `bool` and `string`"));
            // Only a name that *denotes* a type is a candidate: suggesting a procedure in
            // type position would trade E0212 for E0213, which is not help (ADR-0031 §1).
            if let Some(near) = self.nearest_type_name(name) {
                diag = diag.with_help(format!("did you mean `{near}`?"));
            }
        }
        self.diags.push(diag);
    }

    /// The nearest type name to one that does not exist.
    ///
    /// Searched in the same order resolution uses (ADR-0014 §3) — builtins, this file,
    /// then imports — so that where two are equally near, the one resolution would have
    /// picked is the one suggested.
    fn nearest_type_name(&self, wanted: &str) -> Option<String> {
        let mut candidates: Vec<String> = IntKind::NAMES
            .iter()
            .map(|name| (*name).to_owned())
            .chain([String::from("bool"), String::from("string")])
            .collect();
        let is_type = |entry: &SigEntry| entry.type_value.is_some();
        for sym in self.sigs.declared_names() {
            if self.sigs.lookup(sym).as_ref().is_some_and(is_type) {
                candidates.push(self.interner.resolve(sym).to_owned());
            }
        }
        for (_, sigs) in &self.imports {
            for sym in sigs.declared_names() {
                if sigs.lookup(sym).as_ref().is_some_and(is_type) {
                    candidates.push(self.interner.resolve(sym).to_owned());
                }
            }
        }
        crate::suggest::nearest(wanted, candidates.iter().map(String::as_str))
            .map(ToOwned::to_owned)
    }

    /// Reports a type annotation that names something which is not a type.
    fn not_a_type(&mut self, sym: Symbol, kind: SigKind, span: Span) {
        let interner = self.interner;
        let name = interner.resolve(sym);
        let what = match kind {
            SigKind::Const => "a constant",
            SigKind::Var => "a variable",
            SigKind::Proc => "a procedure",
            // Named as an operator rather than a procedure, because an overload's name is the
            // synthetic `"operator+"` that no user wrote (ADR-0048 §1).
            SigKind::Operator => "an operator overload",
            // Unreachable while `type_value` is `Some` for exactly these kinds,
            // but spelled out so that adding a kind is a compile error.
            SigKind::Struct | SigKind::Union | SigKind::Variant | SigKind::Enum => "a type",
        };
        self.diags.push(
            Diagnostic::error(span, format!("`{name}` is {what}, not a type")).with_code(E0213),
        );
    }

    /// Reports a `Name(args)` whose `Name` is not a parameterised struct (ADR-0085 §3, E0269).
    fn not_a_parameterised_struct(&mut self, name: Symbol, span: Span) {
        let text = self.interner.resolve(name);
        self.diags.push(
            Diagnostic::error(
                span,
                format!("`{text}` is not a parameterised struct, so it takes no type arguments"),
            )
            .with_code(E0269)
            .with_note(
                "type arguments apply to a `struct($T) { … }` — declared in this file or exported by an \
                 imported module",
            )
            .with_help(format!(
                "declare `{text}` as `struct($T) {{ … }}`, or drop the `(…)` if `{text}` is an \
                 ordinary type"
            )),
        );
    }

    /// Reports the wrong number of type arguments to a parameterised struct (ADR-0085 §3, E0270).
    fn wrong_type_argument_count(&mut self, name: Symbol, wanted: usize, got: usize, span: Span) {
        let text = self.interner.resolve(name);
        self.diags.push(
            Diagnostic::error(
                span,
                format!(
                    "`{text}` takes {wanted} type argument{}, but {got} {} given",
                    if wanted == 1 { "" } else { "s" },
                    if got == 1 { "was" } else { "were" },
                ),
            )
            .with_code(E0270),
        );
    }
}

/// The next power of two strictly above `value`, for `enum_flags` numbering (ADR-0043 §2).
///
/// Three things this must get right, and each is a way to get it wrong:
///
/// * **Strictly above.** After `A :: 1` the next flag is 2, not 1.
/// * **Above the previous *value*, not its index.** `enum_flags { A; B :: 8; C; }` gives
///   1, 8, 16 — `C` is 16 because it follows 8, not 4 because it is the third member.
/// * **Correct when the previous value is not a power of two.** An explicit `B :: 3` is a
///   legal named mask (ADR-0043 §6), and the flag after it is 4 — so this cannot simply
///   double.
///
/// A non-positive `value` yields 1, which is what makes an explicit `NONE :: 0;` leave the
/// sequence undisturbed: the member after it is the first flag.
fn next_power_of_two_above(value: i64) -> i64 {
    if value <= 0 {
        return 1;
    }
    // `checked_next_power_of_two` on the *successor*, so an exact power of two advances rather
    // than staying put. Saturating at the top bit rather than wrapping to a negative, for the
    // reason the sequential case saturates: a wrapped flag would be a silently wrong number.
    let above = (value as u64).saturating_add(1);
    match above.checked_next_power_of_two() {
        Some(next) if next <= i64::MAX as u64 => next as i64,
        _ => i64::MAX,
    }
}
