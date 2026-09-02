//! One renderer, three consumers: hover, a completion item, and `completionItem/resolve`.
//!
//! # Why this is its own module
//!
//! [ADR-0028](../../../docs/adr/0028-hover-and-completion.md) §1. The specific failure
//! it prevents is a completion list and a hover disagreeing about the same procedure's
//! signature — invisible to tests written per handler, because each one would be
//! internally consistent. Every rendering of a declaration in this crate comes from
//! [`Decl::card`].
//!
//! # Why the card is Jairs syntax
//!
//! §2 of the same ADR. `print :: (s: string)` is what the declaration says; rendering
//! `pub fn print(s: string)` would describe a language this is not. Jairs has no `fn`
//! and no visibility modifier — every top-level item is what an importer may see, which
//! is what `FileSignatures` means — so there is nothing to put in front of the name.
//!
//! # Where the parameter *names* come from
//!
//! `jr-hir`'s `Param::name`; the *types* come from `jr-sema`'s `ProcSig::params`. Neither
//! source has both. That split is why this module needs an [`Interner`] as well as a
//! [`Pool`]: a `Symbol` is an index into the former and a `PoolId` into the latter.

use jr_base::{Interner, Symbol};
use jr_db::{ConstValues, FileDocs};
use jr_hir::{ConstValue, FileHir, ItemId, ItemKind};
use jr_pool::{IntKind, Item, Pool, PoolId};
use jr_sema::{FileSignatures, SigKind};

/// A rendered declaration, ready to become hover markup or a completion item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    /// The module the declaration came from, or the file stem for a local one.
    ///
    /// Always present, so the card's shape does not change with the item's origin
    /// (ADR-0028 §3). Jairs modules are flat, so this is one segment where Rust's
    /// equivalent is a path.
    pub container: String,
    /// The declaration, in Jairs syntax: `add :: (a: s64, b: s64) -> s64`.
    pub signature: String,
    /// The `///` documentation, if the declaration has any.
    pub docs: Option<String>,
}

impl Card {
    /// The card as LSP markdown.
    ///
    /// Container and signature share one `jr` fence so a client highlights the
    /// signature; the `---` rule separates code from prose, which is the shape
    /// ADR-0028 §2 fixed.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = format!("```jr\n{}\n{}\n```", self.container, self.signature);
        if let Some(docs) = &self.docs {
            out.push_str("\n\n---\n\n");
            out.push_str(docs);
        }
        out
    }
}

/// Everything needed to render a declaration from one file.
///
/// Borrowed rather than fetched inside, because for an imported item every one of these
/// comes from the *other* file, and a renderer that fetched them itself would have to
/// know which. The caller already knows.
pub struct Decl<'a> {
    /// The declaring file's HIR: names, parameter names, item kinds.
    pub hir: &'a FileHir,
    /// The declaring file's signatures: resolved types.
    pub sigs: &'a FileSignatures,
    /// The declaring file's doc comments (ADR-0027).
    pub docs: &'a FileDocs,
    /// Constant values, when they have been computed. `None` means a constant renders
    /// its type without a value rather than claiming it has none.
    pub consts: Option<&'a ConstValues>,
    /// The type pool, for rendering a `PoolId`.
    pub pool: &'a Pool,
    /// The interner, for resolving a `Symbol`.
    pub interner: &'a Interner,
    /// What to put on the container line.
    pub container: &'a str,
}

