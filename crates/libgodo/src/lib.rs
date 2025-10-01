#![deny(missing_docs)]
#![deny(rustdoc::missing_crate_level_docs)]
//! Core library for managing ephemeral Git worktrees ("sandboxes") used by godo.
//!
//! This crate exposes a small API surface that lets callers create and
//! manage sandboxes, and control how messages are emitted during operations.
//! The CLI binary in `crates/godo` builds on top of this library.

/// Helper routines for interacting with Git repositories.
mod git;
/// High-level orchestration for sandbox lifecycle management.
mod godo;
/// Output channel abstractions and implementations.
mod output;

/// Re-export of the main manager type and its error.
pub use godo::{Godo, GodoError};
/// Re-exports for output abstraction and concrete implementations.
pub use output::{Output, OutputError, Quiet, Terminal};
