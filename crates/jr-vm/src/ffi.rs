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
            let address = vm.memory_mut().allocate(size, 16)?;
            return Ok(Value::Scalar(address));
        }
        // `free` is a no-op: the VM's region is bump-allocated with no reclamation (the same model
        // its call frames use), so releasing is nothing to do. `free(null)` is defined and lands
        // here too. This means a long-running comptime allocator leaks within the VM, which the
        // memory bound turns into a diagnosable `Exhausted` rather than a fault.
        "free" if foreign.ret == PoolId::VOID => return Ok(Value::Void),
        _ => {}
    }

    let mut raw = Vec::with_capacity(args.len());
    for (value, ty) in args.iter().zip(&foreign.params) {
        raw.push(marshal(vm, value, *ty)?);
    }

    dispatch(vm, foreign, &raw)
}

/// Whether a foreign procedure's declared return type is a pointer.
///
/// Guards the `malloc` interception so a *different* `#foreign` procedure a program happens to name
/// `malloc` — with a non-pointer return — is not silently rerouted to the VM's allocator.
fn is_pointer_return(vm: &Vm<'_>, foreign: &ForeignProc) -> bool {
    matches!(vm.pool().item(foreign.ret), Item::PointerType(_))
}

/// One argument, reduced to the machine word the C ABI passes.
///
/// Every type the slice's `#foreign` declarations use — `s64`, `*u8` — is one word,
/// so a `u64` per argument is the whole marshalling story. An aggregate argument is
/// refused rather than flattened: the arm64 and x86-64 rules for passing a struct by
/// value differ from each other and from the naive "one word per field", so guessing
/// would produce a call that works on one platform and corrupts the stack on the
/// other. `to_c_string()` and by-value aggregates arrive with wave W3.
fn marshal(vm: &Vm<'_>, value: &Value, ty: PoolId) -> Result<u64, VmError> {
    let pool = vm.pool();
    match pool.item(ty) {
        Item::IntType { .. } => {
            let kind = IntKind::of(pool, ty).unwrap_or(IntKind::S64);
            // Sign-extend to a full word: the C ABI passes a narrow signed integer
            // sign-extended, and `Value::Scalar` holds it width-normalised.
            Ok(value.as_int(kind)? as u64)
        }
        Item::BoolType => Ok(u64::from(value.boolean()?)),
        Item::PointerType(_) => {
            let address = value.scalar()?;
            if address == 0 {
                // A null pointer is a legitimate C argument, and `host_pointer` would
                // refuse it, so it is passed through rather than translated.
                return Ok(0);
            }
            let host = vm.memory().host_pointer(address, 1)?;
            Ok(host as u64)
        }
        other => Err(VmError::unsupported(format!(
            "passing {other:?} to a foreign procedure arrives with a later wave"
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
fn dispatch(vm: &mut Vm<'_>, foreign: &ForeignProc, args: &[u64]) -> Result<Value, VmError> {
    if foreign.symbol == "exit" {
        let status = args.first().copied().unwrap_or(0);
        return Err(VmError::Exited(status as i64));
    }

    // Capture what a write would produce before making the call, so a test can assert
    // on a program's output without capturing the process's own stdout.
    if foreign.symbol == "write"
        && let [_, buf, count] = *args
    {
        let count = usize::try_from(count).unwrap_or(0);
        if buf != 0 && count != 0 {
            // SAFETY: `marshal` produced `buf` from `Memory::host_pointer`, which
            // bounds-checked the address inside the VM's non-moving region, and
            // nothing has allocated or released since.
            let bytes = unsafe { core::slice::from_raw_parts(buf as *const u8, count) };
            vm.capture(bytes);
        }
    }

    let code = symbol(foreign)?;
    let signature = Cif::new(
        args.iter().map(|_| Type::u64()).collect::<Vec<_>>(),
        return_type(vm, foreign)?,
    );
    let cell: Vec<Arg> = args.iter().map(arg).collect();

    if foreign.ret == PoolId::VOID {
        // SAFETY: `code` came from `dlsym` for the symbol this declaration names, and
        // the CIF describes exactly as many word-sized arguments as `cell` holds. The
        // declaration is the only description of the callee that exists — `jr-sema`
        // verified it is well-formed, and ADR-0006 accepts that a wrong `#foreign`
        // declaration is undefined behaviour, which is the same bargain C makes.
        unsafe { signature.call::<()>(code, &cell) };
        return Ok(Value::Void);
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
