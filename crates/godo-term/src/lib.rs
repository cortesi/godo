#![warn(missing_docs)]
#![deny(missing_docs)]
#![deny(rustdoc::missing_crate_level_docs)]
//! Terminal output primitives for godo frontends.
//!
//! This crate isolates terminal rendering, prompts, and spinners so libgodo can
//! remain UI-agnostic. Use these helpers in CLI or TUI frontends.

/// Terminal output abstractions and implementations.
mod output;

pub use output::{Output, OutputError, Quiet, Spinner, Terminal};
