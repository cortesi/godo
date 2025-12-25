#![warn(missing_docs)]
#![deny(missing_docs)]
#![deny(rustdoc::missing_crate_level_docs)]
//! Core library for managing ephemeral Git worktrees ("sandboxes") used by godo.
//!
//! This crate exposes a small API surface that lets callers create and manage
//! sandboxes, query their status, and execute cleanup operations. User-facing
//! I/O is handled by frontends such as the `godo` CLI.

/// Helper routines for interacting with Git repositories.
mod git;
/// High-level orchestration for sandbox lifecycle management.
mod godo;
/// Sandbox metadata persistence helpers.
mod metadata;
/// Lightweight session tracking for concurrent godo runs.
mod session;

pub use git::{CommitInfo, DiffStats, MergeStatus};
pub use godo::{
    CleanupBatch, CleanupFailure, CleanupReport, DiffPlan, Godo, GodoError, PrepareSandboxOptions,
    PrepareSandboxPlan, RemovalBlocker, RemovalOptions, RemovalOutcome, RemovalPlan,
    SandboxListEntry, SandboxSession, SandboxStatus, UncommittedPolicy,
};
pub use session::{CleanupGuard, ReleaseOutcome};
