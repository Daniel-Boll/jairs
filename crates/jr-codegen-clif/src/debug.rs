//! The DWARF line table: from a MIR span to `.debug_line` (ADR-0169).
//!
//! # Why this exists at all, and why it starts from zero
//!
//! PLAN §8.4 claimed "line tables exist" and ADR-0159 found there is **no DWARF at all** — `dwarfdump`
//! reports an empty `.debug_line` on a built binary and `size -m` finds no debug section. So this is the
//! first debug information this compiler has ever produced, and W12's first item.
//!
//! # The shape, and the one decision that makes it small
//!
//! Cranelift attaches a `SourceLoc` — an opaque `u32` — to each instruction, and hands back
//! `(code offset, SourceLoc)` pairs after compiling a function. Nothing in Cranelift knows what the `u32`
//! *means*, which is the whole point: it is this crate's number.
//!
//! So the `u32` is an index into a [`LineVocabulary`] — a deduplicated list of `(path, line)` pairs. A
//! statement's span is resolved **once**, when the instruction is emitted, through the same
//! [`jr_codegen::SourceInfo`] a trap message uses. That is the decision that keeps a `.debug_line` row and
//! a trap's `--> file:line:col` from ever disagreeing: they are the same lookup, and ADR-0169 §2 argues it at
//! length.
//!
//! **Rejected: encoding the span's byte offset into the `SourceLoc`.** It needs no vocabulary, and it needs
//! the *file* to be recoverable some other way — a `u32` cannot hold both a `FileId` and an offset without a
//! bit-packing scheme that breaks silently on a large file. An index into a table this crate owns has no such
//! ceiling.
//!
//! **Rejected: `u32::MAX` as a vocabulary entry.** Cranelift spells "no location" as
//! `SourceLoc::default()`, which *is* `u32::MAX`, so that index is unusable and [`LineVocabulary::intern`]
//! refuses to hand it out. A vocabulary would need four billion distinct source lines to reach it, so the
//! refusal is a statement rather than a limit anyone meets.
//!
//! # What a reader gets from this
//!
//! `dwarfdump --debug-line` shows real rows, and a symbolizer maps a crash address to a source line. That is
//! the whole deliverable: a backtrace from a native binary can name a line, which until now only the VM's
//! own trap path could do.
//!
//! Type DIEs and locals are W12's next two items and are deliberately absent — a line program is
//! independently useful, and a `.debug_info` with no types is not.

use gimli::write::{Address, AttributeValue, DwarfUnit, LineProgram, LineString, Sections, Writer};
use gimli::{Encoding, Format, LineEncoding, RunTimeEndian, SectionId};
use rustc_hash::FxHashMap;

/// The `(path, line)` pairs a module's instructions refer to, deduplicated.
///
/// An instruction's Cranelift `SourceLoc` is an index into this. Built while bodies are translated, read
/// when the object is emitted.
#[derive(Default)]
pub struct LineVocabulary {
    /// Every distinct position, in the order first seen. The index *is* the `SourceLoc`.
    rows: Vec<(String, u32)>,
    /// Reverse lookup, so a statement on a line already seen costs no allocation and no row.
    index: FxHashMap<(String, u32), u32>,
}

impl LineVocabulary {
    /// The index for `(path, line)`, adding it if it is new.
    ///
    /// `None` when the vocabulary is full — which needs `u32::MAX` distinct source lines, because that value
    /// is Cranelift's "no location" and cannot be an index. A caller treats `None` as "emit no location for
    /// this instruction", which degrades the line table rather than the program.
    pub fn intern(&mut self, path: &str, line: u32) -> Option<u32> {
        let key = (path.to_owned(), line);
        if let Some(found) = self.index.get(&key) {
            return Some(*found);
        }
        let next = u32::try_from(self.rows.len()).ok()?;
        if next == u32::MAX {
            return None;
        }
        self.rows.push(key.clone());
        self.index.insert(key, next);
        Some(next)
    }

    /// The position an index names.
    fn row(&self, index: u32) -> Option<&(String, u32)> {
        self.rows.get(usize::try_from(index).ok()?)
    }

