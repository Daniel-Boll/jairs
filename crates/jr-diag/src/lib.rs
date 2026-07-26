//! Diagnostic model and rustc-quality renderer for the Jairs compiler.
//!
//! This crate provides:
//! - [`Severity`] — error/warning/note/help levels
//! - [`Label`] — a span annotation with an optional message
//! - [`InstantiationFrame`] — one frame of a polymorph instantiation backtrace
//! - [`Diagnostic`] — a complete diagnostic with primary span, secondary spans, notes, and backtrace
//! - [`Diagnostics`] — a sink that collects and sorts diagnostics
//! - [`Renderer`] — turns diagnostics into text using `annotate-snippets`

mod diagnostic;
mod render;
mod sink;

pub use diagnostic::{Diagnostic, InstantiationFrame, Label, Severity};
pub use render::{Config, Renderer};
pub use sink::Diagnostics;
