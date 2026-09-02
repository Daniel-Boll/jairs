//! The foreign-call bridge: a Jairs address becomes a host pointer, and libc gets called.
//!
//! # What ADR-0006 actually asked for, and what is here
//!
//! ADR-0006 decided that compile-time code **may** call foreign functions, behind an
//! explicit `#foreign_at_comptime` allowance, and recorded the consequence: "the
//! VM's architecture must accommodate a libffi-style dynamic-call path". ADR-0018 §4
//! makes that concrete and adds the distinction ADR-0006 draws but had nowhere to
//! draw:
//!
//! - a foreign call while **running a program** (`jr run`) is the program working;
//! - a foreign call while **evaluating comptime code** (`#run`) is refused until wave
//!   W6 introduces the allowance.
//!
//! The mode check lives in [`crate::Mode`] and is applied by the interpreter before
//! anything here runs, so this module never has to remember it.
//!
//! # Why the signature comes from the pool, not from libffi
//!
//! A dynamic call needs each argument's ABI class. `jr-sema` already typed the
//! declaration, so [`ForeignProc::params`] is a list of `PoolId`s and `jr-pool`'s
//! layout says how wide each one is. Nothing is guessed from the value: a `*u8`
//! argument is passed as a pointer because the *declaration* says `*u8`, which is
//! what makes `write(fd, buf, count)` correct rather than accidentally correct.
//!
//! # Why a Jairs pointer can be handed straight out
//!
//! `Memory` is one non-moving region (see its module docs), so a Jairs address is an
//! offset and [`Memory::host_pointer`] turns it into a real, bounds-checked address
//! in this process. `024-hello.jr`'s `print` therefore hands `write` a pointer to the
//! actual bytes of `"hello from Jairs\n"` with no copy and no marshalling — which is
//! ADR-0004's stated payoff, that Jairs strings are already the `(pointer, length)`
//! shape `write(2)` wants.
//!
//! # What that bounds check does and does not cover
//!
//! [`marshal`] validates a pointer argument for **one byte**, because one byte is all it
//! knows the callee will touch: a C signature does not say how far a `*u8` reaches. So the
//! check confirms the pointer is *inside* the region and nothing more.
//!
//! Where the VM **itself** dereferences a span it must validate that span, and
//! [`capture_write`] does — an over-long `write` is `Trap::BadAddress` rather than a read
//! past the end of the region's `Vec<u8>`. That distinction is the whole of ADR-0126.
//!
//! **Still owed, and stated rather than implied**: a foreign callee that reads further than
//! one byte through a pointer the VM handed it — `strlen` on an unterminated buffer at the
//! region's end, a `memcpy` whose length outruns its source — reads outside the region, and
//! `Memory`'s module docs call that the same hazard native code has. Bounding it needs the
//! length to come from somewhere, and the candidates are a per-symbol table of `(pointer,
//! count)` shapes — the token-set trap this project has counted seven bugs from (ADR-0124) —
//! or a real sandbox that copies in and out, which would cost ADR-0004's zero-copy payoff
//! above. Neither is worth deciding in passing, so the honest position is that the region
//! bounds the VM and does not bound libc.
//!
//! # Why `#foreign` is not yet resolved through a real `dlopen`
//!
//! Every symbol the slice needs — `write`, `exit` — is already linked into this
//! process, because the compiler itself is a C-linking Rust binary. So resolution
//! goes through the process's own symbol table. `ForeignInfo::library` is *still* an
//! unresolved `Option<Symbol>` in the HIR (`jr-sema` checks it names a library for
//! E0225 and records nothing), so the library name is carried but only *checked*
//! here, not used to load anything: naming a library the process has not linked is
//! refused rather than silently resolved against whatever else is loaded.
//! ADR-0018 §4 records that a third independent resolution of the same declaration is
//! the signal to intern the answer beside `Item::ForeignLibraryValue`.

use jr_pool::{Item, PoolId};
use libffi::middle::{Arg, Cif, CodePtr, Type, arg};
use libloading::os::unix::Library as LibraryHandle;

use crate::code::ForeignProc;
use crate::error::VmError;
use crate::interp::Vm;
use crate::value::{IntKind, Value};

