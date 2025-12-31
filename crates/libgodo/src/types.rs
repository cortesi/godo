use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    error::{GodoError, Result},
    git::{CommitInfo, DiffStats, MergeStatus},
    session::SessionLease,
};

/// Metadata persisted for a sandbox in the godo project directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxMetadata {
    /// Commit hash recorded when the sandbox was created.
    pub base_commit: String,
    /// Branch or ref name at sandbox creation, if available.
    pub base_ref: Option<String>,
    /// Unix timestamp (seconds) when the sandbox metadata was created.
    pub created_at: u64,
}

/// Policy for handling uncommitted repository changes when creating a sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UncommittedPolicy {
    /// Abort sandbox creation if the repository is dirty.
    Abort,
    /// Include uncommitted changes when creating the sandbox.
    Include,
    /// Reset the sandbox to a clean state after creation.
    Clean,
}

/// Options for preparing a sandbox.
#[derive(Debug, Clone)]
pub struct PrepareSandboxOptions {
    /// Policy for handling uncommitted changes in the source repository.
    pub uncommitted_policy: UncommittedPolicy,
    /// Directory names to exclude when cloning into the sandbox.
    pub excludes: Vec<String>,
}

/// Result of preparing a sandbox for use.
#[derive(Debug)]
pub struct PrepareSandboxPlan {
    /// Active sandbox session lease.
    pub session: SandboxSession,
    /// Whether the sandbox was created during this call.
    pub created: bool,
    /// Whether the sandbox was reset to a clean state after creation.
    pub cleaned: bool,
}

/// Active session lease for a sandbox.
#[derive(Debug)]
pub struct SandboxSession {
    /// Name of the sandbox.
    pub name: String,
    /// Filesystem path of the sandbox worktree.
    pub path: PathBuf,
    /// Lease used to track active connections.
    pub(crate) lease: SessionLease,
}

impl SandboxSession {
    /// Release the session lease and report whether cleanup is permitted.
    pub fn release(self) -> Result<crate::session::ReleaseOutcome> {
        self.lease.release()
    }
}

/// Status information for a sandbox.
#[derive(Debug, Clone)]
pub struct SandboxStatus {
    /// The name of the sandbox.
    pub name: String,
    /// Whether the branch exists.
    pub has_branch: bool,
    /// Whether the worktree exists.
    pub has_worktree: bool,
    /// Whether the worktree directory path exists.
    pub has_worktree_dir: bool,
    /// Branch currently checked out in the worktree, sans refs prefix, when known.
    pub worktree_branch: Option<String>,
    /// Whether the worktree is in detached HEAD state.
    pub worktree_detached: bool,
    /// Whether the worktree is checking out the expected sandbox branch when attached.
    pub worktree_branch_matches: bool,
    /// Whether there are any staged or unstaged uncommitted changes in the worktree.
    pub has_uncommitted_changes: bool,
    /// Diff statistics for uncommitted changes (lines added/removed).
    pub diff_stats: Option<DiffStats>,
    /// Merge relationship between the sandbox branch and its integration target.
    pub merge_status: MergeStatus,
    /// Commits not yet merged into the integration target.
    pub unmerged_commits: Vec<CommitInfo>,
    /// Whether the worktree is dangling (no backing directory).
    pub is_dangling: bool,
}

impl SandboxStatus {
    /// Returns true if the sandbox has both a worktree and a branch.
    pub fn is_live(&self) -> bool {
        self.has_branch
            && self.has_worktree
            && self.has_worktree_dir
            && (self.worktree_detached || self.worktree_branch_matches)
    }

