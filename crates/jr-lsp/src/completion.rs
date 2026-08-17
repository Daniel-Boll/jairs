//! Completion: what can be written at the cursor.
//!
//! # Why this is in the same wave as the hover card
//!
//! [ADR-0028](../../../docs/adr/0028-hover-and-completion.md) §1 and §5. A completion
//! item's `detail` is a signature, and a signature rendered by a second implementation
//! is a signature that drifts from the hover card's. Both come from
//! [`crate::render::Decl`].
//!
//! # Why the field list derefs pointers and knows about `string`
//!
//! Because `jr-sema`'s `check_field` does. It loops `pointee` until the type is not a
//! pointer, and it answers `data` and `count` for `string` — pseudo-fields, because
//! ADR-0004 fixes the layout as `{data: *u8, count: s64}` while ADR-0015 §2 keeps
//! `string` from *being* that struct. A completion list that offered a field the checker
//! then rejected, or hid one it accepts, would be worse than no list: it would be a
//! second, disagreeing model of field access. This module reads the same pool.
//!
//! # What is deliberately approximate
//!
//! **Locals are offered if they are declared earlier in the body**, rather than by block
//! scope. `jr-hir`'s `Body` holds a flat `locals` arena and the block structure lives in
//! statements, so exact scoping means walking `Stmt::Block` to the cursor. The
//! approximation over-offers: a local from a sibling block that has closed. It never
//! under-offers, which is the direction that makes a completion list feel broken. Stated
//! here rather than left to be discovered.

use std::sync::Arc;

use jr_db::{Db, ModuleSearchPaths, SourceFile};
use jr_hir::{FileHir, ItemKind};
use jr_pool::{Item, PoolId};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, Documentation,
    InsertTextFormat, MarkupContent, MarkupKind,
};

use crate::locate::locate;
use crate::position::{Encoding, Positions};
use crate::render::{Decl, container_of, type_name};

/// The keywords the parser accepts today.
///
/// Deliberately not every keyword `SyntaxKind` has: `enum`, `for`, `cast` and the rest
/// are lexed but refused with a "arrives in wave Wn" diagnostic, so completing them
/// would be offering the user an error.
const KEYWORDS: &[&str] = &[
    "struct", "if", "else", "while", "return", "break", "continue", "true", "false",
];

/// The builtin type names that are not integers.
///
/// The integer tower comes from `jr_pool::IntKind::NAMES` instead of being repeated here
/// (ADR-0037 §1) — this list used to say `["s64", "u8", "bool", "string"]`, which was one of
/// four places the tower was written down and would have fallen behind it.
///
/// They are ordinary identifiers rather than keywords (`docs/spec/01-lexical.md`), which is
/// why they are a separate list with a different completion kind.
const BUILTIN_TYPES: &[&str] = &["bool", "string"];

/// Every builtin type name, integers included.
fn builtin_type_names() -> impl Iterator<Item = &'static str> {
    // Both families from their own crate-owned lists (ADR-0037 §1, ADR-0040 §2), so a width
    // added to either appears here without this function changing.
    jr_pool::IntKind::NAMES
        .iter()
        .copied()
        .chain(jr_pool::FloatKind::NAMES.iter().copied())
        .chain(BUILTIN_TYPES.iter().copied())
}

/// The directives the parser interprets.
const DIRECTIVES: &[&str] = &["#import", "#run", "#foreign", "#system_library"];

/// What the text before the cursor says the user is asking for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Context {
    /// After a `.`: the fields of whatever precedes it.
    Field {
        /// Offset of the `.`, so the receiver can be located just before it.
        dot: usize,
    },
    /// After a `#`: a directive.
    Directive,
    /// Anything else: names in scope, items, imports, keywords, types.
    Name,
}