/// Performs a foreign call.
///
/// # Errors
/// [`VmError::Unsupported`] for a symbol or a signature the bridge cannot handle,
/// [`VmError::Internal`] when the argument count or shapes disagree with the
/// declaration, and [`VmError::Trap`] when an argument names memory the VM does not
/// own.
pub(crate) fn call(
    vm: &mut Vm<'_>,
    foreign: &ForeignProc,
    args: &[Value],
) -> Result<Value, VmError> {
    if args.len() != foreign.params.len() {
        return Err(VmError::internal(format!(
            "`{}` takes {} arguments, called with {}",
            foreign.symbol,
            foreign.params.len(),
            args.len()
        )));
    }

    // `malloc` and `free` are satisfied from the VM's **own** linear region rather than the host
    // (ADR-0061 §1). A Jairs pointer is an offset into that region, bounds-checked on every
    // dereference; a raw host `malloc` address is not such an offset, so a byte written through it
    // would fail the check that keeps the VM a sandbox. Intercepting here means a comptime-adjacent
    // runtime `malloc` returns memory the VM can actually read and write, and the native back end
    // still calls libc — the two engines' pointer *bits* differ, which nothing observes, while the
    // byte round-trip agrees.
    match foreign.symbol.as_str() {
        "malloc" if is_pointer_return(vm, foreign) => {
            let size = args
                .first()
                .map_or(0, |v| v.as_int(IntKind::S64).unwrap_or(0)) as u64;
            // 16-byte alignment, matching what libc `malloc` guarantees, so a program that assumes
            // it holds in the VM too is not surprised. Zero size still yields a usable pointer.
            // **From the heap region**, not the frame bump (ADR-0107 §2). `malloc` memory must outlive the
            // call that allocated it — a procedure whose job is to allocate and return the pointer is the
            // ordinary case — and a frame release would otherwise reclaim it, so a heap write inside a callee
            // read back as zero in the caller while the native back end (calling libc) was right. The two
            // engines disagreed, which the corpus differential caught.
            let address = vm.memory_mut().allocate_heap(size, 16)?;
            return Ok(Value::Scalar(address));
        }
        // `free` is a no-op: the VM's region is bump-allocated with no reclamation (the same model
        // its call frames use), so releasing is nothing to do. `free(null)` is defined and lands
        // here too. This means a long-running comptime allocator leaks within the VM, which the
        // memory bound turns into a diagnosable `Exhausted` rather than a fault.
        "free" if foreign.ret == PoolId::VOID => return Ok(Value::Void),
        _ => {}
    }

    // Capture what a `write` will produce, **before** marshalling and from the Jairs address
    // rather than from a host pointer — see [`capture_write`] for why that ordering is the fix
    // and not a detail.
    if foreign.symbol == "write" {
        capture_write(vm, args)?;
    }

    let mut raw = Vec::with_capacity(args.len());
    for (value, ty) in args.iter().zip(&foreign.params) {
        raw.push(marshal(vm, value, *ty)?);
    }

    dispatch(vm, foreign, &raw)
}

