#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::{symlink_dir, symlink_file};
use std::{
    collections::HashSet,
    env, fs, io,
    path::{Component, Path, PathBuf},
    result::Result as StdResult,
    time::{SystemTime, UNIX_EPOCH},
};

use clonetree::{Options, clone_tree};
use thiserror::Error;

use crate::{
    git::{self, CommitInfo, DiffStats, MergeStatus},
    metadata::{SandboxMetadata, SandboxMetadataStore},
    session::{LEASE_DIR_NAME, ReleaseOutcome, SessionLease, SessionManager},
};

/// Custom Result type for Godo operations.
pub type Result<T> = StdResult<T, GodoError>;

/// Godo-specific error types
#[derive(Error, Debug)]
pub enum GodoError {
    /// A command executed inside the sandbox exited with a non-zero status.
    #[error("Command exited with status code: {code}")]
    CommandExit {
        /// The process exit status code.
        code: i32,
    },

    /// The requested sandbox operation failed due to an invalid state.
    #[error("Sandbox error: {message}")]
    SandboxError {
        /// Name of the sandbox associated with the failure.
        name: String,
        /// Human-readable error description.
        message: String,
    },

    /// The operation was cancelled by the user.
    #[error("Aborted by user")]
    UserAborted,

    /// A contextual precondition failed (e.g. not inside a Git repo).
    #[error("Context error: {0}")]
    ContextError(String),

    /// A high-level operation failed.
    #[error("Operation failed: {0}")]
    OperationError(String),

    /// A git command failed.
    #[error("Git error: {0}")]
    GitError(String),

    /// Base commit resolution failed for a sandbox.
    #[error("Base commit error for sandbox '{name}': {message}")]
    BaseError {
        /// Name of the sandbox associated with the failure.
        name: String,
        /// Human-readable error description.
        message: String,
    },
    /// The repository has uncommitted changes and the selected policy forbids proceeding.
    #[error("Uncommitted changes present in repository: {repo_dir}")]
    UncommittedChanges {
        /// Root of the repository with uncommitted changes.
        repo_dir: PathBuf,
    },

    /// An underlying I/O operation failed.
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),
}

impl GodoError {
    /// Return the recommended process exit code for this error.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::CommandExit { code } => *code,
            Self::UserAborted => 130,
            Self::SandboxError { .. } => 2,
            Self::UncommittedChanges { .. } => 2,
            Self::BaseError { .. } => 3,
            Self::GitError(_) => 4,
            _ => 1,
        }
    }
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
    lease: SessionLease,
}

impl SandboxSession {
    /// Release the session lease and report whether cleanup is permitted.
    pub fn release(self) -> Result<ReleaseOutcome> {
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

/// Hardcoded fallback targets when dynamic detection fails.
const FALLBACK_TARGETS: &[&str] = &["origin/main", "origin/master", "main", "master"];

/// Map git errors into a `GodoError::GitError`.
fn git_error(error: &anyhow::Error) -> GodoError {
    GodoError::GitError(error.to_string())
}

/// Outcome of resolving a sandbox base commit.
struct BaseResolution {
    /// Resolved commit hash.
    commit: String,
    /// Whether a merge-base fallback was used.
    used_fallback: bool,
    /// The target ref used for merge-base fallback, when applicable.
    fallback_target: Option<String>,
}

/// Validates that a sandbox name contains only allowed characters (a-zA-Z0-9-_)
fn validate_sandbox_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(GodoError::SandboxError {
            name: name.to_string(),
            message: "names can only contain letters, numbers, hyphens, and underscores"
                .to_string(),
        });
    }

    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(GodoError::SandboxError {
            name: name.to_string(),
            message: "names can only contain letters, numbers, hyphens, and underscores"
                .to_string(),
        });
    }

    Ok(())
}

/// Cleans a name to only contain allowed characters (a-zA-Z0-9-_)
/// Non-allowed characters are replaced with hyphens
fn clean_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Extract and clean the project name from a Git repository path.
fn project_name(repo_path: &Path) -> Result<String> {
    let name = repo_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_else(|| {
            // For root paths or paths without a file name, try to use the last component
            repo_path
                .components()
                .next_back()
                .and_then(|c| match c {
                    Component::Normal(name) => name.to_str(),
                    _ => None,
                })
                .unwrap_or("root")
        });

    let cleaned = clean_name(name);

    if cleaned.is_empty() {
        return Err(GodoError::ContextError(
            "Could not derive a valid project name from repository path".to_string(),
        ));
    }

    Ok(cleaned)
}

/// Return the Git branch name for a given sandbox name.
fn branch_name(sandbox_name: &str) -> String {
    format!("godo/{}", sandbox_name)
}

/// Manager for creating and operating on ephemeral Git sandboxes based on
/// worktrees.
///
/// A sandbox is a dedicated worktree rooted under a project-specific
/// directory inside the "godo directory". `Godo` provides high-level
/// operations to create sandboxes, inspect their status, and perform cleanup
/// and removal operations.
pub struct Godo {
    /// Base directory where per-project sandbox directories live.
    godo_dir: PathBuf,
    /// Root of the Git repository the sandboxes operate on.
    repo_dir: PathBuf,
}