    /// Whether nothing was ever interned.
    ///
    /// A module with no positions gets no `.debug_line` at all, rather than an empty one: a line program with
    /// no sequences is a section a debugger must parse to learn nothing.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Every distinct path, in first-seen order.
    ///
    /// DWARF's line program has its own file table, so each path needs an index there too. First-seen order
    /// rather than sorted, so the primary source file is index 0 for a single-file program — which is what a
    /// reader expects and what makes the common case's table one entry long.
    fn paths(&self) -> Vec<&str> {
        let mut seen = Vec::new();
        for (path, _) in &self.rows {
            if !seen.contains(&path.as_str()) {
                seen.push(path.as_str());
            }
        }
        seen
    }
}

/// A DWARF type, described in a form that outlives the body it was discovered in (ADR-0173 §1).
///
/// # Why a description and not a gimli DIE
///
/// A struct's members need **field names**, and resolving a `Symbol` needs the driver's
/// [`jr_codegen::SourceInfo`] — which exists only while a body is being defined. The DIEs, meanwhile, can only
/// be written once the object exists, at `finalise`. So the two are separated: the *description* is built when
/// names are available and the *DIE* is written when the unit is.
///
/// The alternative was threading a `SourceInfo` into `finalise`, which would mean the driver keeping a
/// per-module name resolver alive beside its per-body one — a second channel for a question the first already
/// answers, at the wrong granularity.
#[derive(Clone, PartialEq, Eq)]
pub enum TypeDescription {
    /// A `DW_TAG_base_type`: a name, a size in bytes, and a DWARF encoding.
    Base {
        /// The type's name, as the language spells it.
        name: String,
        /// Size in bytes.
        size: u64,
        /// A `DW_ATE_*` encoding.
        encoding: gimli::DwAte,
    },
    /// A `DW_TAG_pointer_type`, with the pointee's description index when it is known.
    Pointer {
        /// Size in bytes — the target's pointer width.
        size: u64,
        /// The pointee, or `None` for an opaque pointer.
        ///
        /// `None` is what breaks a cycle: a self-referential struct's pointer is described without its
        /// pointee rather than recursing forever, the same terminator ADR-0171 §1 records for LLVM.
        pointee: Option<usize>,
    },
    /// A `DW_TAG_structure_type` with a `DW_TAG_member` per field.
    Struct {
        /// Size in bytes, including trailing padding.
        size: u64,
        /// Each member's name, byte offset and description index.
        members: Vec<(String, u64, usize)>,
    },
}

/// Every type description a module needs, deduplicated by `PoolId`.
///
/// Built during `define`, read during `finalise`. The `PoolId` keying inherits the pool's own structural
/// deduplication, so two identical struct declarations produce one DIE.
#[derive(Default)]
pub struct TypeDescriptions {
    /// Descriptions in insertion order; an index into this is how one refers to another.
    entries: Vec<TypeDescription>,
    /// `PoolId` to index, so a type is described once.
    index: FxHashMap<jr_pool::PoolId, usize>,
}