/// Records what a `write` call is about to produce, refusing a span the VM does not own.
///
/// # Why this reads the Jairs address instead of the host pointer
///
/// The bytes have to be validated over **`count`**, and only the Jairs address can be: the
/// bound is a property of the VM's region, so the check belongs to [`Memory::read`], which is
/// the same check every other access goes through. Doing it here — before [`marshal`] — is
/// what makes that possible, because after marshalling only a raw host pointer survives and
/// nothing can bound one.
///
/// This replaces a `slice::from_raw_parts(buf, count)` over a pointer [`marshal`] had
/// validated **one byte** of. That is not a narrow miss: `count` is the program's own value,
/// so `write(1, s.data, 4_000_000)` on a two-byte string read ~3 MB past the end of the
/// region's `Vec<u8>` and captured it as the program's output, and a count of 2e9 killed the
/// compiler with `SIGBUS`. The `unsafe` block's comment asserted the address had been
/// "bounds-checked", which was true only at one byte — a stated invariant nobody had checked,
/// the failure mode `AGENTS.md` names.
///
/// It also removes the `unsafe` rather than fixing it: [`Memory::read`] hands back a safe
/// `&[u8]`, so the span is bounded *by construction* instead of by a comment. Copying it to
/// own the bytes costs one memcpy that `Vm::capture` was doing anyway.
///
/// An over-long count is `Trap::BadAddress`, not a new diagnostic: passing a count past the
/// end of a buffer is a program error exactly as an out-of-range index is (ADR-0003), and it
/// already has the right trap. Refusing **before** the call also keeps the bogus `(pointer,
/// count)` pair away from the real `write(2)`, so the trap fixes the VM's own undefined
/// behaviour and the host call in one place.
///
/// The file descriptor is deliberately ignored, exactly as before: whether a `write` to
/// `STDERR` belongs in `captured_output` is a separate question, and answering it here would
/// change what every existing test observes.
fn capture_write(vm: &mut Vm<'_>, args: &[Value]) -> Result<(), VmError> {
    let [_, buf, count] = args else {
        // A program may declare `write` with some other arity; it is then not the one whose
        // output is captured, and the call is left to the bridge as any other.
        return Ok(());
    };
    let address = buf.scalar()?;
    let count = count.as_int(IntKind::S64)?;
    // A null buffer or a non-positive count produces nothing, and a *negative* count is
    // skipped rather than trapped so that this reports what the previous `usize::try_from`
    // did — the fix is the missing bound, not a new refusal.
    if address == 0 || count <= 0 {
        return Ok(());
    }
    let bytes = vm.memory().read(address, count as u64)?.to_vec();
    vm.capture(&bytes);
    Ok(())
}

/// Whether a foreign procedure's declared return type is a pointer.
///
/// Guards the `malloc` interception so a *different* `#foreign` procedure a program happens to name
/// `malloc` — with a non-pointer return — is not silently rerouted to the VM's allocator.
fn is_pointer_return(vm: &Vm<'_>, foreign: &ForeignProc) -> bool {
    matches!(vm.pool().item(foreign.ret), Item::PointerType(_))
}

/// One argument, ready for libffi.
///
/// Two shapes rather than one word, because an **aggregate passed by value** is its bytes and not an address
/// (ADR-0160 part 2). Keeping them apart in the type means `dispatch` cannot accidentally hand libffi a
/// pointer where the C signature says struct — which is the shape of mistake that produces a call that works
/// on one platform and corrupts the stack on the other.
#[derive(Debug, Clone)]
enum Marshalled {
    /// A scalar: an integer, a `bool`, a pointer, or a float's bits.
    Word(u64),
    /// An aggregate's bytes, in target layout. Owned, because libffi borrows the address across the call.
    Bytes(Vec<u8>),
}

/// The libffi description of `ty`, faithful field by field.
///
/// # Why this describes the struct rather than consulting `jr_pool::classify`
///
/// **libffi implements the C ABI itself.** Given a true description of the struct it places the pieces
/// correctly, including the mixed-register cases `Class::Memory` refuses — so the VM's correct move is to
/// describe and delegate, not to classify and then act on the classification. The two native back ends have
/// no such helper and *must* consult `classify`, which is why that function exists; this engine gets the same
/// answer for free and would keep getting it if the refusal were ever relaxed.
///
/// Faithful means the real widths: a `u8` field is `Type::u8()`, not a word. Describing every field as a word
/// would give libffi the wrong size and alignment for the struct, and on a `{u8, u8}` it would claim eight
/// bytes where C says two.
///
/// `None` for a type libffi cannot be told about — a `void` field, a procedure type — which the caller turns
/// into the same refusal an unsupported scalar gets.
fn ffi_type_of(pool: &jr_pool::Pool, ty: PoolId) -> Option<Type> {
    match pool.item(ty) {
        Item::BoolType => Some(Type::u8()),
        Item::PointerType(_) => Some(Type::pointer()),
        Item::IntType { bits, .. } => match bits {
            8 => Some(Type::u8()),
            16 => Some(Type::u16()),
            32 => Some(Type::u32()),
            _ => Some(Type::u64()),
        },
        Item::FloatType { bits: 32 } => Some(Type::f32()),
        Item::FloatType { .. } => Some(Type::f64()),
        Item::StructType { .. } => {
            let fields = pool.fields_of(ty)?;
            let types: Vec<PoolId> = fields.iter().map(|field| field.ty).collect();
            let described: Option<Vec<Type>> = types
                .iter()
                .map(|field| ffi_type_of(pool, *field))
                .collect();
            Some(Type::structure(described?))
        }
        Item::ArrayType { elem, len } => {
            // An array is a struct of `len` identical members to a C ABI — which is what makes `float64[4]`
            // an HFA exactly as a four-field struct is (ADR-0160 §2).
            let elem = *elem;
            let len = usize::try_from(*len).ok()?;
            let member = ffi_type_of(pool, elem)?;
            let members: Vec<Type> = (0..len).map(|_| member.clone()).collect();
            Some(Type::structure(members))
        }
        // A `string`, a view and a dynamic array are compiler-defined aggregates of words, so they are
        // described as the `{pointer, count}` pairs they are rather than walked as fields they do not have.
        Item::StringType | Item::ViewType { .. } => {
            Some(Type::structure(vec![Type::pointer(), Type::u64()]))
        }
        _ => None,
    }
}

