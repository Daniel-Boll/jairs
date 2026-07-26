//! The newtype-index convention used throughout the compiler.
//!
//! Following rustc's approach: intern everything, then refer to it by a 32-bit
//! index wrapped in a distinct type. This keeps IR nodes small, makes them
//! `Copy`, and makes it a compile error to use a `TypeId` where a `ValueId` was
//! meant.

/// Declares a 32-bit newtype index.
///
/// The generated type is `Copy`, `Eq`, `Hash`, and cheap to pass around. Index
/// `u32::MAX` is reserved as a niche so that `Option<Id>` stays 4 bytes.
///
/// ```
/// jr_base::newtype_index! {
///     /// A local variable slot.
///     pub struct LocalId;
/// }
/// let id = LocalId::from_usize(3);
/// assert_eq!(id.index(), 3);
/// ```
#[macro_export]
macro_rules! newtype_index {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident;
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(transparent)]
        $vis struct $name(::core::num::NonZeroU32);

        impl $name {
            /// The largest representable index.
            $vis const MAX: usize = (u32::MAX - 1) as usize;

            /// Creates an index from a `usize`.
            ///
            /// # Panics
            /// Panics if `index` exceeds [`Self::MAX`].
            #[inline]
            $vis const fn from_usize(index: usize) -> Self {
                assert!(index <= Self::MAX, "newtype index overflow");
                // SAFETY: `index + 1` is non-zero because `index <= u32::MAX - 1`.
                Self(unsafe { ::core::num::NonZeroU32::new_unchecked(index as u32 + 1) })
            }

            /// Creates an index from a `u32`.
            ///
            /// # Panics
            /// Panics if `index` is `u32::MAX`.
            #[inline]
            $vis const fn from_u32(index: u32) -> Self {
                Self::from_usize(index as usize)
            }

            /// Returns the index as a `usize`.
            #[inline]
            $vis const fn index(self) -> usize {
                self.0.get() as usize - 1
            }

            /// Returns the index as a `u32`.
            #[inline]
            $vis const fn as_u32(self) -> u32 {
                self.0.get() - 1
            }
        }

        impl ::core::fmt::Debug for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.index())
            }
        }
    };
}

#[cfg(test)]
mod tests {
    #[allow(dead_code, reason = "the macro generates a full API; tests use part")]
    mod generated {
        crate::newtype_index! {
            /// Test index.
            pub struct TestId;
        }
    }
    use generated::TestId;

    #[test]
    fn round_trips() {
        for raw in [0usize, 1, 7, 4096, TestId::MAX] {
            assert_eq!(TestId::from_usize(raw).index(), raw);
        }
    }

    #[test]
    fn option_is_niche_optimised() {
        assert_eq!(
            size_of::<Option<TestId>>(),
            size_of::<TestId>(),
            "Option<Id> must stay 4 bytes or IR nodes bloat"
        );
    }

    #[test]
    #[should_panic(expected = "newtype index overflow")]
    fn rejects_overflow() {
        let _ = TestId::from_usize(TestId::MAX + 1);
    }
}
