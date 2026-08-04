//! The VM's byte-addressed memory: one non-moving region, addressed by offset.
//!
//! # Why a linear region rather than a Rust allocation per slot
//!
//! ADR-0018 §4 puts a libffi bridge in the VM, and `PLAN.md` §1.4's exit criterion
//! goes through it: `024-hello.jr` calls `print`, which hands `s.data` to libc
//! `write`. That pointer has to be a **real address in this process**, because
//! `write` will dereference it. So a Jairs pointer must be translatable to a host
//! pointer, and the cheapest correct way to do that is one contiguous region whose
//! base never moves — a Jairs address is an offset into it, and translation is
//! `base + offset` with a bounds check.
//!
//! This is wasmtime's linear memory, for the same reason: a sandbox whose contents
//! can nonetheless be handed to native code needs stable addresses and a
//! bounds-checked translation, not a map of separate allocations.
//!
//! # Why it never grows
//!
//! The region is allocated once at its full size and never reallocated. Growing it
//! would move the base, and a host pointer handed to a foreign function during an
//! outstanding call would dangle. Bounding the size instead turns exhaustion into
//! [`VmError::Exhausted`] — a diagnosable compiler limit — rather than into a
//! use-after-free that reproduces once a month. Comptime evaluation in the slice is
//! `#run add(2, 3)`, so the bound is not close to binding.
//!
//! # Why address 0 is reserved
//!
//! Nothing is ever allocated at offset 0, so 0 is available as the null pointer.
//! `jr-mir`'s `zero_value` records the gap this closes: the pool interns no null
//! pointer, so a default-initialised pointer local is currently treated as
//! uninitialised. When it is interned, this is the address it means.
//!
//! # Why frames are a stack mark, not a free list
//!
//! Slots are bump-allocated, and a call records the high-water mark on entry and
//! restores it on return. That makes a loop containing a call reuse the same bytes
//! instead of leaking one frame per iteration, without any of the machinery a real
//! allocator needs. The cost is the usual one: a pointer to a local that outlives
//! its frame dangles, which the source language already calls a bug and which the
//! bounds check turns into [`crate::Trap::BadAddress`] rather than into someone
//! else's data — unless the bytes have since been reused, which is exactly the
//! hazard native code has too.

use jr_pool::align_up;

use crate::error::{Trap, VmError};
use crate::value::Address;

/// The default size of the VM's memory region, in bytes.
///
/// One mebibyte. Chosen as "obviously enough for the slice, obviously bounded":
/// comptime evaluation today is integer arithmetic and a handful of string
/// literals. It is a constant rather than a tuning knob because nothing has asked
/// for one; the day something does, [`Memory::with_capacity`] already takes it.
pub const DEFAULT_CAPACITY: usize = 1 << 20;

/// A saved allocation high-water mark, restored when a call frame is released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark(u64);

/// The VM's memory.
#[derive(Debug)]
pub struct Memory {
    /// The bytes. Allocated once; the base address never moves.
    bytes: Vec<u8>,
    /// The next free offset. Starts at 1 so that 0 is null.
    next: u64,
    /// The heap's low-water mark: `malloc` allocates **downward** from here (ADR-0107 §2).
    ///
    /// A separate cursor because frames restore `next` on return while heap memory must outlive the call that
    /// allocated it — sharing one made a heap write inside a callee read back as zero in the caller, a
    /// disagreement between the two engines that the corpus differential caught. The two regions meet in the
    /// middle, and either running into the other is `Exhausted`.
    heap_next: u64,
}

