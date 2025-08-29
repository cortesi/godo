//! Core library for managing ephemeral Git worktrees ("sandboxes") used by godo.
//!
//! This crate exposes a small API surface that lets callers create and
//! manage sandboxes, and control how messages are emitted during operations.
//! The CLI binary in `crates/godo` builds on top of this library.

mod git;
mod godo;
mod output;

/// Re-export of the main manager type and its error.
pub use godo::{Godo, GodoError};
/// Re-exports for output abstraction and concrete implementations.
pub use output::{Output, OutputError, Quiet, Terminal};