/// Everything that can be written at `position`.
///
/// Returns an empty list rather than `None` where there is nothing to offer, because an
/// empty completion list and a failed request are different things to a client.
#[must_use]
pub fn completion(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    encoding: Encoding,
    position: lsp_types::Position,
) -> Vec<CompletionItem> {
    let text = file.text(db);
    let index = jr_db::line_index(db, file);
    let positions = Positions::new(text.as_ref(), &index, encoding);
    let offset = usize::from(positions.offset(position));

    let mut items = match context_at(text.as_ref(), offset) {
        Context::Field { dot } => fields_at(db, file, search_paths, dot),
        Context::Directive => DIRECTIVES
            .iter()
            .map(|name| CompletionItem {
                label: (*name).to_owned(),
                kind: Some(CompletionItemKind::KEYWORD),
                ..CompletionItem::default()
            })
            .collect(),
        Context::Name => names_at(db, file, search_paths, offset),
    };

    // Stamped here rather than at each construction site, because forgetting it at one of
    // them would produce items that silently never resolve — a `completionItem/resolve`
    // request carries no document, so `data` is the only thing that says which file an
    // `ItemId` indexes.
    let path = file.path(db);
    for item in &mut items {
        if let Some(serde_json::Value::Object(data)) = item.data.as_mut() {
            data.insert(
                String::from("file"),
                serde_json::Value::String(path.as_ref().to_owned()),
            );
        }
    }
    items
}

/// Classifies the cursor from the text before it.
///
/// Purely textual, and that is on purpose: at the moment a completion is requested the
/// buffer usually does **not** parse — `p.` and `#im` are both syntax errors — so a
/// classification that depended on the CST would be least reliable exactly when it is
/// needed. This is the one place in the crate that reads source text rather than a
/// query's output.
fn context_at(text: &str, offset: usize) -> Context {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let word_start = before
        .char_indices()
        .rev()
        .find(|(_, c)| !is_ident_char(*c))
        .map_or(0, |(i, c)| i + c.len_utf8());

    let prefix = &before[..word_start];
    match prefix.chars().next_back() {
        Some('#') => Context::Directive,
        // `p.x` and `p.` alike: the dot is the last non-identifier character.
        Some('.') => Context::Field {
            dot: word_start - 1,
        },
        _ => Context::Name,
    }
}

/// Whether `c` can appear in the middle of an identifier.
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The fields of the receiver before a `.`.
///
/// Mirrors `jr_sema::check_field`: pointers are followed to their pointee, `string`
/// answers its two pseudo-fields, and a struct answers the pool's field list.
fn fields_at(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    dot: usize,
) -> Vec<CompletionItem> {
    let hir = jr_db::file_hir(db, file);
    // One byte before the dot is inside the receiver. `saturating_sub` because a file
    // starting with `.` is a syntax error the user is mid-way through typing.
    let Some(found) = locate(hir.as_ref(), (dot.saturating_sub(1) as u32).into()) else {
        return Vec::new();
    };
    let types = jr_db::checked(db, file, search_paths).types;
    let Some(mut ty) = types.expr_type(found.scope, found.expr) else {
        return Vec::new();
    };
    let sigs = jr_db::file_signatures(db, file, search_paths).signatures;
    let pool = db.pool().lock().unwrap_or_else(|e| e.into_inner());

    // Auto-deref, exactly as the checker does.
    while let Item::PointerType(pointee) = pool.item(ty) {
        ty = *pointee;
    }

    let field = |name: &str, ty: PoolId| CompletionItem {
        label: name.to_owned(),
        kind: Some(CompletionItemKind::FIELD),
        detail: Some(type_name(&pool, sigs.as_ref(), ty)),
        ..CompletionItem::default()
    };

    match pool.item(ty) {
        Item::StringType => vec![field("data", PoolId::PTR_U8), field("count", PoolId::S64)],
        // `[N]T` has exactly one pseudo-field, and deliberately no `.data` — offering one
        // would advertise a field `jr-sema` rejects (ADR-0039 §5).
        // A view offers the same one pseudo-field for the same reason, even though its
        // `.count` is a load rather than a constant — completion cares what you may write,
        // not how it lowers (ADR-0044 §4).
        Item::ArrayType { .. } | Item::ViewType { .. } => vec![field("count", PoolId::S64)],
        Item::StructType { .. } => pool
            .fields_of(ty)
            .unwrap_or(&[])
            .iter()
            .map(|f| field(db.interner().resolve(f.name), f.ty))
            .collect(),
        _ => Vec::new(),
    }
}