fn marshal(vm: &Vm<'_>, value: &Value, ty: PoolId) -> Result<Marshalled, VmError> {
    let pool = vm.pool();
    match pool.item(ty) {
        Item::IntType { .. } => {
            let kind = IntKind::of(pool, ty).unwrap_or(IntKind::S64);
            // Sign-extend to a full word: the C ABI passes a narrow signed integer
            // sign-extended, and `Value::Scalar` holds it width-normalised.
            Ok(Marshalled::Word(value.as_int(kind)? as u64))
        }
        // **A float is marshalled to its bits, and `dispatch` passes it in a float register** (ADR-0114 §1).
        // The bits are stored width-normalised in `Value::Scalar` already; keying the libffi arg type on the
        // parameter is what makes libffi place it in `xmm0`/`d0` rather than an integer register, which every
        // real ABI requires and which passing the bits as a `u64` would get wrong — silently, since the callee
        // would read a float register that was never written.
        Item::FloatType { .. } => Ok(Marshalled::Word(value.scalar()?)),
        Item::BoolType => Ok(Marshalled::Word(u64::from(value.boolean()?))),
        Item::PointerType(_) => {
            let address = value.scalar()?;
            if address == 0 {
                // A null pointer is a legitimate C argument, and `host_pointer` would
                // refuse it, so it is passed through rather than translated.
                return Ok(Marshalled::Word(0));
            }
            let host = vm.memory().host_pointer(address, 1)?;
            Ok(Marshalled::Word(host as u64))
        }
        // **An aggregate passed by value is its bytes** (ADR-0160 part 2). `Value::Aggregate` already holds
        // them in target layout, so there is nothing to read out of the VM's memory region and nothing to
        // translate — which is also why this direction has none of the nested-pointer problem ADR-0158 §3
        // found: the bytes are copied, and a pointer *inside* them would still be region-relative. A struct of
        // pointers therefore stays wrong across this boundary, and `jr-sema` is where that is refused.
        Item::StructType { .. }
        | Item::ArrayType { .. }
        | Item::StringType
        | Item::ViewType { .. } => {
            let Value::Aggregate(bytes) = value else {
                return Err(VmError::unsupported(String::from(
                    "an aggregate argument arrived as a scalar, which means the caller and the signature \
                     disagree about its type",
                )));
            };
            Ok(Marshalled::Bytes(bytes.clone()))
        }
        // **A procedure value cannot cross this boundary, and never will** (ADR-0175 §4). C wants a
        // machine address to call; this interpreter executes bytecode and there *is* no machine code for
        // a Jairs procedure, so there is no address to hand over. That is a property of interpreting, not
        // a wave that has not happened — the previous message said "arrives with a later wave", which
        // promised something unreachable without a JIT.
        //
        // This is what makes a compile-time `pthread_create` impossible: every spawn needs a function
        // pointer for the thread body, so the refusal lands here rather than at any thread API.
        Item::ProcType { .. } | Item::ProcValue { .. } => Err(VmError::unsupported(String::from(
            "a procedure cannot be passed to a `#foreign` procedure at compile time: C needs a machine \
             address to call and the compile-time interpreter has no machine code to point at",
        ))),
        // Everything else is a genuine gap rather than an impossibility, so it still says so — and it
        // names the *type* rather than dumping a `Debug` rendering of the pool item, which is what this
        // arm used to do and is unreadable in a diagnostic.
        _ => Err(VmError::unsupported(String::from(
            "this argument's type cannot be passed to a `#foreign` procedure yet",
        ))),
    }
}