impl Godo {
    /// Directory under the project root reserved for godo's internal bookkeeping.
    const LEASE_DIR: &'static str = LEASE_DIR_NAME;
    /// Directory under the project root reserved for sandbox metadata.
    const METADATA_DIR: &'static str = SandboxMetadataStore::DIR_NAME;
    /// Create a new [`Godo`] manager.
    ///
    /// - `godo_dir`: directory where project sandboxes are stored
    /// - `repo_dir`: optional path to the git repository root. If `None`, the
    ///   repository root is discovered by walking up from the current directory.
    pub fn new(godo_dir: PathBuf, repo_dir: Option<PathBuf>) -> Result<Self> {
        // Ensure godo directory exists
        ensure_godo_directory(&godo_dir)?;

        // Normalize godo directory to an absolute path when possible so we
        // compare paths against Git's absolute worktree paths consistently.
        let godo_dir = fs::canonicalize(&godo_dir).unwrap_or(godo_dir);

        // Determine repository directory
        let repo_dir = if let Some(dir) = repo_dir {
            dir
        } else {
            // Find git root from current directory
            let current_dir = env::current_dir().map_err(|_| {
                GodoError::ContextError("Failed to get current directory".to_string())
            })?;
            git::find_root(&current_dir).ok_or(GodoError::ContextError(
                "Not in a git repository".to_string(),
            ))?
        };

        // Canonicalize the repository root to keep sandbox paths stable.
        let repo_dir = fs::canonicalize(&repo_dir).unwrap_or(repo_dir);

        Ok(Self { godo_dir, repo_dir })
    }

    /// Get the project directory path within the godo directory
    fn project_dir(&self) -> Result<PathBuf> {
        let project = project_name(&self.repo_dir)?;
        Ok(self.godo_dir.join(&project))
    }

    /// Calculate the path for a sandbox given its name.
    pub fn sandbox_path(&self, sandbox_name: &str) -> Result<PathBuf> {
        Ok(self.project_dir()?.join(sandbox_name))
    }

    /// Build a metadata store for the current project.
    fn metadata_store(&self) -> Result<SandboxMetadataStore> {
        Ok(SandboxMetadataStore::new(&self.project_dir()?))
    }

    /// Persist metadata for a newly created sandbox.
    fn record_metadata(
        &self,
        sandbox_name: &str,
        base_commit: String,
        base_ref: Option<String>,
    ) -> Result<()> {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let metadata = SandboxMetadata {
            base_commit,
            base_ref,
            created_at,
        };
        self.metadata_store()?
            .write(sandbox_name, &metadata)
            .map_err(|e| GodoError::OperationError(format!("Metadata error: {e}")))?;
        Ok(())
    }

    /// Remove metadata for a sandbox if present.
    fn remove_metadata(&self, sandbox_name: &str) -> Result<()> {
        self.metadata_store()?
            .remove(sandbox_name)
            .map_err(|e| GodoError::OperationError(format!("Metadata error: {e}")))?;
        Ok(())
    }

    /// Get the status for a sandbox or return a not-found error.
    fn require_sandbox_status(&self, name: &str) -> Result<SandboxStatus> {
        match self.get_sandbox(name)? {
            Some(status) => Ok(status),
            None => Err(GodoError::SandboxError {
                name: name.to_string(),
                message: "does not exist".to_string(),
            }),
        }
    }

    /// Get the sandbox worktree path when a live worktree exists.
    fn require_worktree_path(&self, name: &str) -> Result<PathBuf> {
        let status = self.require_sandbox_status(name)?;
        if !status.has_worktree || !status.has_worktree_dir {
            return Err(GodoError::SandboxError {
                name: name.to_string(),
                message: "has no worktree to operate on".to_string(),
            });
        }
        self.sandbox_path(name)
    }

    /// Check whether the source repository has uncommitted changes.
    pub fn repo_has_uncommitted_changes(&self) -> Result<bool> {
        git::has_uncommitted_changes(&self.repo_dir).map_err(|e| git_error(&e))
    }