impl Decl<'_> {
    /// The full card for a file-level item.
    ///
    /// `None` for an item with no name — a top-level `#run` — because there is nothing
    /// to hover or complete.
    #[must_use]
    pub fn card(&self, item: ItemId) -> Option<Card> {
        Some(Card {
            container: self.container.to_owned(),
            signature: self.signature(item)?,
            docs: self.docs.get(item).map(ToOwned::to_owned),
        })
    }

    /// Just the signature line, which is what a completion item's `detail` shows.
    #[must_use]
    pub fn signature(&self, item_id: ItemId) -> Option<String> {
        let item = self.hir.items.get(item_id.index())?;
        let name = self.interner.resolve(item.name?).to_owned();

        match &item.kind {
            ItemKind::Const { value } => match value {
                ConstValue::Proc(proc) => Some(self.proc_signature(&name, *proc)),
                // Rendered as `operator + :: (…)` — the *source* form, not the synthetic name,
                // because a hover card showing `operator+` would display something the user
                // cannot write (ADR-0048 §1).
                ConstValue::Operator(proc, op) => {
                    let rendered = self.proc_signature(&name, *proc);
                    let spelled =
                        rendered.replacen(&name, &format!("operator {}", op_text(*op)), 1);
                    Some(spelled)
                }
                ConstValue::Struct(_) => Some(self.struct_signature(&name, "struct")),
                ConstValue::Union(_) => Some(self.struct_signature(&name, "union")),
                ConstValue::Variant(_) => Some(self.struct_signature(&name, "variant")),
                ConstValue::Enum(id) => Some(self.enum_signature(&name, *id)),
                ConstValue::Expr(_) => Some(self.const_signature(&name, item_id)),
            },
            ItemKind::Var { .. } => Some(format!("{name}: {}", self.declared_type(&name))),
            // Rendered because hovering the path of an `#import` is a reasonable thing
            // to do, and because a card whose signature is blank looks like a bug.
            ItemKind::Import { path, .. } => Some(format!("#import \"{path}\"")),
            // A top-level `#run` has no name, so `item.name?` above already returned.
            // Kept exhaustive so that a new item kind is a compile error here.
            ItemKind::Run { .. } => None,
        }
    }

    /// `add :: (a: s64, b: s64) -> s64`.
    ///
    /// The parameter list is zipped from two sources; where they disagree in length —
    /// which means lowering and signature resolution saw different parameters, a bug —
    /// the type is rendered as `?` rather than panicking or silently truncating.
    fn proc_signature(&self, name: &str, proc: jr_hir::ProcId) -> String {
        let params = self.hir.procs.get(proc.index()).map(|p| &p.params);
        let sig = self.sigs.proc_sig(proc);

        let rendered: Vec<String> = match (params, sig) {
            (Some(params), Some(sig)) => params
                .iter()
                .enumerate()
                .map(|(i, param)| {
                    let ty = sig
                        .params
                        .get(i)
                        .map_or_else(|| String::from("?"), |ty| self.type_name_of(*ty));
                    format!("{}: {ty}", self.interner.resolve(param.name))
                })
                .collect(),
            // No signature: the declaration did not resolve. The names are still
            // useful, and claiming a type we do not have would be worse.
            (Some(params), None) => params
                .iter()
                .map(|param| self.interner.resolve(param.name).to_owned())
                .collect(),
            (None, _) => Vec::new(),
        };

        let ret = sig.map(|sig| self.type_name_of(sig.ret));
        match ret {
            // Jairs writes no arrow for a procedure that returns nothing, so neither
            // does this.
            Some(ret) if ret != "void" => {
                format!("{name} :: ({}) -> {ret}", rendered.join(", "))
            }
            _ => format!("{name} :: ({})", rendered.join(", ")),
        }
    }

    /// `Point :: struct { x: s64; y: s64 }`.
    ///
    /// Fields come from the pool rather than from HIR, because the pool holds them
    /// *typed*: `jr-hir`'s `Field::ty` is a `TypeRefId` that still has to be resolved,
    /// and resolving it here would be a second implementation of what `jr-sema` did.
    /// Renders `name :: struct { … }` or `name :: union { … }`.
    ///
    /// `keyword` is a parameter rather than a literal, because a hover card that called a union
    /// a struct would misdescribe a type whose fields *overlap* — the same class of mistake as
    /// the formatter emitting a literal `"enum"` for an `enum_flags` (ADR-0043).
    fn struct_signature(&self, name: &str, keyword: &str) -> String {
        let fields = self
            .interner
            .get(name)
            .and_then(|sym| self.sigs.lookup(sym))
            .and_then(|entry| entry.type_value)
            .and_then(|ty| match self.pool.item(ty) {
                Item::StructType { .. } | Item::UnionType { .. } | Item::VariantType { .. } => {
                    self.pool.fields_of(ty)
                }
                _ => None,
            });

        match fields {
            Some(fields) if !fields.is_empty() => {
                let body: Vec<String> = fields
                    .iter()
                    .map(|field| {
                        format!(
                            "{}: {}",
                            self.interner.resolve(field.name),
                            self.type_name_of(field.ty)
                        )
                    })
                    .collect();
                format!("{name} :: {keyword} {{ {} }}", body.join("; "))
            }
            // An empty or unresolved aggregate still renders with its keyword, because that
            // is what the source says.
            _ => format!("{name} :: {keyword} {{}}"),
        }
    }

    /// `Colour :: enum { RED = 0; GREEN = 1; }` — with the *resolved* numbers.
    ///
    /// The values are what a hover card can say that the source cannot: auto-numbering is
    /// invisible in `enum { RED; GREEN; }`, and the whole point of showing a member list is
    /// to answer "what number is `GREEN`". Read from the pool, which is where `jr-sema`
    /// recorded them, so the card cannot disagree with the compiler about a value.
    fn enum_signature(&self, name: &str, id: jr_hir::EnumId) -> String {
        let denoted = self
            .interner
            .get(name)
            .and_then(|sym| self.sigs.lookup(sym))
            .and_then(|entry| entry.type_value);
        // The *form* matters on a card as much as the values do: `enum` and `enum_flags`
        // number differently and accept different operators (ADR-0043), so showing one as the
        // other would misdescribe the type.
        let keyword = match denoted.map(|ty| self.pool.item(ty)) {
            Some(Item::EnumType { flags: true, .. }) => "enum_flags",
            _ => "enum",
        };
        let members = denoted.and_then(|ty| match self.pool.item(ty) {
            Item::EnumType { decl, .. } => self.pool.enum_members(*decl),
            _ => None,
        });

        match members {
            Some(members) if !members.is_empty() => {
                let body: Vec<String> = members
                    .iter()
                    .map(|m| format!("{} = {}", self.interner.resolve(m.name), m.value))
                    .collect();
                format!("{name} :: {keyword} {{ {} }}", body.join("; "))
            }
            // `id` is unused for well-formed input; keeping the parameter means a future
            // per-member rendering has the HIR to reach for without changing the caller.
            _ => {
                let _ = id;
                format!("{name} :: {keyword} {{}}")
            }
        }
    }

    /// `MESSAGE :: string`, or `COMPUTED :: s64 = 9` when the value is known.
    ///
    /// The value comes from `file_consts`, which is why [`Decl::consts`] is optional:
    /// hover on a constant should not be the thing that forces const-evaluation of a
    /// file that has not been checked yet.
    fn const_signature(&self, name: &str, item: ItemId) -> String {
        let ty = self.declared_type(name);
        match self.consts.and_then(|consts| consts.item(item)) {
            Some(value) => match self.value_text(value) {
                Some(text) => format!("{name} :: {ty} = {text}"),
                None => format!("{name} :: {ty}"),
            },
            None => format!("{name} :: {ty}"),
        }
    }

    /// The type a name denotes when it is a type, and otherwise the type it has.
    fn declared_type(&self, name: &str) -> String {
        self.interner
            .get(name)
            .and_then(|sym| self.sigs.lookup(sym))
            .map_or_else(
                || String::from("<unknown>"),
                |entry| match entry.kind {
                    // A struct or enum name used as a value has type `type` (ADR-0012),
                    // which is not what a reader wants to see on its own declaration.
                    SigKind::Struct | SigKind::Union | SigKind::Variant | SigKind::Enum => entry
                        .type_value
                        .map_or_else(|| String::from("type"), |ty| self.type_name_of(ty)),
                    SigKind::Const | SigKind::Var | SigKind::Proc | SigKind::Operator => {
                        self.type_name_of(entry.ty)
                    }
                },
            )
    }

    /// A constant's computed value as source-like text, if it has one.
    ///
    /// The inlay-hint caller's entry point (ADR-0031 §7). Public where `value_text`
    /// is private because a hint needs the value *alone*, without the `NAME :: type =`
    /// framing a card puts around it — and ADR-0028 §1 requires that both come from here
    /// rather than a hint formatting a `PoolId` itself.
    #[must_use]
    pub fn value_of(&self, item: ItemId) -> Option<String> {
        let value = self.consts?.item(item)?;
        self.value_text(value)
    }

    /// A constant's value as source-like text, or `None` if there is nothing useful to print.
    ///
    /// **An aggregate constant renders its elements** (ADR-0074 §1), which this used to say was not worth
    /// it because "an aggregate constant would need the layout rules to print". That was true when the only
    /// possible representation was a byte image; interning the *element values* instead means the elements
    /// are right there, and `{7, 0}` on a hover card is strictly better than the type alone.
    fn value_text(&self, value: PoolId) -> Option<String> {
        if value.index() >= self.pool.len() {
            return None;
        }
        match self.pool.item(value) {
            // Decoded through `IntKind` rather than printed as raw bits: `jr-mir`'s
            // dump prints `18446744073709551615_s64` for -1 because a dump is a debug
            // aid, but a hover card is prose and must say `-1`.
            Item::IntValue { ty, bits } => {
                IntKind::of(self.pool, *ty).map(|kind| kind.decode(*bits).to_string())
            }
            Item::BoolValue(value) => Some(value.to_string()),
            // A results aggregate is a transport, never a constant (ADR-0052 §4).
            Item::ContextType | Item::ResultsType { .. } => None,
            // `{:?}` so `1.0` does not render as `1` on a hover card and read as an integer.
            Item::FloatValue { ty, bits } => jr_pool::FloatKind::of(self.pool, *ty)
                .map(|kind| format!("{:?}", kind.decode(*bits))),
            Item::StrValue(id) => Some(format!("\"{}\"", escape(self.pool.resolve_str(*id)))),
            // Elements, recursively, so a nested aggregate reads as `{{1, 2}, {3, 4}}` (ADR-0074 §1).
            // An element that renders nothing is skipped rather than omitted silently — printing
            // `{7, }` would be worse than `{7}`, and both are better than the type alone.
            Item::AggregateValue { elements, .. } => {
                let parts: Vec<String> =
                    elements.iter().filter_map(|e| self.value_text(*e)).collect();
                Some(format!("{{{}}}", parts.join(", ")))
            }
            Item::VoidValue
            | Item::TypeValue(_)
            | Item::ProcValue { .. }
            | Item::ForeignLibraryValue(_)
            | Item::VoidType
            | Item::BoolType
            | Item::IntType { .. }
            | Item::FloatType { .. }
            | Item::StringType
            | Item::TypeType
            | Item::ErrorType
            | Item::ForeignLibraryType
            | Item::PointerType(_)
            // An array constant would need the layout rules to print, which is exactly
            // the narrowness this function's doc comment claims. A vector's is the same bytes and
            // the same narrowness.
            // A compiler-emitted table has no hover card: its contents are a read-only region
            // a program never names (ADR-0152 §1).
            | Item::StaticArray { .. }
            | Item::ArrayType { .. }
            | Item::VectorType { .. }
            | Item::ViewType { .. }
            | Item::DynamicArrayType { .. }
            | Item::EnumType { .. }
            | Item::StructType { .. }
        | Item::UnionType { .. }
        | Item::VariantType { .. }
            | Item::ProcType { .. } => None,
        }
    }

    /// [`type_name`] with this card's pool and signatures.
    fn type_name_of(&self, ty: PoolId) -> String {
        type_name(self.pool, self.sigs, ty)
    }
}