impl Memory {
    /// A region of [`DEFAULT_CAPACITY`] bytes.
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// A region of `capacity` bytes.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            // Zero-filled, which is also what makes a freshly allocated slot read as
            // zero. That is not load-bearing — `jr-mir` emits an explicit store for
            // a default-initialised local, and `---` deliberately emits none — but a
            // deterministic zero beats whatever the allocator last left there when a
            // bug does read an unwritten slot.
            bytes: vec![0; capacity.max(1)],
            next: 1,
            // The heap starts at the top and grows down, so it begins empty.
            heap_next: capacity.max(1) as u64,
        }
    }

    /// How many bytes are in use.
    #[must_use]
    pub const fn used(&self) -> u64 {
        self.next
    }

    /// The current high-water mark, to be restored by [`Self::release`].
    #[must_use]
    pub const fn mark(&self) -> Mark {
        Mark(self.next)
    }

    /// Releases everything allocated since `mark`.
    ///
    /// The bytes are zeroed on release rather than merely forgotten. That costs a
    /// memset per frame and buys determinism: a dangling pointer into a released
    /// frame then reads zero every time instead of reading whatever the previous
    /// occupant left, which is the difference between a reproducible bug and a
    /// heisenbug.
    pub fn release(&mut self, mark: Mark) {
        let from = usize::try_from(mark.0).unwrap_or(usize::MAX);
        let to = usize::try_from(self.next).unwrap_or(usize::MAX);
        if let Some(range) = self.bytes.get_mut(from..to) {
            range.fill(0);
        }
        self.next = mark.0;
    }

    /// Allocates `size` bytes aligned to `align`, returning the address.
    ///
    /// A zero-sized allocation still returns a distinct usable address, because a
    /// zero-sized value has to have one — `void` is a real type (ADR-0015 §3) and
    /// may be stored.
    ///
    /// # Errors
    /// [`VmError::Exhausted`] when the region is full.
    pub fn allocate(&mut self, size: u64, align: u32) -> Result<Address, VmError> {
        let align = align.max(1);
        let start = align_up(self.next, align);
        let end = start
            .checked_add(size.max(1))
            .ok_or(VmError::Exhausted("memory"))?;
        // Bounded by the **heap's** low-water mark rather than by the region's end (ADR-0107 §2): the heap
        // grows downward from the top, so the two meet in the middle and either one running into the other is
        // exhaustion.
        if end > self.heap_next {
            return Err(VmError::Exhausted("memory"));
        }
        self.next = end;
        Ok(start)
    }

    /// Allocates `size` bytes for the **heap** — `malloc` — which a frame release never reclaims.
    ///
    /// # Why the heap grows from the far end
    ///
    /// Frames are a bump mark restored on return (see the module docs), and for a *slot* that is exactly
    /// right: a local's bytes should die with its frame. But `malloc` memory must **outlive the call that
    /// allocated it** — a procedure whose whole job is to allocate and hand the pointer back is the ordinary
    /// case, and it is what a growable array's `grow` does.
    ///
    /// Sharing one bump pointer made that silently wrong: the memory was released on return and the next
    /// frame reused the same bytes, so a heap write performed inside a callee read back as **zero** in the
    /// caller — zero rather than garbage, because `release` deliberately zeroes for determinism. The native
    /// back end calls libc and was correct, so the two engines **disagreed**, which is exactly the failure the
    /// corpus differential exists to catch and the first time it has caught one (ADR-0107 §2).
    ///
    /// Growing downward from the top is the standard answer and needs no free list: the two regions cannot
    /// overlap while `next <= heap_next`, which is the one check `allocate` and this share. Exhaustion is
    /// still [`VmError::Exhausted`], a diagnosable limit rather than a fault.
    ///
    /// Nothing reclaims heap bytes — `free` is a no-op — so a long-running comptime allocator leaks within the
    /// VM. That was already true and is unchanged; the bound turns it into a diagnosable error.
    pub fn allocate_heap(&mut self, size: u64, align: u32) -> Result<Address, VmError> {
        let align = u64::from(align.max(1));
        let size = size.max(1);
        let end = self
            .heap_next
            .checked_sub(size)
            .ok_or(VmError::Exhausted("memory"))?;
        // Aligned *downward*, since the block grows toward lower addresses: rounding up would move the start
        // into bytes the previous heap block already owns.
        let start = end - (end % align);
        if start < self.next {
            return Err(VmError::Exhausted("memory"));
        }
        self.heap_next = start;
        Ok(start)
    }

    /// Allocates space for `data` and copies it in.
    ///
    /// # Errors
    /// As [`Self::allocate`].
    pub fn allocate_bytes(&mut self, data: &[u8], align: u32) -> Result<Address, VmError> {
        let address = self.allocate(data.len() as u64, align)?;
        self.write(address, data)?;
        Ok(address)
    }

    /// Reads `size` bytes.
    ///
    /// # Errors
    /// [`Trap::BadAddress`] when the range is not entirely inside the region, or
    /// when it starts at null.
    pub fn read(&self, address: Address, size: u64) -> Result<&[u8], VmError> {
        let range = self.range(address, size)?;
        Ok(&self.bytes[range])
    }

    /// Writes `data` at `address`.
    ///
    /// # Errors
    /// As [`Self::read`].
    pub fn write(&mut self, address: Address, data: &[u8]) -> Result<(), VmError> {
        let range = self.range(address, data.len() as u64)?;
        self.bytes[range].copy_from_slice(data);
        Ok(())
    }

    /// Reads a little-endian `u64` of `size` bytes, zero-extended.
    ///
    /// Little-endian unconditionally: every target in the slice is arm64 or x86-64.
    /// A big-endian target would need this to consult [`jr_pool::TargetLayout`],
    /// which is the reason that type exists and takes the target as a parameter.
    ///
    /// # Errors
    /// As [`Self::read`], plus [`VmError::Internal`] if `size` exceeds 8.
    pub fn read_scalar(&self, address: Address, size: u64) -> Result<u64, VmError> {
        if size > 8 {
            return Err(VmError::internal(format!(
                "a scalar of {size} bytes does not fit a register"
            )));
        }
        let mut buf = [0u8; 8];
        let bytes = self.read(address, size)?;
        buf[..bytes.len()].copy_from_slice(bytes);
        Ok(u64::from_le_bytes(buf))
    }

    /// Writes the low `size` bytes of `bits` little-endian.
    ///
    /// # Errors
    /// As [`Self::read_scalar`].
    pub fn write_scalar(&mut self, address: Address, size: u64, bits: u64) -> Result<(), VmError> {
        if size > 8 {
            return Err(VmError::internal(format!(
                "a scalar of {size} bytes does not fit a register"
            )));
        }
        let buf = bits.to_le_bytes();
        let size = usize::try_from(size).unwrap_or(8);
        self.write(address, &buf[..size])
    }

    /// A raw host pointer to `size` bytes at `address`, for the FFI bridge.
    ///
    /// This is the whole reason the region does not move. The pointer is valid until
    /// the next [`Self::release`] or [`Self::allocate`] that would reuse the bytes;
    /// callers must use it within one foreign call and never store it.
    ///
    /// # Errors
    /// As [`Self::read`].
    pub fn host_pointer(&self, address: Address, size: u64) -> Result<*const u8, VmError> {
        let range = self.range(address, size)?;
        Ok(self.bytes[range].as_ptr())
    }

    /// Validates an access and returns it as a slice range.
    fn range(&self, address: Address, size: u64) -> Result<core::ops::Range<usize>, VmError> {
        let bad = || VmError::Trap(Trap::BadAddress { address, size });
        if address == 0 {
            return Err(bad());
        }
        let end = address.checked_add(size).ok_or_else(bad)?;
        if end > self.bytes.len() as u64 {
            return Err(bad());
        }
        let start = usize::try_from(address).map_err(|_| bad())?;
        let end = usize::try_from(end).map_err(|_| bad())?;
        Ok(start..end)
    }
}