/// Calls the symbol through libffi, and turns its result back into a [`Value`].
///
/// # Why `exit` never reaches libffi
///
/// Calling the host `exit` would terminate the **compiler**, taking pending
/// diagnostics and any `jr run` bookkeeping with it — and under `#run` it would end
/// the build partway through. It becomes [`VmError::Exited`] instead, which the CLI
/// turns into the process exit status. This is the one symbol whose C behaviour the
/// VM deliberately does not reproduce, and the reason is that the VM is a *guest*
/// inside a process that has other work to finish.
fn dispatch(vm: &mut Vm<'_>, foreign: &ForeignProc, args: &[Marshalled]) -> Result<Value, VmError> {
    if foreign.symbol == "exit" {
        let status = match args.first() {
            Some(Marshalled::Word(word)) => *word,
            _ => 0,
        };
        return Err(VmError::Exited(status as i64));
    }

    let code = symbol(foreign)?;
    // **Per-argument libffi types** (ADR-0114 §1). An integer or a pointer is a word (`Type::u64`); a float is
    // `Type::f32`/`Type::f64`, which is what places it in a float register. The float *values* are decoded into
    // `floats`, kept alive for the duration of the call, because `libffi::arg` borrows its operand — a
    // temporary would dangle before `signature.call`.
    let pool = vm.pool();
    // **Per-argument libffi types.** An aggregate is described faithfully so libffi places its pieces itself
    // (ADR-0160 part 2); a float is `f32`/`f64`, which is what puts it in a float register; everything else is
    // a word.
    let mut arg_types: Vec<Type> = Vec::with_capacity(foreign.params.len());
    for ty in &foreign.params {
        if matches!(
            pool.item(*ty),
            Item::StructType { .. }
                | Item::ArrayType { .. }
                | Item::StringType
                | Item::ViewType { .. }
        ) {
            let described = ffi_type_of(pool, *ty).ok_or_else(|| {
                VmError::unsupported(format!(
                    "an aggregate parameter of {:?} cannot be described to the C ABI",
                    pool.item(*ty)
                ))
            })?;
            arg_types.push(described);
            continue;
        }
        arg_types.push(match jr_pool::FloatKind::of(pool, *ty) {
            Some(k) if k.bits == 32 => Type::f32(),
            Some(_) => Type::f64(),
            None => Type::u64(),
        });
    }
    let mut floats32: Vec<f32> = Vec::new();
    let mut floats64: Vec<f64> = Vec::new();
    // Where each argument's operand lives, so `cell` can take its address. A float has to be *decoded* into a
    // width-appropriate store first, because `libffi::arg` borrows its operand and a temporary would dangle
    // before the call; an aggregate's bytes are already owned by `args`, so its arm carries no store.
    enum Cell {
        /// The word in `args` at this position.
        Word,
        /// `floats32[index]`.
        Float32(usize),
        /// `floats64[index]`.
        Float64(usize),
        /// The bytes in `args` at this position.
        Bytes,
    }
    let mut plan: Vec<Cell> = Vec::with_capacity(args.len());
    for (value, ty) in args.iter().zip(foreign.params.iter()) {
        match value {
            Marshalled::Bytes(_) => plan.push(Cell::Bytes),
            Marshalled::Word(word) => match jr_pool::FloatKind::of(pool, *ty) {
                Some(k) if k.bits == 32 => {
                    floats32.push(k.decode(*word) as f32);
                    plan.push(Cell::Float32(floats32.len() - 1));
                }
                Some(k) => {
                    floats64.push(k.decode(*word));
                    plan.push(Cell::Float64(floats64.len() - 1));
                }
                None => plan.push(Cell::Word),
            },
        }
    }
    let signature = Cif::new(arg_types, return_type(vm, foreign)?);
    let cell: Vec<Arg> = args
        .iter()
        .zip(plan.iter())
        .map(|(value, where_it_is)| match (value, where_it_is) {
            (Marshalled::Word(word), Cell::Word) => arg(word),
            (_, Cell::Float32(index)) => arg(&floats32[*index]),
            (_, Cell::Float64(index)) => arg(&floats64[*index]),
            // **The address of the first byte**, which is what libffi wants for a struct: it reads the
            // described type's size from there. An empty aggregate has no first byte, so it gets the address
            // of the vector itself — libffi will read zero bytes from it, and a null would be a different
            // argument entirely.
            (Marshalled::Bytes(bytes), Cell::Bytes) => {
                bytes.first().map_or_else(|| arg(bytes), arg)
            }
            // Unreachable: `plan` was built from `args` in the same order, so the two always agree. Answered
            // rather than panicking, because a panic inside the compiler is worse than a wrong argument the
            // signature check already prevented.
            (Marshalled::Bytes(bytes), Cell::Word) => arg(bytes),
            (Marshalled::Word(word), Cell::Bytes) => arg(word),
        })
        .collect();

    if foreign.ret == PoolId::VOID {
        // SAFETY: `code` came from `dlsym` for the symbol this declaration names, and
        // the CIF describes exactly as many word-sized arguments as `cell` holds. The
        // declaration is the only description of the callee that exists — `jr-sema`
        // verified it is well-formed, and ADR-0006 accepts that a wrong `#foreign`
        // declaration is undefined behaviour, which is the same bargain C makes.
        unsafe { signature.call::<()>(code, &cell) };
        return Ok(Value::Void);
    }

    // **A float return comes back through a float register** (ADR-0114 §1), so it must be read as `f32`/`f64`
    // rather than as a word — `signature.call::<u64>` would read an integer register the callee never wrote.
    // Re-encoded to the declared width's bits, the inverse of the argument path.
    if let Some(kind) = jr_pool::FloatKind::of(vm.pool(), foreign.ret) {
        // SAFETY: as the integer path below. The CIF's return type is `f32`/`f64` (from `return_type`), so
        // reading the matching Rust type is exactly what libffi placed there.
        if kind.bits == 32 {
            let r = unsafe { signature.call::<f32>(code, &cell) };
            return Ok(Value::Scalar(kind.encode(f64::from(r))));
        }
        let r = unsafe { signature.call::<f64>(code, &cell) };
        return Ok(Value::Scalar(kind.encode(r)));
    }

    // **An aggregate return comes back as bytes** (ADR-0160 part 2), read into a buffer big enough for every
    // shape the classification permits: four eight-byte members is the largest, so thirty-two bytes.
    //
    // `#[repr(C, align(16))]` matters. `libffi::low::call` writes into a `MaybeUninit<R>` directly whenever
    // `size_of::<R>()` is at least a word, and a returned struct is stored from registers — so the buffer has
    // to be aligned for the widest member. A bare `[u8; 32]` is one-aligned and would be undefined behaviour
    // on a target that requires alignment for the store.
    if let Some(class) = jr_pool::classify(pool, jr_pool::TargetLayout::host(), foreign.ret)
        .map_err(|error| VmError::unsupported(format!("{error}")))?
    {
        if class == jr_pool::Class::Memory {
            return Err(VmError::unsupported(String::from(
                "returning this aggregate from a `#foreign` procedure needs a register class this \
                 compiler does not implement",
            )));
        }
        let layout = jr_pool::layout_of(pool, jr_pool::TargetLayout::host(), foreign.ret)
            .map_err(|error| VmError::unsupported(format!("{error}")))?;
        #[repr(C, align(16))]
        #[derive(Clone, Copy)]
        struct ReturnBuffer([u8; 32]);
        // SAFETY: as the paths above — the declaration is the only description of the callee that exists, and
        // ADR-0006 accepts that a wrong `#foreign` declaration is undefined behaviour. The CIF's return type
        // is the faithful struct description from `return_type`, whose size is at most thirty-two bytes
        // because `classify` answered a register class; every bit pattern is a valid `[u8; 32]`.
        let returned = unsafe { signature.call::<ReturnBuffer>(code, &cell) };
        let size = usize::try_from(layout.size).unwrap_or(0).min(32);
        return Ok(Value::Aggregate(returned.0[..size].to_vec()));
    }

    // SAFETY: as above. The result is read as a full word and then narrowed by the
    // declared return type, rather than being read at the declared width, so that a
    // callee returning `int` into an `s64` declaration does not read three bytes of
    // adjacent register.
    let raw = unsafe { signature.call::<u64>(code, &cell) };

    let pool = vm.pool();
    match IntKind::of(pool, foreign.ret) {
        Some(kind) => Ok(Value::Scalar(kind.wrap(kind.decode(raw)))),
        // A **pointer** return is the raw word unchanged (ADR-0060 §2): a pointer is one machine
        // word and its bits are the address, so there is nothing to narrow — the same way the
        // native back end treats `malloc`'s `-> *u8`. `IntKind::of` answers `None` for a pointer
        // type, so this arm is what a pointer return needs and `return_type` above already accepts
        // one; without it a `malloc` binding refused at run time while `return_type` said it was
        // callable — the two disagreeing about the same declaration.
        None if matches!(pool.item(foreign.ret), Item::PointerType(_)) => Ok(Value::Scalar(raw)),
        None => Err(VmError::unsupported(format!(
            "a foreign procedure returning {:?} arrives with a later wave",
            pool.item(foreign.ret)
        ))),
    }
}

