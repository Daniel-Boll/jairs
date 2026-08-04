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
    BodyId, ConstValue, ExprScope, FileHir, ItemId, ItemKind, LocalId, ResolveMap, StructId,
    TypeRef, TypeRefId,
};
use jr_pool::{ContextKind, DeclId, IntKind, Item, Pool, PoolId};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::code::{E0211, E0212, E0213, E0214, E0233, E0237, E0240, E0269, E0270};
use crate::map::TypeMap;
use crate::sigs::{FileSignatures, SigEntry, SigKind};

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
        mode: Mode,
    ) -> Self {
        Self {
            call_position: FxHashSet::default(),
            type_position: FxHashSet::default(),
            type_info_calls: FxHashMap::default(),
            type_bindings: FxHashMap::default(),
            instantiations: FxHashMap::default(),
            any_calls: FxHashMap::default(),
            hir,
            file,
            resolve,
            interner,
            pool,
            imports,
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
            // `(T, T) -> T` (ADR-0059 §3). Interned to the **same** `Item::ProcType` a declared
            // procedure gets, so `add`'s type and a `fn: (s64, s64) -> s64` parameter's type are one
            // entry and passing the procedure is an ordinary type match. `ContextKind::Jairs`
            // always: the type syntax carries no `#c_call`, so a `#foreign` procedure's `CCall` type
            // is a *different* interned type — which is what makes ADR-0059 §5's refusal fall out of
            // the type system rather than needing a separate check.
            TypeRef::Proc { params, ret } => {
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
                    self.pool.proc_type(resolved, ret_ty, ContextKind::Jairs)
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

        // The constructor must be a parameterised struct declared in this file. A cross-file or
        // non-struct constructor, a bad arity, or an argument that failed to resolve, each poison —
        // reported once, here, at the reference.
        let Some((sid, poly_vars)) = self.parameterised_struct(name) else {
            self.not_a_parameterised_struct(name, span);
            return PoolId::ERROR;
        };
        if args.len() != poly_vars.len() {
            self.wrong_type_argument_count(name, poly_vars.len(), args.len(), span);
            return PoolId::ERROR;
        }
        if poisoned {
            return PoolId::ERROR;
        }

        let decl = DeclId::new(self.file, sid.as_u32());
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
            let fields = self.resolve_instance_fields(sid);
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
    fn resolve_instance_fields(&mut self, sid: StructId) -> Vec<jr_pool::Field> {
        let fields = self.hir.struct_def(sid).fields.clone();
        let mut resolved = Vec::with_capacity(fields.len());
        for field in &fields {
            let field_ty = match field.ty {
                Some(id) => self.resolve_type(ExprScope::TopLevel, id, field.name_span),
                None => PoolId::ERROR,
            };
            resolved.push(if field.using {
                jr_pool::Field::embedded(field.name, field_ty)
            } else {
                jr_pool::Field::new(field.name, field_ty)
            });
        }
        resolved
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
    fn enum_member_literal(&mut self, expr: jr_hir::ExprId, span: Span) -> Option<i64> {
        // Read straight from the top-level arena: `expr_of` lives on the checking half of
        // this context and a member value is resolved during *signatures*, which runs first.
        if let Some(jr_hir::Expr::Literal(jr_hir::Literal::Int { value, .. }, _)) =
            self.hir.exprs.get(expr.index())
            && let Ok(value) = i64::try_from(*value)
        {
            return Some(value);
        }
        self.diags.push(
            Diagnostic::error(span, "an enum member's value must be an integer literal")
                .with_code(E0237)
                .with_note(
                    "a computed value needs the compile-time evaluator, which arrives with \
                     full `#run` in wave W4",
                )
                .with_help("write the value as a literal, e.g. `NOT_FOUND :: 404;`"),
        );
        None
    }

    /// Resolves and records the field list of the struct declared at `sid`.
    pub(crate) fn resolve_struct_body(&mut self, sid: StructId, ty: PoolId, span: Span) {
        let hir = self.hir;
        let fields = hir.struct_def(sid).fields.clone();
        let mut resolved = Vec::with_capacity(fields.len());
        for field in &fields {
            let field_ty = match field.ty {
                Some(id) => self.resolve_type(ExprScope::TopLevel, id, field.name_span),
                None => PoolId::ERROR,
            };
            // The `using` flag travels with the field so that *field lookup* can follow an
            // embedded base (ADR-0050 §4). It changes no offset: `field_offset` never reads it.
            resolved.push(if field.using {
                jr_pool::Field::embedded(field.name, field_ty)
            } else {
                jr_pool::Field::new(field.name, field_ty)
            });
        }
        let decl = DeclId::new(self.file, sid.as_u32());
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

    /// The length a name denotes, when it names a constant whose initialiser is an integer literal
    /// (ADR-0070 §1).
    ///
    /// **No evaluation happens here**, which is the whole reason this is available a sub-wave before
    /// `[2 + 2]u8` is: the literal is already in the HIR, and this crate depends on neither `jr-db` nor
    /// `jr-vm` (ADR-0039 §3a's constraint, still honoured). A length that needs a *value* — arithmetic, a
    /// `#run`, or a constant in another file — answers `None` here and is refused.
    ///
    /// One level of indirection only: `B :: A` where `A :: 4` answers `None` rather than following the
    /// chain, because a chain needs a fixpoint and a cycle check, which is the evaluation machinery this
    /// deliberately avoids (ADR-0070 §4).
    fn constant_array_length(&self, name: Symbol) -> Option<u64> {
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
        // A negative length, or one past `u64`, fails here exactly as a negative *literal* length does —
        // the value takes the same path once known, so ADR-0039 §3's checks are unchanged.
        u64::try_from(*value).ok()
    }

    /// Reports an array length that is not a usable integer literal (ADR-0039 §3a).
    ///
    /// The message does not name the offending text: a `TypeRef` carries no way back to
    /// the source, and the span already points at it. Naming the *reason* is what matters,
    /// because "write a literal" is not obvious advice unless you know why.
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
            Item::ViewType { elem } => format!("[]{}", self.describe(*elem)),
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
            Item::ProcType { params, ret, .. } => {
                let rendered: Vec<String> = params.iter().map(|p| self.describe(*p)).collect();
                format!("({}) -> {}", rendered.join(", "), self.describe(*ret))
            }
            // A value is never what a "type" diagnostic means to name, but
            // `describe` must be total, so fall through to the value's type.
            Item::VoidValue
            | Item::BoolValue(_)
            | Item::IntValue { .. }
            | Item::FloatValue { .. }
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
        let mut diag =
            Diagnostic::error(span, format!("unknown type name `{name}`")).with_code(E0212);
        // `void` is a real type (ADR-0015 §3) that has no spelling, so the
        // obvious guess deserves the obvious answer.
        if name == "void" {
            diag = diag
                .with_note("`void` is not a type name in Jairs")
                .with_help("a procedure that returns nothing omits the `->` entirely");
        } else {
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
                "type arguments apply to a `struct($T) { … }` declared in this file; a \
                 parameterised struct imported from another module is not yet supported",
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