impl Default for Memory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_allocated_at_null() {
        let mut mem = Memory::new();
        let address = mem.allocate(8, 8).expect("room");
        assert_ne!(address, 0, "address 0 is reserved for the null pointer");
        assert!(
            matches!(mem.read(0, 1), Err(VmError::Trap(Trap::BadAddress { .. }))),
            "a null dereference must trap, not read byte zero"
        );
    }

    #[test]
    fn allocations_respect_alignment() {
        let mut mem = Memory::new();
        let _ = mem.allocate(1, 1).expect("room");
        let aligned = mem.allocate(8, 8).expect("room");
        assert_eq!(aligned % 8, 0);
    }

    #[test]
    fn a_zero_sized_allocation_still_gets_a_distinct_address() {
        // `void` is a real type (ADR-0015 §3) and may be stored, so it needs one.
        let mut mem = Memory::new();
        let first = mem.allocate(0, 1).expect("room");
        let second = mem.allocate(0, 1).expect("room");
        assert_ne!(first, second);
    }

    #[test]
    fn scalars_round_trip_at_every_width() {
        let mut mem = Memory::new();
        let address = mem.allocate(8, 8).expect("room");
        for (size, bits) in [
            (1u64, 0xffu64),
            (2, 0xbeef),
            (4, 0xdead_beef),
            (8, u64::MAX),
        ] {
            mem.write_scalar(address, size, bits).expect("in bounds");
            assert_eq!(mem.read_scalar(address, size).expect("in bounds"), bits);
        }
    }

    #[test]
    fn a_narrow_read_does_not_see_the_neighbouring_bytes() {
        let mut mem = Memory::new();
        let address = mem.allocate(8, 8).expect("room");
        mem.write_scalar(address, 8, u64::MAX).expect("in bounds");
        assert_eq!(mem.read_scalar(address, 1).expect("in bounds"), 0xff);
    }

    #[test]
    fn bytes_round_trip() {
        let mut mem = Memory::new();
        let address = mem.allocate_bytes(b"hello", 1).expect("room");
        assert_eq!(mem.read(address, 5).expect("in bounds"), b"hello");
    }

    #[test]
    fn an_access_past_the_end_traps() {
        let mut mem = Memory::with_capacity(64);
        let address = mem.allocate(8, 8).expect("room");
        assert!(matches!(
            mem.read(address, 1024),
            Err(VmError::Trap(Trap::BadAddress { .. }))
        ));
        assert!(
            matches!(
                mem.read(u64::MAX, 1),
                Err(VmError::Trap(Trap::BadAddress { .. }))
            ),
            "the end computation must not wrap"
        );
    }

    #[test]
    fn exhaustion_is_reported_rather_than_growing_the_region() {
        // Growing would move the base and dangle any host pointer handed to a
        // foreign call, which is why the bound exists.
        let mut mem = Memory::with_capacity(64);
        assert!(matches!(
            mem.allocate(1024, 1),
            Err(VmError::Exhausted("memory"))
        ));
    }

    #[test]
    fn releasing_a_frame_reuses_and_zeroes_its_bytes() {
        let mut mem = Memory::new();
        let mark = mem.mark();
        let address = mem.allocate(8, 8).expect("room");
        mem.write_scalar(address, 8, 0xabcd).expect("in bounds");
        mem.release(mark);

        assert_eq!(mem.mark(), mark, "the mark must be restored exactly");
        let reused = mem.allocate(8, 8).expect("room");
        assert_eq!(reused, address, "a loop with a call must not leak a frame");
        assert_eq!(
            mem.read_scalar(reused, 8).expect("in bounds"),
            0,
            "released bytes are zeroed so a dangling read is reproducible"
        );
    }

    #[test]
    fn a_host_pointer_addresses_the_same_bytes() {
        let mut mem = Memory::new();
        let address = mem.allocate_bytes(b"jairs", 1).expect("room");
        let pointer = mem.host_pointer(address, 5).expect("in bounds");
        // SAFETY: `host_pointer` bounds-checked five bytes at `address`, and nothing
        // has allocated or released since, so the region is unchanged.
        let seen = unsafe { core::slice::from_raw_parts(pointer, 5) };
        assert_eq!(seen, b"jairs");
    }

    #[test]
    fn a_host_pointer_is_bounds_checked_like_any_other_access() {
        let mem = Memory::with_capacity(64);
        assert!(
            matches!(
                mem.host_pointer(1, 1024),
                Err(VmError::Trap(Trap::BadAddress { .. }))
            ),
            "the FFI boundary must not be a way around the bounds check"
        );
    }
}