impl TypeDescriptions {
    /// The index for `ty`, describing it and its members if this is the first ask.
    ///
    /// `None` for a type this wave does not describe — the same set LLVM's mapping leaves out, and for the
    /// same reasons (ADR-0171 §1): `void` has no DIE by definition, and views, arrays, unions, variants and
    /// procedure types each need a naming decision this wave does not make.
    pub fn describe(
        &mut self,
        pool: &jr_pool::Pool,
        target: jr_pool::TargetLayout,
        ty: jr_pool::PoolId,
        names: &dyn jr_codegen::SourceInfo,
    ) -> Option<usize> {
        if let Some(found) = self.index.get(&ty) {
            return Some(*found);
        }
        let layout = jr_pool::layout_of(pool, target, ty).ok()?;

        let described = match pool.item(ty) {
            jr_pool::Item::BoolType => TypeDescription::Base {
                name: "bool".to_owned(),
                size: layout.size,
                encoding: gimli::DW_ATE_boolean,
            },
            jr_pool::Item::IntType { signed, bits } => TypeDescription::Base {
                name: if *signed {
                    format!("s{bits}")
                } else {
                    format!("u{bits}")
                },
                size: layout.size,
                encoding: if *signed {
                    gimli::DW_ATE_signed
                } else {
                    gimli::DW_ATE_unsigned
                },
            },
            jr_pool::Item::FloatType { bits } => TypeDescription::Base {
                name: format!("float{bits}"),
                size: layout.size,
                encoding: gimli::DW_ATE_float,
            },
            jr_pool::Item::PointerType(pointee) => {
                let pointee = *pointee;
                TypeDescription::Pointer {
                    size: layout.size,
                    // Described first, then referenced — so a cycle finds nothing and yields an opaque
                    // pointer instead of recursing.
                    pointee: self.describe(pool, target, pointee, names),
                }
            }
            jr_pool::Item::StructType { decl, .. } => {
                let decl = *decl;
                let fields = pool.struct_fields(decl)?.to_vec();
                let mut members = Vec::with_capacity(fields.len());
                for (position, field) in fields.iter().enumerate() {
                    let member = self.describe(pool, target, field.ty, names)?;
                    let (offset, _) =
                        jr_pool::field_offset(pool, target, ty, u32::try_from(position).ok()?)
                            .ok()?;
                    members.push((names.symbol(field.name).unwrap_or_default(), offset, member));
                }
                TypeDescription::Struct {
                    size: layout.size,
                    members,
                }
            }
            _ => return None,
        };

        let at = self.entries.len();
        self.entries.push(described);
        self.index.insert(ty, at);
        Some(at)
    }

    /// Whether nothing was described.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// One compiled function's DWARF subprogram (ADR-0173 §2).
pub struct FunctionSubprogram {
    /// The object symbol the code was defined under, for `DW_AT_low_pc`'s relocation.
    pub symbol: object::write::SymbolId,
    /// The code length, for `DW_AT_high_pc`.
    pub length: u64,
    /// The source name, or `None` for a procedure no item binds.
    pub name: Option<String>,
    /// The declaration line, or 0 when unknown.
    pub line: u32,
    /// The return type's description index, when it has one.
    pub ret: Option<usize>,
    /// Each named local: its name, its type description index, and its offset from the frame base
    /// (ADR-0174 §2).
    pub variables: Vec<(String, usize, i64)>,
}

/// One compiled function's contribution to the line table.
pub struct FunctionLines {
    /// The object symbol the function's code was defined under.
    ///
    /// A **symbol** rather than an address, because at object-file level there is no address yet: the line
    /// program's sequence start becomes a relocation the linker fills in. Getting this wrong is the classic
    /// way to produce a line table whose every row points at the start of the text section.
    pub symbol: object::write::SymbolId,
    /// The function's code length in bytes.
    pub length: u64,
    /// `(code offset from the function's start, vocabulary index)`, ascending by offset.
    pub rows: Vec<(u32, u32)>,
}

/// A relocation a DWARF section needs the linker to fill in.
///
/// A line program's sequence starts at a function's *address*, which does not exist in an object file — so the
/// bytes are left zero and this says which symbol goes there. Getting it wrong is the classic way to produce a
/// line table whose every row points at the start of the text section.
#[derive(Clone)]
struct DebugReloc {
    /// Byte offset within the section being written.
    offset: usize,
    /// Index into the writer's symbol table.
    symbol: usize,
    /// Added to the symbol's address.
    addend: i64,
    /// Width of the slot, in bytes.
    size: u8,
}

/// A gimli [`Writer`] that records symbol references instead of refusing them.
///
/// `gimli::write::EndianVec` errors on [`Address::Symbol`], because a plain byte buffer has nowhere to put a
/// relocation — which is exactly what an object file needs. So this wraps a buffer and a relocation list.
///
/// **The symbol is an index into a side table, not `object`'s `SymbolId`.** gimli's `Address::Symbol` carries a
/// `usize` and `object`'s `SymbolId` is opaque with no accessor, so the two cannot be the same number. An index
/// into a `Vec<SymbolId>` this crate owns bridges them exactly, and the first draft of this file instead
/// recovered the id by parsing `SymbolId`'s `Debug` output — which worked and depended on another crate's
/// formatting, so it is recorded here as the rejected alternative rather than left in.
#[derive(Clone)]
struct RelocWriter {
    bytes: Vec<u8>,
    endian: RunTimeEndian,
    relocs: Vec<DebugReloc>,
}

impl RelocWriter {
    fn new(endian: RunTimeEndian) -> Self {
        Self {
            bytes: Vec::new(),
            endian,
            relocs: Vec::new(),
        }
    }
}

impl Writer for RelocWriter {
    type Endian = RunTimeEndian;