/// The card for an `#import` line: the declaration, where it resolved, and the module's
/// own `//!` documentation.
///
/// A free function rather than a [`Decl`] method because none of [`Decl`]'s fields apply —
/// an import has no type, no signature and no `///` of its own. What it has is a *resolved
/// path*, which only the caller knows, having done the lookup.
///
/// # Why the path is the point
///
/// `#import "Basic"` does not say **which** `Basic`. ADR-0014's search-path order decides,
/// so the answer depends on how the server was configured — and that is the one question the
/// line cannot answer for itself (ADR-0035 §2). The module's `//!` block comes along because
/// `file_docs` has collected it since ADR-0027 §2 and nothing has ever shown it.
///
/// `found` is `None` for a module that did not resolve. The card then says so rather than
/// rendering nothing: E0210 already reports the error, and a hover that vanishes next to a
/// diagnostic reads as a second, unrelated failure.
#[must_use]
pub fn import_card(module: &str, found: Option<&std::path::Path>, docs: Option<&str>) -> Card {
    let mut body = match found {
        Some(path) => format!("{}", path.display()),
        None => String::from("not found on any module search path"),
    };
    if let Some(docs) = docs {
        body.push_str("\n\n");
        body.push_str(docs);
    }
    Card {
        container: String::from("module"),
        signature: format!("#import \"{module}\""),
        docs: Some(body),
    }
}