    /// Create or reuse a sandbox and acquire a session lease for it.
    pub fn prepare_sandbox(
        &self,
        sandbox_name: &str,
        options: PrepareSandboxOptions,
    ) -> Result<PrepareSandboxPlan> {
        validate_sandbox_name(sandbox_name)?;

        let sandbox_path = self.sandbox_path(sandbox_name)?;
        let project_dir = self.project_dir()?;
        let session_manager = SessionManager::new(&project_dir);

        // Acquire lock to ensure exclusive access during creation/verification
        let locked_session = session_manager.lock(sandbox_name)?;

        let existing_sandbox = self.get_sandbox(sandbox_name)?;
        let mut created = false;
        let mut cleaned = false;

        if let Some(sandbox) = existing_sandbox {
            if !sandbox.is_live() {
                let status = sandbox.component_status();
                return Err(GodoError::SandboxError {
                    name: sandbox_name.to_string(),
                    message: format!("exists but is not live - remove it first ({status})"),
                });
            }
        } else {
            let PrepareSandboxOptions {
                uncommitted_policy,
                excludes,
            } = options;
            let has_uncommitted = self.repo_has_uncommitted_changes()?;
            let use_clean_branch = matches!(uncommitted_policy, UncommittedPolicy::Clean);

            if has_uncommitted && matches!(uncommitted_policy, UncommittedPolicy::Abort) {
                return Err(GodoError::UncommittedChanges {
                    repo_dir: self.repo_dir.clone(),
                });
            }

            // Ensure project directory exists
            fs::create_dir_all(&project_dir)?;

            let base_commit = git::rev_parse(&self.repo_dir, "HEAD").map_err(|e| git_error(&e))?;
            let base_ref = git::head_ref(&self.repo_dir).map_err(|e| git_error(&e))?;

            let branch = branch_name(sandbox_name);
            git::create_worktree(&self.repo_dir, &sandbox_path, &branch)
                .map_err(|e| git_error(&e))?;

            // Clone each top-level entry from repo to sandbox, skipping .git.
            // We do this entry-by-entry because clone_tree requires the destination
            // not to exist, but the worktree already created the sandbox with .git.
            for entry in fs::read_dir(&self.repo_dir)? {
                let entry = entry?;
                let name = entry.file_name();
                if name == ".git" {
                    continue;
                }

                // Check user excludes
                let name_str = name.to_string_lossy();
                if excludes.iter().any(|ex| name_str == *ex) {
                    continue;
                }

                let src = entry.path();
                let dest = sandbox_path.join(&name);

                // Remove existing entry in sandbox (from worktree checkout)
                if dest.exists() || dest.is_symlink() {
                    if dest.is_dir() && !dest.is_symlink() {
                        fs::remove_dir_all(&dest)?;
                    } else {
                        fs::remove_file(&dest)?;
                    }
                }

                if src.is_dir() && !src.is_symlink() {
                    clone_tree(&src, &dest, &Options::new()).map_err(|e| {
                        GodoError::OperationError(format!(
                            "Failed to clone {:?} to sandbox: {e}",
                            name
                        ))
                    })?;
                } else if src.is_symlink() {
                    let target = fs::read_link(&src)?;
                    #[cfg(unix)]
                    symlink(&target, &dest)?;
                    #[cfg(windows)]
                    {
                        if target.is_dir() {
                            symlink_dir(&target, &dest)?;
                        } else {
                            symlink_file(&target, &dest)?;
                        }
                    }
                } else {
                    reflink_copy::reflink_or_copy(&src, &dest).map_err(|e| {
                        GodoError::OperationError(format!(
                            "Failed to copy {:?} to sandbox: {e}",
                            name
                        ))
                    })?;
                }
            }

            if has_uncommitted && use_clean_branch {
                git::reset_hard(&sandbox_path)
                    .map_err(|e| GodoError::GitError(format!("Failed to reset sandbox: {e}")))?;
                git::clean(&sandbox_path)
                    .map_err(|e| GodoError::GitError(format!("Failed to clean sandbox: {e}")))?;
                cleaned = true;
            }

            self.record_metadata(sandbox_name, base_commit, base_ref)?;
            created = true;
        }

        // Acquire session lease to track concurrent connections.
        let lease = locked_session.acquire_lease()?;
        let session = SandboxSession {
            name: sandbox_name.to_string(),
            path: sandbox_path,
            lease,
        };

        Ok(PrepareSandboxPlan {
            session,
            created,
            cleaned,
        })
    }

    /// Plan a diff for a sandbox against its recorded base commit.
    pub fn diff_plan(&self, sandbox_name: &str, base_override: Option<&str>) -> Result<DiffPlan> {
        validate_sandbox_name(sandbox_name)?;

        let sandbox = match self.get_sandbox(sandbox_name)? {
            Some(status) => status,
            None => {
                return Err(GodoError::SandboxError {
                    name: sandbox_name.to_string(),
                    message: "does not exist".to_string(),
                });
            }
        };

        if !sandbox.is_live() {
            let status = sandbox.component_status();
            return Err(GodoError::SandboxError {
                name: sandbox_name.to_string(),
                message: format!("exists but is not live - remove it first ({status})"),
            });
        }

        let sandbox_path = self.sandbox_path(sandbox_name)?;
        let base = self.resolve_base_commit(sandbox_name, base_override)?;
        let untracked_files = git::untracked_files(&sandbox_path).map_err(|e| git_error(&e))?;

        Ok(DiffPlan {
            sandbox_name: sandbox_name.to_string(),
            sandbox_path,
            base_commit: base.commit,
            used_fallback: base.used_fallback,
            fallback_target: base.fallback_target,
            untracked_files,
        })
    }

    /// Resolve the base commit for a sandbox diff.
    fn resolve_base_commit(
        &self,
        sandbox_name: &str,
        base_override: Option<&str>,
    ) -> Result<BaseResolution> {
        if let Some(base) = base_override {
            let commit =
                git::rev_parse(&self.repo_dir, base).map_err(|e| GodoError::BaseError {
                    name: sandbox_name.to_string(),
                    message: format!("override '{base}' could not be resolved: {e}"),
                })?;
            return Ok(BaseResolution {
                commit,
                used_fallback: false,
                fallback_target: None,
            });
        }

        let metadata =
            self.metadata_store()?
                .read(sandbox_name)
                .map_err(|e| GodoError::BaseError {
                    name: sandbox_name.to_string(),
                    message: format!("metadata unreadable: {e}"),
                })?;

        let metadata = metadata.ok_or_else(|| GodoError::BaseError {
            name: sandbox_name.to_string(),
            message: "metadata missing for sandbox".to_string(),
        })?;

        match git::rev_parse(&self.repo_dir, &metadata.base_commit) {
            Ok(commit) => Ok(BaseResolution {
                commit,
                used_fallback: false,
                fallback_target: None,
            }),
            Err(_) => {
                let branch = branch_name(sandbox_name);
                let mut candidates = Vec::new();

                // First priority: the recorded base_ref from metadata
                if let Some(base_ref) = metadata.base_ref.as_ref() {
                    candidates.push(base_ref.clone());
                }

                // Second priority: dynamically detected default integration target
                if let Ok(Some(default_target)) = git::default_integration_target(&self.repo_dir)
                    && !candidates.contains(&default_target)
                {
                    candidates.push(default_target);
                }

                // Third priority: hardcoded fallbacks for common branch names
                for fallback in FALLBACK_TARGETS {
                    let fallback_str = (*fallback).to_string();
                    if !candidates.contains(&fallback_str) {
                        candidates.push(fallback_str);
                    }
                }

                let mut last_error = None;
                for target in candidates {
                    match git::merge_base(&self.repo_dir, &branch, &target) {
                        Ok(commit) => {
                            return Ok(BaseResolution {
                                commit,
                                used_fallback: true,
                                fallback_target: Some(target),
                            });
                        }
                        Err(error) => last_error = Some((target, error)),
                    }
                }

                let message = if let Some((target, error)) = last_error {
                    format!("recorded base missing and merge-base failed for {target}: {error}")
                } else {
                    "recorded base missing and merge-base failed".to_string()
                };

                Err(GodoError::BaseError {
                    name: sandbox_name.to_string(),
                    message,
                })
            }
        }
    }