    fn endian(&self) -> Self::Endian {
        self.endian
    }

    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), gimli::write::Error> {
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn write_at(&mut self, offset: usize, bytes: &[u8]) -> Result<(), gimli::write::Error> {
        let end = offset + bytes.len();
        if end > self.bytes.len() {
            return Err(gimli::write::Error::LengthOutOfBounds);
        }
        self.bytes[offset..end].copy_from_slice(bytes);
        Ok(())
    }

    fn write_address(&mut self, address: Address, size: u8) -> Result<(), gimli::write::Error> {
        match address {
            Address::Constant(value) => self.write_udata(value, size),
            Address::Symbol { symbol, addend } => {
                self.relocs.push(DebugReloc {
                    offset: self.bytes.len(),
                    symbol,
                    addend,
                    size,
                });
                // Zero for now: the linker writes the real address over these bytes using the relocation.
                self.write_udata(0, size)
            }
        }
    }
}

/// Builds a module's line table and adds it to `object`.
///
/// `comp_dir` and `primary` name the compilation unit: DWARF wants a directory and a primary file, and a
/// consumer joins them to find source, so `comp_dir` must be absolute.
///
/// Does nothing when the vocabulary is empty — a module with no positions gets no `.debug_line` at all rather
/// than an empty one, because a line program with no sequences is a section a consumer must parse to learn
/// nothing.
///
/// # Errors
/// The gimli error when a section cannot be written, which in practice means a malformed line program — a
/// sequence whose rows run past its length, say — rather than anything a source program can cause.
pub fn emit(
    object: &mut object::write::Object<'_>,
    vocabulary: &LineVocabulary,
    functions: &[FunctionLines],
    types: &TypeDescriptions,
    subprograms: &[FunctionSubprogram],
    comp_dir: &str,
    primary: &str,
    endian: RunTimeEndian,
    frame_pointer: gimli::Register,
) -> Result<(), gimli::write::Error> {
    if vocabulary.is_empty() && types.is_empty() {
        return Ok(());
    }

    let encoding = Encoding {
        // DWARF 4 rather than 5. Both `dwarfdump` and `lldb` read 4 everywhere, and 5 moves the line-program
        // header's file table in a way older consumers reject — a wave whose deliverable is "a debugger can
        // read this" should not also be a bet on consumer versions.
        version: 4,
        address_size: 8,
        format: Format::Dwarf32,
    };

    let mut program = LineProgram::new(
        encoding,
        LineEncoding::default(),
        // The working directory, then the primary file's directory (`None` = the working one) and the file
        // itself.
        LineString::String(comp_dir.as_bytes().to_vec()),
        None,
        LineString::String(primary.as_bytes().to_vec()),
        None,
    );

    // The line program's own file table, and the map from a vocabulary path to its index in it.
    let directory = program.default_directory();
    let mut files: FxHashMap<&str, gimli::write::FileId> = FxHashMap::default();
    for path in vocabulary.paths() {
        let id = program.add_file(
            LineString::String(path.as_bytes().to_vec()),
            directory,
            None,
        );
        files.insert(path, id);
    }

    // gimli addresses a symbol by index; this is the table those indices point into.
    let mut symbols: Vec<object::write::SymbolId> = Vec::new();
    for function in functions {
        if function.rows.is_empty() || function.length == 0 {
            // Nothing to say about a function with no positions, and a zero-length sequence is invalid DWARF.
            continue;
        }
        let symbol = symbols.len();
        symbols.push(function.symbol);
        program.begin_sequence(Some(Address::Symbol { symbol, addend: 0 }));
        for (offset, index) in &function.rows {
            let Some((path, line)) = vocabulary.row(*index) else {
                continue;
            };
            let Some(file) = files.get(path.as_str()) else {
                continue;
            };
            program.row().address_offset = u64::from(*offset);
            program.row().file = *file;
            program.row().line = u64::from(*line);
            // The column is deliberately not set. A DWARF row's column is optional, this compiler's spans are
            // per-statement rather than per-expression, and a column that is always the statement's first byte
            // is worse than none: a consumer would render a caret under the wrong token with full confidence.
            // `SourceInfo::position` carries the column for the trap *message*, which renders it as text
            // where its precision is visible.
            program.generate_row();
        }
        program.end_sequence(function.length);
    }
    if symbols.is_empty() {
        // Every function was skipped, so there is no sequence and the same "no empty section" rule applies.
        return Ok(());
    }

    let mut unit = DwarfUnit::new(encoding);
    unit.unit.line_program = program;
    let root = unit.unit.root();
    write_types_and_subprograms(
        &mut unit,
        root,
        types,
        subprograms,
        &mut symbols,
        frame_pointer,
    );
    unit.unit.get_mut(root).set(
        gimli::DW_AT_name,
        AttributeValue::String(primary.as_bytes().to_vec()),
    );
    unit.unit.get_mut(root).set(
        gimli::DW_AT_comp_dir,
        AttributeValue::String(comp_dir.as_bytes().to_vec()),
    );
    // The producer string, so `dwarfdump` says who made this. Cheap, and the first question anyone asks of an
    // unfamiliar DWARF section.
    unit.unit.get_mut(root).set(
        gimli::DW_AT_producer,
        AttributeValue::String(b"jairs".to_vec()),
    );

    let mut sections = Sections::new(RelocWriter::new(endian));
    unit.write(&mut sections)?;

    // Add every non-empty section to the object, then its relocations. Two passes because a relocation needs
    // the `SectionId` the first pass returns.
    let mut added: Vec<(object::write::SectionId, &RelocWriter)> = Vec::new();
    sections.for_each(|id, writer| {
        if writer.bytes.is_empty() {
            return Ok::<(), gimli::write::Error>(());
        }
        let name = debug_section_name(id, object.format());
        // The segment matters on Mach-O: a debug section outside `__DWARF` links with an alignment warning and
        // then fails with "pointer not aligned", because `ld` treats it as ordinary data to be laid out among
        // the pointers. `object` knows each format's convention, so it is asked rather than hard-coded — this
        // was the wave's second wrong result, after the section *name*.
        let segment = object
            .segment_name(object::write::StandardSegment::Debug)
            .to_vec();
        let section = object.add_section(segment, name.into_bytes(), object::SectionKind::Debug);
        object
            .section_mut(section)
            .set_data(writer.bytes.clone(), 1);
        added.push((section, writer));
        Ok(())
    })?;

    for (section, writer) in added {
        for reloc in &writer.relocs {
            object
                .add_relocation(
                    section,
                    object::write::Relocation {
                        offset: reloc.offset as u64,
                        symbol: symbols[reloc.symbol],
                        addend: reloc.addend,
                        flags: object::RelocationFlags::Generic {
                            kind: object::RelocationKind::Absolute,
                            encoding: object::RelocationEncoding::Generic,
                            size: reloc.size * 8,
                        },
                    },
                )
                // A relocation this crate constructed cannot be rejected for a reason a source program controls,
                // so this is an internal invariant rather than a user-facing failure.
                .map_err(|_| gimli::write::Error::InvalidAddress)?;
        }
    }

    Ok(())
}

/// Writes the type and subprogram DIEs into `unit` (ADR-0173 §2).
///
/// # Why the types come first
///
/// A subprogram's `DW_AT_type` and a member's are `UnitRef` attributes — a reference to another DIE by its id.
/// So every type must exist before anything points at one, which is why this makes two passes rather than
/// interleaving. gimli's ids are stable once handed out, so the first pass's `Vec` is the whole mapping.
fn write_types_and_subprograms(
    unit: &mut DwarfUnit,
    root: gimli::write::UnitEntryId,
    types: &TypeDescriptions,
    subprograms: &[FunctionSubprogram],
    symbols: &mut Vec<object::write::SymbolId>,
    frame_pointer: gimli::Register,
) {
    // Pass one: a DIE per description, children of the unit root.
    let mut ids = Vec::with_capacity(types.entries.len());
    for description in &types.entries {
        let tag = match description {
            TypeDescription::Base { .. } => gimli::DW_TAG_base_type,
            TypeDescription::Pointer { .. } => gimli::DW_TAG_pointer_type,
            TypeDescription::Struct { .. } => gimli::DW_TAG_structure_type,
        };
        ids.push(unit.unit.add(root, tag));
    }

    // Pass two: fill each in, now that every id exists and a reference can be resolved.
    for (position, description) in types.entries.iter().enumerate() {
        let id = ids[position];
        match description {
            TypeDescription::Base {
                name,
                size,
                encoding,
            } => {
                let die = unit.unit.get_mut(id);
                die.set(
                    gimli::DW_AT_name,
                    AttributeValue::String(name.as_bytes().to_vec()),
                );
                die.set(gimli::DW_AT_byte_size, AttributeValue::Udata(*size));
                die.set(gimli::DW_AT_encoding, AttributeValue::Encoding(*encoding));
            }
            TypeDescription::Pointer { size, pointee } => {
                let target = pointee.and_then(|at| ids.get(at).copied());
                let die = unit.unit.get_mut(id);
                die.set(gimli::DW_AT_byte_size, AttributeValue::Udata(*size));
                if let Some(target) = target {
                    die.set(gimli::DW_AT_type, AttributeValue::UnitRef(target));
                }
            }
            TypeDescription::Struct { size, members } => {
                unit.unit
                    .get_mut(id)
                    .set(gimli::DW_AT_byte_size, AttributeValue::Udata(*size));
                // **Anonymous**, for ADR-0171 §4's reason: the pool records no declared name, and faking one
                // from the `DeclId` would print a number no reader recognises.
                for (name, offset, member_type) in members {
                    let member = unit.unit.add(id, gimli::DW_TAG_member);
                    let target = ids.get(*member_type).copied();
                    let die = unit.unit.get_mut(member);
                    die.set(
                        gimli::DW_AT_name,
                        AttributeValue::String(name.as_bytes().to_vec()),
                    );
                    die.set(
                        gimli::DW_AT_data_member_location,
                        AttributeValue::Udata(*offset),
                    );
                    if let Some(target) = target {
                        die.set(gimli::DW_AT_type, AttributeValue::UnitRef(target));
                    }
                }
            }
        }
    }

    for subprogram in subprograms {
        // The same side table the line program's sequences use, appended to rather than duplicated: gimli
        // addresses a symbol by index into one list per writer, so a second list would resolve to the wrong
        // function.
        let symbol = symbols.len();
        symbols.push(subprogram.symbol);
        let id = unit.unit.add(root, gimli::DW_TAG_subprogram);
        let ret = subprogram.ret.and_then(|at| ids.get(at).copied());
        let die = unit.unit.get_mut(id);
        if let Some(name) = &subprogram.name {
            die.set(
                gimli::DW_AT_name,
                AttributeValue::String(name.as_bytes().to_vec()),
            );
        }
        // `low_pc` is a relocation against the function's symbol; `high_pc` is a *length*, which is DWARF 4's
        // form and avoids a second relocation. Getting these wrong makes every frame in a backtrace resolve to
        // the first function in the object.
        die.set(
            gimli::DW_AT_low_pc,
            AttributeValue::Address(Address::Symbol { symbol, addend: 0 }),
        );
        die.set(
            gimli::DW_AT_high_pc,
            AttributeValue::Udata(subprogram.length),
        );
        if subprogram.line > 0 {
            die.set(
                gimli::DW_AT_decl_line,
                AttributeValue::Udata(u64::from(subprogram.line)),
            );
        }
        if let Some(ret) = ret {
            die.set(gimli::DW_AT_type, AttributeValue::UnitRef(ret));
        }
        // Marked external, so a consumer treats it as a definition rather than a nested declaration.
        die.set(gimli::DW_AT_external, AttributeValue::Flag(true));

        if !subprogram.variables.is_empty() {
            // **The frame base is the frame-pointer register** (ADR-0174 §2), so each variable's location is
            // `DW_OP_fbreg <offset from FP>`. `DW_OP_call_frame_cfa` would be more idiomatic and needs
            // `.eh_frame`, which this compiler does not emit — so the CFA is not available to point at and the
            // register is the honest base.
            let mut base = gimli::write::Expression::new();
            base.op_reg(frame_pointer);
            unit.unit
                .get_mut(id)
                .set(gimli::DW_AT_frame_base, AttributeValue::Exprloc(base));

            for (name, ty, offset) in &subprogram.variables {
                let target = ids.get(*ty).copied();
                let variable = unit.unit.add(id, gimli::DW_TAG_variable);
                let mut location = gimli::write::Expression::new();
                location.op_fbreg(*offset);
                let die = unit.unit.get_mut(variable);
                die.set(
                    gimli::DW_AT_name,
                    AttributeValue::String(name.as_bytes().to_vec()),
                );
                die.set(gimli::DW_AT_location, AttributeValue::Exprloc(location));
                if let Some(target) = target {
                    die.set(gimli::DW_AT_type, AttributeValue::UnitRef(target));
                }
            }
        }
    }
}

/// The section name for `id` in `format`.
///
/// Mach-O names a debug section `__debug_line` in the `__DWARF` segment, ELF names it `.debug_line`. `object`
/// wants the platform's own spelling, and getting it wrong produces a section `dwarfdump` silently ignores —
/// which is indistinguishable from emitting nothing, and was this wave's first wrong result.
fn debug_section_name(id: SectionId, format: object::BinaryFormat) -> String {
    match format {
        object::BinaryFormat::MachO => format!("__{}", id.name().trim_start_matches('.')),
        _ => id.name().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::LineVocabulary;

    #[test]
    fn interning_the_same_position_twice_gives_one_row() {
        let mut vocabulary = LineVocabulary::default();
        let first = vocabulary
            .intern("a.jr", 3)
            .expect("a fresh vocabulary has room");
        let again = vocabulary
            .intern("a.jr", 3)
            .expect("a fresh vocabulary has room");
        assert_eq!(first, again, "the same position must reuse its index");
        assert_eq!(vocabulary.rows.len(), 1);
    }

    #[test]
    fn distinct_lines_and_distinct_files_are_distinct_rows() {
        let mut vocabulary = LineVocabulary::default();
        let a3 = vocabulary.intern("a.jr", 3).expect("room");
        let a4 = vocabulary.intern("a.jr", 4).expect("room");
        let b3 = vocabulary.intern("b.jr", 3).expect("room");
        assert_ne!(a3, a4, "a different line is a different row");
        assert_ne!(
            a3, b3,
            "the same line in a different file is a different row"
        );
        assert_eq!(vocabulary.row(a4), Some(&("a.jr".to_owned(), 4)));
    }

    #[test]
    fn the_path_table_is_deduplicated_in_first_seen_order() {
        let mut vocabulary = LineVocabulary::default();
        vocabulary.intern("b.jr", 1).expect("room");
        vocabulary.intern("a.jr", 1).expect("room");
        vocabulary.intern("b.jr", 2).expect("room");
        // First-seen, not sorted: the primary file of a single-file program must be index 0, and sorting
        // would make that depend on its name.
        assert_eq!(vocabulary.paths(), vec!["b.jr", "a.jr"]);
    }

    #[test]
    fn an_empty_vocabulary_is_reported_empty() {
        // A module with no positions must get no `.debug_line` at all: an empty line program is a section a
        // consumer must parse to learn nothing.
        assert!(LineVocabulary::default().is_empty());
    }
}
