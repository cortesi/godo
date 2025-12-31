#![warn(missing_docs)]
#![deny(missing_docs)]
#![deny(rustdoc::missing_crate_level_docs)]
//! Core library for managing ephemeral Git worktrees ("sandboxes") used by godo.
//!
//! This crate exposes a small API surface that lets callers create and manage
//! sandboxes, query their status, and execute cleanup operations. User-facing
//! I/O is handled by frontends such as the `godo` CLI.

/// Error types for Godo operations.
mod error;
/// Helper routines for interacting with Git repositories.
mod git;
/// High-level orchestration for sandbox lifecycle management.
mod godo;
/// Lightweight session tracking for concurrent godo runs.
mod session;
/// Sandbox metadata persistence helpers.
mod store;
/// Domain types for Godo operations.
mod types;

pub use error::GodoError;
pub use git::{CommitInfo, DiffStats, MergeStatus};
pub use godo::Godo;
pub use session::{CleanupGuard, ReleaseOutcome};
pub use types::{
    CleanupBatch, CleanupFailure, CleanupReport, DiffPlan, PrepareSandboxOptions,
    PrepareSandboxPlan, RemovalBlocker, RemovalOptions, RemovalOutcome, RemovalPlan,
    SandboxListEntry, SandboxSession, SandboxStatus, UncommittedPolicy,
};