/// The libffi description of a foreign procedure's return type.
fn return_type(vm: &Vm<'_>, foreign: &ForeignProc) -> Result<Type, VmError> {
    let pool = vm.pool();
    if foreign.ret == PoolId::VOID {
        return Ok(Type::void());
    }
    match pool.item(foreign.ret) {
        Item::IntType { .. } | Item::BoolType | Item::PointerType(_) => Ok(Type::u64()),
        // A float return is described to libffi as `f32`/`f64` so it reads the float register (ADR-0114 §1).
        Item::FloatType { bits: 32 } => Ok(Type::f32()),
        Item::FloatType { .. } => Ok(Type::f64()),
        // An aggregate return is described faithfully, exactly as an aggregate *argument* is, so libffi
        // decides between registers and a hidden pointer itself (ADR-0160 part 2).
        Item::StructType { .. }
        | Item::ArrayType { .. }
        | Item::StringType
        | Item::ViewType { .. } => ffi_type_of(pool, foreign.ret).ok_or_else(|| {
            VmError::unsupported(format!(
                "an aggregate return of {:?} cannot be described to the C ABI",
                pool.item(foreign.ret)
            ))
        }),
        other => Err(VmError::unsupported(format!(
            "a foreign procedure returning {other:?} arrives with a later wave"
        ))),
    }
}