/// Names in scope at `offset`: locals, parameters, this file's items, imported items,
/// then keywords and builtin type names.
fn names_at(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    offset: usize,
) -> Vec<CompletionItem> {
    let hir = jr_db::file_hir(db, file);
    let sigs = jr_db::file_signatures(db, file, search_paths).signatures;
    let docs = jr_db::file_docs(db, file);
    let container = container_of(file.path(db).as_ref());
    let mut out = Vec::new();

    // Every query this function needs is called *before* the pool is locked. Locking it
    // first and then calling a query deadlocks: a query locks the pool itself and
    // `std::sync::Mutex` is not reentrant. The first version of this function did exactly
    // that and hung the test run with no output at all, which is what a deadlock looks
    // like from outside.
    let types = jr_db::checked(db, file, search_paths).types;
    let imported = imported_completions(db, hir.as_ref(), search_paths);

    {
        let pool = db.pool().lock().unwrap_or_else(|e| e.into_inner());

        // Locals and parameters of the body the cursor is in, first: the nearest
        // binding is the likeliest completion.
        if let Some(body_id) = body_at(hir.as_ref(), offset) {
            if let Some(body) = hir.bodies.get(body_id.index()) {
                for (i, local) in body.locals.iter().enumerate() {
                    if usize::from(local.span.range.start()) >= offset {
                        continue;
                    }
                    let ty = types
                        .local_type(body_id, jr_hir::LocalId::from_usize(i))
                        .map(|ty| type_name(&pool, sigs.as_ref(), ty));
                    out.push(CompletionItem {
                        label: db.interner().resolve(local.name).to_owned(),
                        kind: Some(CompletionItemKind::VARIABLE),
                        detail: ty,
                        ..CompletionItem::default()
                    });
                }
            }
            if let Some(proc) = owner_of(hir.as_ref(), body_id)
                && let Some(proc) = hir.procs.get(proc.index())
            {
                for param in &proc.params {
                    out.push(CompletionItem {
                        label: db.interner().resolve(param.name).to_owned(),
                        kind: Some(CompletionItemKind::VARIABLE),
                        ..CompletionItem::default()
                    });
                }
            }
        }

        let decl = Decl {
            hir: hir.as_ref(),
            sigs: sigs.as_ref(),
            docs: docs.as_ref(),
            // Not fetched: a completion keystroke should not evaluate the file's `#run`
            // expressions. The signature still shows the constant's type.
            consts: None,
            pool: &pool,
            interner: db.interner(),
            container: &container,
        };

        for (index, item) in hir.items.iter().enumerate() {
            if item.name.is_none() {
                continue;
            }
            let id = jr_hir::ItemId::from_usize(index);
            if let Some(completion) = item_completion(&decl, hir.as_ref(), id, &container) {
                out.push(completion);
            }
        }
    }

    out.extend(imported);

    out.extend(KEYWORDS.iter().map(|kw| CompletionItem {
        label: (*kw).to_owned(),
        kind: Some(CompletionItemKind::KEYWORD),
        ..CompletionItem::default()
    }));
    out.extend(builtin_type_names().map(|ty| CompletionItem {
        label: ty.to_owned(),
        kind: Some(CompletionItemKind::CLASS),
        ..CompletionItem::default()
    }));
    out
}