/// A card for a parameter or a local, which are not items and have no documentation.
#[must_use]
pub fn binding_card(container: &str, name: &str, ty: Option<String>) -> Card {
    Card {
        container: container.to_owned(),
        signature: format!(
            "{name}: {}",
            ty.unwrap_or_else(|| String::from("<unknown>"))
        ),
        docs: None,
    }
}

/// Renders a type for a human.
///
/// Moved here from `handlers` when it acquired a second caller. ADR-0024 §6 records why
/// a third copy of *this* is acceptable where ADR-0022 §2 refused a third copy of
/// arithmetic: a wrong render is cosmetic, a wrong fold is a miscompile.
#[must_use]
pub fn type_name(pool: &Pool, signatures: &FileSignatures, ty: PoolId) -> String {
    if ty.index() >= pool.len() {
        return String::from("<unknown>");
    }
    match pool.item(ty) {
        Item::VoidType => String::from("void"),
        Item::BoolType => String::from("bool"),
        Item::IntType { signed, bits } => format!("{}{bits}", if *signed { 's' } else { 'u' }),
        Item::StringType => String::from("string"),
        Item::FloatType { bits } => format!("float{bits}"),
        Item::EnumType { decl, .. } => signatures
            .type_name(ty)
            .map_or_else(|| format!("enum{decl:?}"), ToOwned::to_owned),
        Item::TypeType => String::from("type"),
        Item::ErrorType => String::from("<unknown>"),
        Item::ForeignLibraryType => String::from("#system_library"),
        Item::PointerType(pointee) => format!("*{}", type_name(pool, signatures, *pointee)),
        Item::ArrayType { elem, len } => {
            format!("[{len}]{}", type_name(pool, signatures, *elem))
        }
        // `#simd` included, for the same reason sema's `describe` and the MIR dump both spell it: a
        // hover card that said `[4]s32` would name a type the program does not have, and the one
        // difference between the two is the whole reason the vector exists (ADR-0148 §1).
        Item::VectorType { elem, lanes } => {
            format!("#simd [{lanes}]{}", type_name(pool, signatures, *elem))
        }
        Item::ViewType { elem } => format!("[]{}", type_name(pool, signatures, *elem)),
        Item::DynamicArrayType { elem } => format!("[..]{}", type_name(pool, signatures, *elem)),
        // Spelled as written, so hovering a multi-result procedure shows `-> (s64, bool)` rather
        // than a name the user never typed (ADR-0052 §1).
        Item::ContextType => "Context".to_owned(),
        Item::ResultsType { elems } => {
            let parts: Vec<String> = elems
                .iter()
                .map(|ty| type_name(pool, signatures, *ty))
                .collect();
            format!("({})", parts.join(", "))
        }
        Item::StructType { decl, .. } => signatures
            .type_name(ty)
            .map_or_else(|| format!("struct{decl:?}"), ToOwned::to_owned),
        Item::UnionType { decl, .. } => signatures
            .type_name(ty)
            .map_or_else(|| format!("union{decl:?}"), ToOwned::to_owned),
        Item::VariantType { decl, .. } => signatures
            .type_name(ty)
            .map_or_else(|| format!("variant{decl:?}"), ToOwned::to_owned),
        Item::ProcType {
            params,
            ret,
            context: _,
            effects: _,
        } => {
            let params: Vec<String> = params
                .iter()
                .map(|ty| type_name(pool, signatures, *ty))
                .collect();
            let ret = type_name(pool, signatures, *ret);
            if ret == "void" {
                format!("({})", params.join(", "))
            } else {
                format!("({}) -> {ret}", params.join(", "))
            }
        }
        // A value where a type was expected is a bug elsewhere; rendered rather than
        // hidden, for the same reason `jr-mir`'s dump does it.
        Item::VoidValue
        | Item::BoolValue(_)
        | Item::IntValue { .. }
        | Item::FloatValue { .. }
        | Item::StaticArray { .. }
        | Item::StrValue(_)
        | Item::TypeValue(_)
        | Item::ProcValue { .. }
        | Item::ForeignLibraryValue(_)
        // A value where a *type* was expected (ADR-0074 §1).
        | Item::AggregateValue { .. } => String::from("<value>"),
    }
}