    /// Get the status of a sandbox by name.
    fn get_sandbox(&self, name: &str) -> Result<Option<SandboxStatus>> {
        let sandbox_path = self.sandbox_path(name)?;
        let branch_name = branch_name(name);

        // Check if branch exists
        let has_branch =
            git::has_branch(&self.repo_dir, &branch_name).map_err(|e| git_error(&e))?;

        // Get all worktrees to check if this sandbox has a worktree attached in the godo directory
        let worktrees = git::list_worktrees(&self.repo_dir).map_err(|e| git_error(&e))?;
        let matching_worktree = worktrees.iter().find(|w| w.path == sandbox_path);

        let has_worktree = matching_worktree.is_some();
        let mut worktree_branch = None;
        let mut worktree_detached = false;
        let mut branch_matches_worktree = false;

        if let Some(info) = matching_worktree {
            worktree_detached = info.is_detached;
            if let Some(branch) = &info.branch {
                let normalized = branch
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch)
                    .to_string();
                branch_matches_worktree = normalized == branch_name;
                worktree_branch = Some(normalized);
            } else {
                // Detached worktrees still count as present for reuse purposes.
                branch_matches_worktree = true;
            }
        }

        // Check if the worktree directory exists
        let has_worktree_dir = sandbox_path.exists();

        // If neither branch, worktree, nor directory exists, the sandbox doesn't exist
        if !has_branch && !has_worktree && !has_worktree_dir {
            return Ok(None);
        }

        // Determine merge status relative to integration target (only if branch exists)
        let (merge_status, unmerged_commits) = if has_branch {
            let status = git::branch_merge_status(&self.repo_dir, &branch_name)
                .unwrap_or(MergeStatus::Unknown);
            let commits = if matches!(status, MergeStatus::Diverged) {
                git::unmerged_commits(&self.repo_dir, &branch_name).unwrap_or_default()
            } else {
                Vec::new()
            };
            (status, commits)
        } else {
            (MergeStatus::Unknown, Vec::new())
        };

        // Check if dangling:
        //  - Git records a worktree but the directory is gone, or
        //  - A directory exists but no branch backs it.
        let is_dangling = (has_worktree && !has_worktree_dir) || (has_worktree_dir && !has_branch);

        // Check for uncommitted changes (only if worktree exists)
        let (has_uncommitted_changes, diff_stats) = if has_worktree && has_worktree_dir {
            let has_changes = git::has_uncommitted_changes(&sandbox_path).unwrap_or(false);
            let stats = if has_changes {
                git::diff_stats(&sandbox_path).ok()
            } else {
                None
            };
            (has_changes, stats)
        } else {
            (false, None)
        };