    /// Summarize which sandbox components are currently present.
    pub fn component_status(&self) -> String {
        let branch = if self.has_branch {
            "present"
        } else {
            "missing"
        };
        let worktree = if self.has_worktree {
            "present"
        } else {
            "missing"
        };
        let directory = if self.has_worktree_dir {
            "present"
        } else {
            "missing"
        };

        let mut parts = vec![
            format!("branch: {branch}"),
            format!("worktree: {worktree}"),
            format!("directory: {directory}"),
        ];

        if self.is_dangling {
            parts.push("state: dangling".to_string());
        }

        if self.has_worktree {
            if self.worktree_detached {
                parts.push("worktree-branch: detached".to_string());
            } else if let Some(branch) = &self.worktree_branch
                && !self.worktree_branch_matches
            {
                parts.push(format!("worktree-branch: {branch}"));
            }
        }

        parts.join(", ")
    }
}

/// List entry combining sandbox status with active connection count.
#[derive(Debug, Clone)]
pub struct SandboxListEntry {
    /// Status information for the sandbox.
    pub status: SandboxStatus,
    /// Number of active godo sessions in the sandbox.
    pub active_connections: usize,
}

/// Plan describing how to show a diff for a sandbox.
#[derive(Debug, Clone)]
pub struct DiffPlan {
    /// Name of the sandbox being diffed.
    pub sandbox_name: String,
    /// Filesystem path to the sandbox worktree.
    pub sandbox_path: PathBuf,
    /// Base commit to diff against.
    pub base_commit: String,
    /// Whether a merge-base fallback was used to resolve the base.
    pub used_fallback: bool,
    /// Target ref used to compute the fallback base, when applicable.
    pub fallback_target: Option<String>,
    /// Untracked files to diff with `git diff --no-index`.
    pub untracked_files: Vec<PathBuf>,
}

/// Reasons that block a sandbox removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalBlocker {
    /// The sandbox has uncommitted changes.
    UncommittedChanges,
    /// The sandbox branch has unmerged commits.
    UnmergedCommits,
    /// The merge status of the sandbox branch is unknown.
    MergeStatusUnknown,
}

/// Removal plan describing the sandbox state and blockers.
#[derive(Debug, Clone)]
pub struct RemovalPlan {
    /// Status information for the sandbox.
    pub status: SandboxStatus,
    /// Reasons removal is blocked without confirmation.
    pub blockers: Vec<RemovalBlocker>,
}

/// Options for removing a sandbox in the presence of blockers.
#[derive(Debug, Clone, Copy)]
pub struct RemovalOptions {
    /// Allow removal when uncommitted changes exist.
    pub allow_uncommitted_changes: bool,
    /// Allow removal when unmerged commits exist.
    pub allow_unmerged_commits: bool,
    /// Allow removal when merge status is unknown.
    pub allow_unknown_merge_status: bool,
}

impl RemovalOptions {
    /// Allow removal regardless of blockers.
    pub fn force() -> Self {
        Self {
            allow_uncommitted_changes: true,
            allow_unmerged_commits: true,
            allow_unknown_merge_status: true,
        }
    }
}

/// Outcome of attempting a removal with options applied.
#[derive(Debug, Clone)]
pub enum RemovalOutcome {
    /// The sandbox was removed.
    Removed,
    /// Removal was blocked by the listed conditions.
    Blocked(Vec<RemovalBlocker>),
}

/// Report describing what happened during a cleanup.
#[derive(Debug, Clone)]
pub struct CleanupReport {
    /// Status information captured before cleanup.
    pub status: SandboxStatus,
    /// Whether the worktree was removed.
    pub worktree_removed: bool,
    /// Whether the branch was removed.
    pub branch_removed: bool,
    /// Whether a dangling directory was removed.
    pub directory_removed: bool,
}

/// Collection of cleanup reports and failures for batch operations.
#[derive(Debug, Default)]
pub struct CleanupBatch {
    /// Successful cleanup reports.
    pub reports: Vec<CleanupReport>,
    /// Per-sandbox cleanup failures.
    pub failures: Vec<CleanupFailure>,
}

/// Error information captured when cleaning a sandbox fails.
#[derive(Debug)]
pub struct CleanupFailure {
    /// Name of the sandbox that failed to clean.
    pub sandbox_name: String,
    /// Error encountered while cleaning.
    pub error: GodoError,
}