/// The container line for a file path: its stem.
///
/// `modules/Basic/module.jr` is the module `Basic`, not `module` — the directory names
/// a Jairs module and the file inside it is always `module.jr`, so the stem would be
/// the same string for every module in the system.
#[must_use]
pub fn container_of(path: &str) -> String {
    let path = std::path::Path::new(path);
    let stem = path.file_stem().map(|stem| stem.to_string_lossy());
    match stem.as_deref() {
        Some("module") => path.parent().and_then(|dir| dir.file_name()).map_or_else(
            || String::from("module"),
            |dir| dir.to_string_lossy().into(),
        ),
        Some(stem) => stem.to_owned(),
        None => String::new(),
    }
}

/// Escapes a string constant's value for display inside quotes.
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
        .replace('"', "\\\"")
}

/// The symbol for a name, if it has ever been interned.
///
/// A name that was never interned cannot be declared anywhere, so `None` is a lookup
/// miss rather than an error.
#[must_use]
pub fn symbol_of(interner: &Interner, name: &str) -> Option<Symbol> {
    interner.get(name)
}

/// The source spelling of a binary operator, for an overload's hover card.
///
/// A local copy rather than an export from `jr-sema`, whose `bin_op_text` is `pub(crate)`:
/// widening that crate's API to hand out a formatting choice would make it part of the public
/// contract, which is the same argument `jr-mir`'s dump makes for having its own `ty`.
fn op_text(op: jr_hir::BinOp) -> &'static str {
    match op {
        jr_hir::BinOp::Add => "+",
        jr_hir::BinOp::Sub => "-",
        jr_hir::BinOp::Mul => "*",
        jr_hir::BinOp::Div => "/",
        jr_hir::BinOp::Rem => "%",
        jr_hir::BinOp::WrapAdd => "+%",
        jr_hir::BinOp::WrapSub => "-%",
        jr_hir::BinOp::WrapMul => "*%",
        jr_hir::BinOp::Eq => "==",
        jr_hir::BinOp::Ne => "!=",
        jr_hir::BinOp::Lt => "<",
        jr_hir::BinOp::Le => "<=",
        jr_hir::BinOp::Gt => ">",
        jr_hir::BinOp::Ge => ">=",
        jr_hir::BinOp::And => "&&",
        jr_hir::BinOp::Or => "||",
        jr_hir::BinOp::BitAnd => "&",
        jr_hir::BinOp::BitOr => "|",
        jr_hir::BinOp::BitXor => "^",
        jr_hir::BinOp::Shl => "<<",
        jr_hir::BinOp::Shr => ">>",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_without_docs_has_no_rule() {
        let card = Card {
            container: String::from("Basic"),
            signature: String::from("print :: (s: string)"),
            docs: None,
        };
        assert_eq!(
            card.to_markdown(),
            "```jr\nBasic\nprint :: (s: string)\n```"
        );
    }

    #[test]
    fn a_card_with_docs_separates_them_with_a_rule() {
        let card = Card {
            container: String::from("Basic"),
            signature: String::from("print :: (s: string)"),
            docs: Some(String::from("Write a string to standard output.")),
        };
        assert_eq!(
            card.to_markdown(),
            "```jr\nBasic\nprint :: (s: string)\n```\n\n---\n\nWrite a string to standard output."
        );
    }

    #[test]
    fn a_module_file_is_named_by_its_directory() {
        // Every module's file is `module.jr`, so the stem alone would render every
        // module in the system as `module`.
        assert_eq!(container_of("/x/modules/Basic/module.jr"), "Basic");
        assert_eq!(
            container_of("/x/tests/corpus/valid/024-hello.jr"),
            "024-hello"
        );
        assert_eq!(container_of("main.jr"), "main");
    }

    #[test]
    fn a_string_value_is_escaped() {
        assert_eq!(escape("hello\n"), "hello\\n");
        assert_eq!(escape("a\"b"), "a\\\"b");
        assert_eq!(escape("a\\b"), "a\\\\b");
    }
}