        Ok(Some(SandboxStatus {
            name: name.to_string(),
            has_branch,
            has_worktree,
            has_worktree_dir,
            worktree_branch,
            worktree_detached,
            worktree_branch_matches: branch_matches_worktree,
            has_uncommitted_changes,
            diff_stats,
            merge_status,
            unmerged_commits,
            is_dangling,
        }))
    }

    /// Gather every sandbox name present in branches, worktrees, or on disk.
    fn all_sandbox_names(&self) -> Result<Vec<String>> {
        let project_dir = self.project_dir()?;

        let mut all_names = HashSet::new();

        let all_branches = git::list_branches(&self.repo_dir).map_err(|e| git_error(&e))?;
        for branch in &all_branches {
            if let Some(name) = branch.strip_prefix("godo/") {
                all_names.insert(name.to_string());
            }
        }

        for worktree in git::list_worktrees(&self.repo_dir).map_err(|e| git_error(&e))? {
            if let Some(branch) = &worktree.branch {
                let branch = branch.strip_prefix("refs/heads/").unwrap_or(branch);
                if let Some(name) = branch.strip_prefix("godo/") {
                    all_names.insert(name.to_string());
                }
            }
        }

        if project_dir.exists() {
            for entry in fs::read_dir(&project_dir)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    let dir_name = entry.file_name().to_string_lossy().to_string();
                    if dir_name == Self::LEASE_DIR || dir_name == Self::METADATA_DIR {
                        continue;
                    }
                    all_names.insert(dir_name);
                }
            }
        }

        // Sort all names alphabetically
        let mut sorted_names: Vec<String> = all_names.into_iter().collect();
        sorted_names.sort();

        Ok(sorted_names)
    }

    /// List all known sandboxes for the current project with their status.
    pub fn list(&self) -> Result<Vec<SandboxListEntry>> {
        let sorted_names = self.all_sandbox_names()?;
        let project_dir = self.project_dir()?;
        let session_manager = SessionManager::new(&project_dir);

        let mut entries = Vec::new();
        for name in &sorted_names {
            if let Some(status) = self.get_sandbox(name)? {
                let connections = session_manager.active_connections(name)?;
                entries.push(SandboxListEntry {
                    status,
                    active_connections: connections,
                });
            }
        }

        Ok(entries)
    }

    /// Get the status of a sandbox by name.
    pub fn sandbox_status(&self, name: &str) -> Result<Option<SandboxStatus>> {
        self.get_sandbox(name)
    }

    /// Build a removal plan for a sandbox.
    pub fn removal_plan(&self, name: &str) -> Result<RemovalPlan> {
        let status = self.require_sandbox_status(name)?;

        let mut blockers = Vec::new();
        if status.has_uncommitted_changes {
            blockers.push(RemovalBlocker::UncommittedChanges);
        }
        match status.merge_status {
            MergeStatus::Diverged => blockers.push(RemovalBlocker::UnmergedCommits),
            MergeStatus::Unknown => blockers.push(RemovalBlocker::MergeStatusUnknown),
            MergeStatus::Clean => {}
        }

        Ok(RemovalPlan { status, blockers })
    }

    /// Remove a sandbox based on a plan and options.
    pub fn remove(&self, plan: &RemovalPlan, options: &RemovalOptions) -> Result<RemovalOutcome> {
        let mut blocked = Vec::new();
        for blocker in &plan.blockers {
            let allowed = match blocker {
                RemovalBlocker::UncommittedChanges => options.allow_uncommitted_changes,
                RemovalBlocker::UnmergedCommits => options.allow_unmerged_commits,
                RemovalBlocker::MergeStatusUnknown => options.allow_unknown_merge_status,
            };
            if !allowed {
                blocked.push(*blocker);
            }
        }

        if !blocked.is_empty() {
            return Ok(RemovalOutcome::Blocked(blocked));
        }

        self.remove_sandbox_force(&plan.status.name)?;
        Ok(RemovalOutcome::Removed)
    }

    /// Remove the sandbox worktree while keeping its branch.
    pub fn remove_worktree_keep_branch(&self, name: &str) -> Result<()> {
        let status = self.require_sandbox_status(name)?;

        let sandbox_path = self.sandbox_path(name)?;

        if status.has_worktree {
            git::remove_worktree(&self.repo_dir, &sandbox_path, true).map_err(|e| git_error(&e))?;
        }
        if sandbox_path.exists() {
            fs::remove_dir_all(&sandbox_path).map_err(|e| {
                GodoError::OperationError(format!("Failed to remove sandbox directory: {e}"))
            })?;
        }

        self.remove_metadata(name)?;
        Ok(())
    }

    /// Stage and commit all changes inside a sandbox.
    pub fn commit_all(&self, name: &str, message: &str) -> Result<()> {
        let sandbox_path = self.require_worktree_path(name)?;
        git::add_all(&sandbox_path).map_err(|e| git_error(&e))?;
        git::commit(&sandbox_path, message).map_err(|e| git_error(&e))?;
        Ok(())
    }

    /// Clean one sandbox or all sandboxes by removing stale worktrees/branches
    /// when safe to do so.
    pub fn clean(&self, name: Option<&str>) -> Result<CleanupBatch> {
        let mut batch = CleanupBatch::default();

        match name {
            Some(name) => match self.cleanup_sandbox(name) {
                Ok(report) => batch.reports.push(report),
                Err(error) => batch.failures.push(CleanupFailure {
                    sandbox_name: name.to_string(),
                    error,
                }),
            },
            None => {
                let all_names = self.all_sandbox_names()?;

                for sandbox_name in all_names {
                    match self.cleanup_sandbox(&sandbox_name) {
                        Ok(report) => batch.reports.push(report),
                        Err(error) => batch.failures.push(CleanupFailure {
                            sandbox_name,
                            error,
                        }),
                    }
                }
            }
        }

        Ok(batch)
    }

    /// Clean up a sandbox by removing worktree if no uncommitted changes and branch if no unmerged commits.
    fn cleanup_sandbox(&self, name: &str) -> Result<CleanupReport> {
        let status = self.require_sandbox_status(name)?;

        let sandbox_path = self.sandbox_path(name)?;
        let branch = branch_name(name);

        let mut worktree_removed = false;
        let mut branch_removed = false;
        let mut directory_removed = false;

        // Remove the worktree if it exists and has no uncommitted changes
        if status.has_worktree && !status.has_uncommitted_changes {
            git::remove_worktree(&self.repo_dir, &sandbox_path, false)
                .map_err(|e| git_error(&e))?;
            worktree_removed = true;
        }

        // Clean up the directory if it still exists and worktree was removed
        if !status.has_worktree && status.has_worktree_dir {
            fs::remove_dir_all(&sandbox_path).map_err(|e| {
                GodoError::OperationError(format!("Failed to remove sandbox directory: {e}"))
            })?;
            directory_removed = true;
        }

        // Only remove the branch if:
        // 1. It exists
        // 2. It has no unmerged commits
        // 3. Either we successfully removed the worktree OR (there was no worktree to begin with AND no worktree directory exists)
        if status.has_branch
            && matches!(status.merge_status, MergeStatus::Clean)
            && (worktree_removed || (!status.has_worktree && !status.has_worktree_dir))
        {
            git::delete_branch(&self.repo_dir, &branch, false).map_err(|e| git_error(&e))?;
            branch_removed = true;
        }

        if worktree_removed || branch_removed || directory_removed {
            self.remove_metadata(name)?;
        }

        Ok(CleanupReport {
            status,
            worktree_removed,
            branch_removed,
            directory_removed,
        })
    }

    /// Remove a sandbox forcefully, ignoring blockers.
    fn remove_sandbox_force(&self, name: &str) -> Result<()> {
        // Get sandbox status to check current state
        let status = self.require_sandbox_status(name)?;

        let sandbox_path = self.sandbox_path(name)?;
        let branch = branch_name(name);

        if status.has_worktree {
            git::remove_worktree(&self.repo_dir, &sandbox_path, true).map_err(|e| git_error(&e))?;
        }
        if sandbox_path.exists() {
            fs::remove_dir_all(&sandbox_path).map_err(|e| {
                GodoError::OperationError(format!("Failed to remove sandbox directory: {e}"))
            })?;
        }
        if status.has_branch {
            git::delete_branch(&self.repo_dir, &branch, true).map_err(|e| git_error(&e))?;
        }
        self.remove_metadata(name)?;
        Ok(())
    }
}