/// One item's completion, with a call snippet when it is a procedure.
///
/// The snippet uses the declaration's real parameter names, which is what ADR-0028 §5
/// traded against the risk of guessing intent: a procedure name in Jairs-0 is always
/// followed by a call, because there are no procedure values yet.
fn item_completion(
    decl: &Decl<'_>,
    hir: &FileHir,
    id: jr_hir::ItemId,
    container: &str,
) -> Option<CompletionItem> {
    let item = hir.items.get(id.index())?;
    let name = decl.interner.resolve(item.name?).to_owned();
    let signature = decl.signature(id)?;

    let (kind, insert, format) = match &item.kind {
        ItemKind::Const {
            value: jr_hir::ConstValue::Proc(proc),
        } => {
            let params = hir
                .procs
                .get(proc.index())
                .map(|p| p.params.as_slice())
                .unwrap_or_default();
            (
                CompletionItemKind::FUNCTION,
                Some(call_snippet(&name, params, decl.interner)),
                Some(InsertTextFormat::SNIPPET),
            )
        }
        // One arm for both aggregates: the protocol has no `UNION`, and `STRUCT` is the
        // nearest true thing — an aggregate with named fields.
        ItemKind::Const {
            value:
                jr_hir::ConstValue::Struct(_)
                | jr_hir::ConstValue::Union(_)
                | jr_hir::ConstValue::Variant(_),
        } => (CompletionItemKind::STRUCT, None, None),
        ItemKind::Const {
            value: jr_hir::ConstValue::Enum(_),
        } => (CompletionItemKind::ENUM, None, None),
        // An overload's name is the synthetic `"operator+"`, which a user cannot type — so it is
        // deliberately **not offered** as a completion. `OPERATOR` exists in the protocol and
        // suggesting the name would insert something that does not parse (ADR-0048 §1).
        ItemKind::Const {
            value: jr_hir::ConstValue::Operator(_, _),
        } => return None,
        ItemKind::Const {
            value: jr_hir::ConstValue::Expr(_),
        } => (CompletionItemKind::CONSTANT, None, None),
        ItemKind::Var { .. } => (CompletionItemKind::VARIABLE, None, None),
        ItemKind::Import { .. } | ItemKind::Run { .. } => return None,
    };

    Some(CompletionItem {
        label: name,
        kind: Some(kind),
        detail: Some(signature),
        label_details: Some(CompletionItemLabelDetails {
            detail: None,
            description: Some(container.to_owned()),
        }),
        insert_text: insert,
        insert_text_format: format,
        // Documentation is left for `completionItem/resolve`, so a large module does not
        // pay for prose the user may never look at (ADR-0028 §5).
        data: Some(serde_json::json!({ "item": id.index() })),
        ..CompletionItem::default()
    })
}

/// `add(${1:a}, ${2:b})$0`, or `f()$0` when there are no parameters.
fn call_snippet(name: &str, params: &[jr_hir::Param], interner: &jr_base::Interner) -> String {
    if params.is_empty() {
        return format!("{name}()$0");
    }
    let holes: Vec<String> = params
        .iter()
        .enumerate()
        .map(|(i, param)| format!("${{{}:{}}}", i + 1, interner.resolve(param.name)))
        .collect();
    format!("{name}({})$0", holes.join(", "))
}

/// Completions for every name each `#import` brings in.
///
/// Read from the imported file's own HIR and signatures, the same route
/// `goto_definition` takes (ADR-0014 §4).
fn imported_completions(
    db: &dyn Db,
    hir: &FileHir,
    search_paths: ModuleSearchPaths,
) -> Vec<CompletionItem> {
    let mut out = Vec::new();
    for item in &hir.items {
        let ItemKind::Import { path, .. } = &item.kind else {
            continue;
        };
        let lookup = jr_db::module_file(db, search_paths, Arc::from(path.as_str()));
        let Some(found) = lookup.found else { continue };
        let Some(module) = db.source_file_for_path(found.to_string_lossy().as_ref()) else {
            continue;
        };

        let other = jr_db::file_hir(db, module);
        let sigs = jr_db::file_signatures(db, module, search_paths).signatures;
        let docs = jr_db::file_docs(db, module);
        let container = container_of(found.to_string_lossy().as_ref());
        let pool = db.pool().lock().unwrap_or_else(|e| e.into_inner());
        let decl = Decl {
            hir: other.as_ref(),
            sigs: sigs.as_ref(),
            docs: docs.as_ref(),
            consts: None,
            pool: &pool,
            interner: db.interner(),
            container: &container,
        };

        for (index, other_item) in other.items.iter().enumerate() {
            if other_item.name.is_none() {
                continue;
            }
            let id = jr_hir::ItemId::from_usize(index);
            if let Some(mut completion) = item_completion(&decl, other.as_ref(), id, &container) {
                // The module is carried so that resolve can look the documentation up in
                // the file that actually declares it.
                completion.data = Some(serde_json::json!({
                    "item": id.index(),
                    "module": path.clone(),
                }));
                out.push(completion);
            }
        }
    }
    out
}