/// Resolves a `#foreign` declaration to a callable address.
///
/// Looks in the compiler's own process, which is where every symbol the slice needs
/// already lives: `jr` is a Rust binary linked against the C library, so `write` and
/// friends are present without loading anything. A declaration that names a library
/// other than `"c"` is refused rather than resolved this way, because finding
/// `write` in the process while the program asked some other library for it would be
/// a wrong answer dressed as a working one.
///
/// `ForeignInfo::library` is still an unresolved `Option<Symbol>` in the HIR —
/// `jr-sema` checks it names a library for E0225 and records nothing — so this is the
/// second independent resolution of the same declaration. ADR-0018 §4 records that a
/// third is the signal to intern the answer beside `Item::ForeignLibraryValue`.
fn symbol(foreign: &ForeignProc) -> Result<CodePtr, VmError> {
    if let Some(library) = &foreign.library
        && library != "c"
    {
        return Err(VmError::unsupported(format!(
            "`#system_library \"{library}\"` cannot be loaded yet; only \"c\" is available"
        )));
    }

    // SAFETY: `Library::this` takes a handle to the already-loaded process image and
    // does not run any initialiser; `get` is unsafe because it cannot check the type
    // of what it finds, which is exactly what the `#foreign` declaration asserts.
    let found = unsafe {
        let this = LibraryHandle::this();
        this.get::<*const ()>(foreign.symbol.as_bytes())
            .map(|symbol| *symbol)
    };

    match found {
        Ok(address) if !address.is_null() => Ok(CodePtr(address.cast_mut().cast())),
        _ => Err(VmError::unsupported(format!(
            "the foreign symbol `{}` was not found in this process",
            foreign.symbol
        ))),
    }
}