/// Ensure the primary godo directory hierarchy exists.
fn ensure_godo_directory(godo_dir: &Path) -> Result<()> {
    // Create main godo directory
    fs::create_dir_all(godo_dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        process::Command,
    };

    use tempfile::{TempDir, tempdir};

    use super::*;
    use crate::session::{ReleaseOutcome, SessionManager};

    struct DirGuard {
        original: PathBuf,
    }

    impl DirGuard {
        fn change_to(target: &Path) -> Self {
            let original = env::current_dir().unwrap();
            env::set_current_dir(target).unwrap();
            Self { original }
        }
    }

    impl Drop for DirGuard {
        fn drop(&mut self) {
            env::set_current_dir(&self.original).unwrap();
        }
    }

    fn run_git(repo_dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(repo_dir)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {} failed", args.join(" "));
    }

    fn init_repo(repo_dir: &Path) {
        fs::create_dir_all(repo_dir).unwrap();
        run_git(repo_dir, &["init", "-b", "main"]);
        run_git(repo_dir, &["config", "user.email", "test@example.com"]);
        run_git(repo_dir, &["config", "user.name", "Test User"]);
        fs::write(repo_dir.join("README.md"), "base").unwrap();
        run_git(repo_dir, &["add", "README.md"]);
        run_git(repo_dir, &["commit", "-m", "Initial commit"]);
    }

    fn init_repo_with_origin(tmp: &TempDir) -> (PathBuf, PathBuf) {
        let origin_dir = tmp.path().join("origin.git");
        fs::create_dir_all(&origin_dir).unwrap();
        run_git(&origin_dir, &["init", "--bare"]);

        let repo_dir = tmp.path().join("repo");
        init_repo(&repo_dir);
        run_git(
            &repo_dir,
            &["remote", "add", "origin", origin_dir.to_str().unwrap()],
        );
        run_git(&repo_dir, &["push", "-u", "origin", "main"]);

        (repo_dir, origin_dir)
    }

    #[test]
    fn test_clean_name() {
        let test_cases = vec![
            // (input, expected)
            // Valid characters (should not change)
            ("valid-name", "valid-name"),
            ("valid_name", "valid_name"),
            ("ValidName123", "ValidName123"),
            ("a-b_c123", "a-b_c123"),
            // Special characters replacement
            ("my.project", "my-project"),
            ("my project", "my-project"),
            ("my@project!", "my-project-"),
            ("my/project", "my-project"),
            (r"my\project", "my-project"),
            ("my:project", "my-project"),
            ("my*project", "my-project"),
            ("my?project", "my-project"),
            ("my\"project", "my-project"),
            ("my<project>", "my-project-"),
            ("my|project", "my-project"),
            // Unicode and non-ASCII characters
            ("café", "caf-"),
            ("项目", "--"),
            ("проект", "------"),
            ("🚀rocket", "-rocket"),
            // Multiple consecutive special characters
            ("my...project", "my---project"),
            ("my   project", "my---project"),
            ("my@#$%project", "my----project"),
            // Edge cases
            ("", ""),
            ("-", "-"),
            ("_", "_"),
            ("---", "---"),
            ("@#$%", "----"),
            // Beginning and ending special characters
            (".project", "-project"),
            ("project.", "project-"),
            (".project.", "-project-"),
            // Real-world examples
            ("my-awesome-project", "my-awesome-project"),
            ("MyCompany.Project", "MyCompany-Project"),
            ("2024-project", "2024-project"),
            ("project (copy)", "project--copy-"),
            ("project [v2]", "project--v2-"),
        ];

        for (input, expected) in test_cases {
            assert_eq!(clean_name(input), expected, "Failed for input: '{input}'");
        }
    }

    #[test]
    fn sandbox_component_status_reports_components() {
        let sandbox = SandboxStatus {
            name: "example".to_string(),
            has_branch: true,
            has_worktree: true,
            has_worktree_dir: true,
            worktree_branch: None,
            worktree_detached: true,
            worktree_branch_matches: true,
            has_uncommitted_changes: false,
            diff_stats: None,
            merge_status: MergeStatus::Unknown,
            unmerged_commits: Vec::new(),
            is_dangling: false,
        };

        assert_eq!(
            sandbox.component_status(),
            "branch: present, worktree: present, directory: present, worktree-branch: detached"
        );
    }

    #[test]
    fn internal_dirs_are_not_listed_as_sandbox() {
        let tmp = tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        init_repo(&repo_dir);

        let godo_dir = tmp.path().join("godo");
        let manager = Godo::new(godo_dir, Some(repo_dir)).unwrap();

        // Create project dir and internal dirs inside it
        let project_dir = manager.project_dir().unwrap();
        fs::create_dir_all(project_dir.join(Godo::LEASE_DIR)).unwrap();
        fs::create_dir_all(project_dir.join(Godo::METADATA_DIR)).unwrap();
        fs::create_dir_all(project_dir.join("real-sandbox")).unwrap();

        let names = manager.all_sandbox_names().unwrap();
        assert!(names.contains(&"real-sandbox".to_string()));
        assert!(!names.contains(&Godo::LEASE_DIR.to_string()));
        assert!(!names.contains(&Godo::METADATA_DIR.to_string()));
    }

    #[test]
    fn resolve_base_commit_requires_metadata() {
        let tmp = tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        init_repo(&repo_dir);

        let godo_dir = tmp.path().join("godo");
        let manager = Godo::new(godo_dir, Some(repo_dir.clone())).unwrap();

        let sandbox_path = manager.sandbox_path("box").unwrap();
        git::create_worktree(&repo_dir, &sandbox_path, &branch_name("box")).unwrap();

        let result = manager.resolve_base_commit("box", None);
        assert!(matches!(result, Err(GodoError::BaseError { .. })));
    }

    #[test]
    fn resolve_base_commit_uses_override_without_metadata() {
        let tmp = tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        init_repo(&repo_dir);

        let godo_dir = tmp.path().join("godo");
        let manager = Godo::new(godo_dir, Some(repo_dir.clone())).unwrap();

        let sandbox_path = manager.sandbox_path("box").unwrap();
        git::create_worktree(&repo_dir, &sandbox_path, &branch_name("box")).unwrap();

        let expected = git::rev_parse(&repo_dir, "HEAD").unwrap();
        let resolved = manager.resolve_base_commit("box", Some("HEAD")).unwrap();
        assert_eq!(resolved.commit, expected);
        assert!(!resolved.used_fallback);
    }

    #[test]
    fn resolve_base_commit_falls_back_to_merge_base() {
        let tmp = tempdir().unwrap();
        let (repo_dir, _origin_dir) = init_repo_with_origin(&tmp);

        let godo_dir = tmp.path().join("godo");
        let manager = Godo::new(godo_dir, Some(repo_dir.clone())).unwrap();

        let sandbox_path = manager.sandbox_path("box").unwrap();
        git::create_worktree(&repo_dir, &sandbox_path, &branch_name("box")).unwrap();

        let metadata = SandboxMetadata {
            base_commit: "deadbeef".to_string(),
            base_ref: None,
            created_at: 1_700_000_000,
        };
        manager
            .metadata_store()
            .unwrap()
            .write("box", &metadata)
            .unwrap();

        let resolved = manager.resolve_base_commit("box", None).unwrap();
        // Should fallback to merge-base with a detected integration target
        // (could be "main", "origin/main", etc. depending on git config)
        assert!(resolved.used_fallback);
        let target = resolved.fallback_target.as_deref().unwrap();

        // Verify the commit matches what merge-base would return for that target
        let expected = git::merge_base(&repo_dir, &branch_name("box"), target).unwrap();
        assert_eq!(resolved.commit, expected);

        // Verify the target is one of the expected candidates
        let valid_targets = ["main", "master", "origin/main", "origin/master"];
        assert!(
            valid_targets.contains(&target),
            "unexpected fallback target: {target}"
        );
    }

    #[test]
    fn resolve_base_commit_prefers_recorded_base_ref() {
        let tmp = tempdir().unwrap();
        let (repo_dir, _origin_dir) = init_repo_with_origin(&tmp);

        let initial_commit = git::rev_parse(&repo_dir, "HEAD").unwrap();
        run_git(&repo_dir, &["checkout", "-b", "dev"]);
        run_git(&repo_dir, &["push", "-u", "origin", "dev"]);

        run_git(&repo_dir, &["checkout", "main"]);
        fs::write(repo_dir.join("main.txt"), "main update").unwrap();
        run_git(&repo_dir, &["add", "main.txt"]);
        run_git(&repo_dir, &["commit", "-m", "Update main"]);
        run_git(&repo_dir, &["push", "origin", "main"]);

        let godo_dir = tmp.path().join("godo");
        let manager = Godo::new(godo_dir, Some(repo_dir.clone())).unwrap();

        let sandbox_path = manager.sandbox_path("box").unwrap();
        git::create_worktree(&repo_dir, &sandbox_path, &branch_name("box")).unwrap();

        let metadata = SandboxMetadata {
            base_commit: "deadbeef".to_string(),
            base_ref: Some("origin/dev".to_string()),
            created_at: 1_700_000_000,
        };
        manager
            .metadata_store()
            .unwrap()
            .write("box", &metadata)
            .unwrap();

        let resolved = manager.resolve_base_commit("box", None).unwrap();
        assert_eq!(resolved.commit, initial_commit);
        assert!(resolved.used_fallback);
        assert_eq!(resolved.fallback_target.as_deref(), Some("origin/dev"));
    }

    #[test]
    fn session_manager_counts_connections() {
        let tmp = tempdir().unwrap();
        let manager = SessionManager::new(tmp.path());

        let lease1 = manager.lock("box").unwrap().acquire_lease().unwrap();
        let lease2 = manager.lock("box").unwrap().acquire_lease().unwrap();

        assert_eq!(manager.active_connections("box").unwrap(), 2);

        drop(lease1);
        assert_eq!(manager.active_connections("box").unwrap(), 1);

        match lease2.release().unwrap() {
            ReleaseOutcome::Last(_guard) => {} // Expected
            _ => panic!("expected last lease"),
        }

        assert_eq!(manager.active_connections("box").unwrap(), 0);
    }

    #[test]
    fn detects_missing_worktree_directory() {
        let tmp = tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        fs::create_dir(&repo_dir).unwrap();
        Command::new("git")
            .arg("init")
            .current_dir(&repo_dir)
            .status()
            .unwrap();

        let godo_dir = tmp.path().join("godo");
        let manager = Godo::new(godo_dir, Some(repo_dir.clone())).unwrap();

        let sandbox_path = manager.sandbox_path("box").unwrap();
        git::create_worktree(&repo_dir, &sandbox_path, &branch_name("box")).unwrap();

        fs::remove_dir_all(&sandbox_path).unwrap();

        let sandbox = manager.get_sandbox("box").unwrap().unwrap();
        assert!(sandbox.is_dangling);
        assert!(!sandbox.is_live());
    }

    #[test]
    fn test_project_name() {
        let test_cases = vec![
            // (path, expected)
            // Normal project names
            ("/home/user/projects/my-project", "my-project"),
            ("/home/user/projects/my_project", "my_project"),
            ("/home/user/projects/MyProject123", "MyProject123"),
            // Names with special characters that get replaced
            ("/home/user/projects/my.project", "my-project"),
            ("/home/user/projects/my project", "my-project"),
            ("/home/user/projects/my@project!", "my-project-"),
            // Edge cases
            ("/", "root"),
            ("project", "project"),
            // Paths that result in all special chars being replaced
            ("/home/user/projects/...", "---"),
            ("/home/user/projects/@#$%^&*()", "---------"),
        ];

        for (path, expected) in test_cases {
            let result = project_name(&PathBuf::from(path)).unwrap();
            assert_eq!(result, expected, "Failed for path: '{path}'");
        }
    }

    #[test]
    fn test_validate_sandbox_name() {
        let valid_names = vec![
            "test",
            "test-123",
            "test_123",
            "TEST",
            "a1b2c3",
            "my-feature-branch",
            "bug_fix_123",
            "RELEASE-2024",
        ];

        let invalid_names = vec![
            "",              // empty
            "test space",    // contains space
            "test.dot",      // contains dot
            "test@symbol",   // contains @
            "test/slash",    // contains /
            r"test\back",    // contains backslash
            "test:colon",    // contains colon
            "test*star",     // contains asterisk
            "test?question", // contains question mark
            "test|pipe",     // contains pipe
        ];

        for name in valid_names {
            assert!(
                validate_sandbox_name(name).is_ok(),
                "Expected '{name}' to be valid"
            );
        }

        for name in invalid_names {
            assert!(
                validate_sandbox_name(name).is_err(),
                "Expected '{name}' to be invalid"
            );
        }
    }

    #[test]
    fn test_branch_name() {
        let test_cases = vec![
            ("test", "godo/test"),
            ("feature-123", "godo/feature-123"),
            ("my_sandbox", "godo/my_sandbox"),
            ("bugfix", "godo/bugfix"),
            ("RELEASE-2024", "godo/RELEASE-2024"),
        ];

        for (sandbox, expected) in test_cases {
            assert_eq!(
                branch_name(sandbox),
                expected,
                "Failed for sandbox name: '{sandbox}'"
            );
        }
    }

    #[test]
    fn test_sandbox_and_project_paths() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let godo_dir = temp_dir.path().join(".godo");
        let repo_dir = PathBuf::from("/home/user/projects/my-project");

        // Create a Godo instance
        let godo = Godo::new(godo_dir.clone(), Some(repo_dir)).unwrap();

        // Test project_dir method
        let project_dir = godo.project_dir().unwrap();
        let canonical_godo_dir = fs::canonicalize(&godo_dir).unwrap_or(godo_dir);
        assert_eq!(project_dir, canonical_godo_dir.join("my-project"));

        // Test sandbox_path method
        let sandbox_path = godo.sandbox_path("test-sandbox").unwrap();
        assert_eq!(
            sandbox_path,
            canonical_godo_dir.join("my-project").join("test-sandbox")
        );
    }

    #[test]
    fn prepare_sandbox_reuses_sandbox_with_relative_godo_dir() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo_dir = temp_dir.path().join("repo");
        fs::create_dir_all(&repo_dir).unwrap();

        Command::new("git")
            .current_dir(&repo_dir)
            .args(["init", "-b", "main"])
            .output()
            .unwrap();
        Command::new("git")
            .current_dir(&repo_dir)
            .args(["config", "user.email", "test@example.com"])
            .output()
            .unwrap();
        Command::new("git")
            .current_dir(&repo_dir)
            .args(["config", "user.name", "Test User"])
            .output()
            .unwrap();

        fs::write(repo_dir.join("README.md"), "sandbox test").unwrap();
        Command::new("git")
            .current_dir(&repo_dir)
            .args(["add", "README.md"])
            .output()
            .unwrap();
        Command::new("git")
            .current_dir(&repo_dir)
            .args(["commit", "-m", "Initial commit"])
            .output()
            .unwrap();

        let _guard = DirGuard::change_to(&repo_dir);

        // Use a godo dir outside the repo to avoid "destination inside source" errors
        let godo_dir = temp_dir.path().join("godo-outside");
        let godo = Godo::new(godo_dir, Some(repo_dir)).unwrap();

        let plan = godo
            .prepare_sandbox(
                "test-sandbox",
                PrepareSandboxOptions {
                    uncommitted_policy: UncommittedPolicy::Include,
                    excludes: Vec::new(),
                },
            )
            .unwrap();
        assert!(plan.created);

        let sandbox_path = plan.session.path.clone();
        Command::new("git")
            .current_dir(&sandbox_path)
            .args(["checkout", "--detach", "HEAD"])
            .status()
            .unwrap();

        let _ = plan.session.release().unwrap();

        let plan = godo
            .prepare_sandbox(
                "test-sandbox",
                PrepareSandboxOptions {
                    uncommitted_policy: UncommittedPolicy::Include,
                    excludes: Vec::new(),
                },
            )
            .unwrap();
        assert!(!plan.created);
        let _ = plan.session.release().unwrap();

        let removal_plan = godo.removal_plan("test-sandbox").unwrap();
        let outcome = godo
            .remove(&removal_plan, &RemovalOptions::force())
            .unwrap();
        assert!(matches!(outcome, RemovalOutcome::Removed));
    }
}
