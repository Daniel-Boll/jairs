//! String interning.
//!
//! Identifiers recur constantly in source code, so they are interned once and
//! then compared and hashed as a 32-bit [`Symbol`]. The interner is
//! thread-safe because [`crate`]-level parsing and semantic analysis are
//! intended to run in parallel (wave W8) and retrofitting that is painful.

use lasso::{Spur, ThreadedRodeo};

/// An interned string.
///
/// Cheap to copy, compare, and hash. Resolve it back to text with
/// [`Interner::resolve`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Symbol(Spur);

impl core::fmt::Debug for Symbol {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Deliberately opaque: a Symbol cannot resolve itself without the
        // interner, and pretending otherwise leads to misleading debug output.
        write!(f, "Symbol#{}", self.0.into_inner())
    }
}

/// A thread-safe string interner.
///
/// Cloning is cheap and shares the same underlying table.
#[derive(Debug, Clone, Default)]
pub struct Interner {
    rodeo: std::sync::Arc<ThreadedRodeo>,
}

impl Interner {
    /// Creates an empty interner.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Interns `text`, returning a stable [`Symbol`].
    ///
    /// Interning the same text twice yields the same symbol.
    pub fn intern(&self, text: &str) -> Symbol {
        Symbol(self.rodeo.get_or_intern(text))
    }

    /// Returns the [`Symbol`] for `text` if it has already been interned.
    pub fn get(&self, text: &str) -> Option<Symbol> {
        self.rodeo.get(text).map(Symbol)
    }

    /// Resolves a symbol back to its text.
    ///
    /// # Panics
    /// Panics if `symbol` came from a different interner.
    pub fn resolve(&self, symbol: Symbol) -> &str {
        self.rodeo.resolve(&symbol.0)
    }

    /// Returns the number of distinct interned strings.
    pub fn len(&self) -> usize {
        self.rodeo.len()
    }

    /// Returns `true` if nothing has been interned.
    pub fn is_empty(&self) -> bool {
        self.rodeo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_idempotent() {
        let interner = Interner::new();
        let a = interner.intern("main");
        let b = interner.intern("main");
        let c = interner.intern("Main");
        assert_eq!(a, b);
        assert_ne!(a, c, "interning must be case-sensitive");
        assert_eq!(interner.resolve(a), "main");
        assert_eq!(interner.len(), 2);
    }

    #[test]
    fn symbol_is_four_bytes() {
        assert_eq!(size_of::<Symbol>(), 4);
        assert_eq!(size_of::<Option<Symbol>>(), 4, "Symbol must have a niche");
    }

    #[test]
    fn get_does_not_intern() {
        let interner = Interner::new();
        assert_eq!(interner.get("absent"), None);
        assert!(interner.is_empty());
    }

    #[test]
    fn shares_table_across_clones() {
        let a = Interner::new();
        let b = a.clone();
        let sym = a.intern("shared");
        assert_eq!(b.resolve(sym), "shared");
    }
}