/// Fills in an item's documentation, on demand.
///
/// The other half of ADR-0028 §5's trade: the list is cheap because it carries no prose,
/// so the prose has to arrive here. Renders through [`Decl`] like everything else, which
/// is what stops a resolved item disagreeing with the one in the list.
#[must_use]
pub fn resolve_completion(
    db: &dyn Db,
    file: SourceFile,
    search_paths: ModuleSearchPaths,
    mut item: CompletionItem,
) -> CompletionItem {
    let Some(data) = item.data.clone() else {
        return item;
    };
    let Some(index) = data.get("item").and_then(serde_json::Value::as_u64) else {
        return item;
    };
    let id = jr_hir::ItemId::from_usize(index as usize);

    // A completion from an imported module resolves against that module's file, not this
    // one: the `ItemId` indexes the declaring file's items.
    let target = match data.get("module").and_then(serde_json::Value::as_str) {
        Some(path) => {
            let lookup = jr_db::module_file(db, search_paths, Arc::from(path));
            lookup
                .found
                .and_then(|found| db.source_file_for_path(found.to_string_lossy().as_ref()))
        }
        None => Some(file),
    };
    let Some(target) = target else { return item };

    let hir = jr_db::file_hir(db, target);
    let sigs = jr_db::file_signatures(db, target, search_paths).signatures;
    let docs = jr_db::file_docs(db, target);
    let container = container_of(target.path(db).as_ref());
    let pool = db.pool().lock().unwrap_or_else(|e| e.into_inner());

    let card = Decl {
        hir: hir.as_ref(),
        sigs: sigs.as_ref(),
        docs: docs.as_ref(),
        consts: None,
        pool: &pool,
        interner: db.interner(),
        container: &container,
    }
    .card(id);

    if let Some(card) = card {
        item.documentation = Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: card.to_markdown(),
        }));
    }
    item
}

/// Which body contains `offset`.
fn body_at(hir: &FileHir, offset: usize) -> Option<jr_hir::BodyId> {
    hir.procs
        .iter()
        .find(|proc| {
            let range = proc.span.range;
            let start = u32::from(range.start()) as usize;
            let end = u32::from(range.end()) as usize;
            proc.body.is_some() && start <= offset && offset <= end
        })
        .and_then(|proc| proc.body)
}

/// The procedure whose body is `body`.
fn owner_of(hir: &FileHir, body: jr_hir::BodyId) -> Option<jr_hir::ProcId> {
    hir.procs
        .iter()
        .position(|proc| proc.body == Some(body))
        .map(jr_hir::ProcId::from_usize)
}

/// The scope a completion request is in, for tests that want to assert it directly.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dot_asks_for_fields() {
        assert_eq!(context_at("p.", 2), Context::Field { dot: 1 });
        assert_eq!(context_at("p.x", 3), Context::Field { dot: 1 });
        assert_eq!(context_at("foo.bar", 7), Context::Field { dot: 3 });
    }

    #[test]
    fn a_hash_asks_for_directives() {
        assert_eq!(context_at("#", 1), Context::Directive);
        assert_eq!(context_at("#imp", 4), Context::Directive);
    }

    #[test]
    fn anything_else_asks_for_names() {
        assert_eq!(context_at("", 0), Context::Name);
        assert_eq!(context_at("pri", 3), Context::Name);
        assert_eq!(context_at("    n := ", 9), Context::Name);
        // A deref is not a field access, and `.*` is one token to the lexer.
        assert_eq!(context_at("p.*", 3), Context::Name);
    }

    #[test]
    fn the_classifier_does_not_need_the_file_to_parse() {
        // Which is the point: at the moment completion is requested it usually does not.
        assert_eq!(
            context_at("main :: () {\n    p.", 19),
            Context::Field { dot: 18 }
        );
    }

    #[test]
    fn a_call_snippet_numbers_its_holes() {
        let interner = jr_base::Interner::new();
        let span = jr_base::Span {
            file: jr_base::FileId::from_usize(0),
            range: jr_base::TextRange::empty(0.into()),
        };
        let param = |name: &str| jr_hir::Param {
            name: interner.intern(name),
            name_span: span,
            ty: None,
            using: false,
            comptime: false,
            variadic: false,
            default: None,
        };
        let params = vec![param("a"), param("b")];
        assert_eq!(
            call_snippet("add", &params, &interner),
            "add(${1:a}, ${2:b})$0"
        );
        assert_eq!(call_snippet("f", &[], &interner), "f()$0");
    }
}
