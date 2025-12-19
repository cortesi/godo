use std::{
    collections::HashSet,
    env, fs, io,
    os::unix::fs::symlink,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    result::Result as StdResult,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use clonetree::{Options, clone_tree};
use liboutput::{Output, OutputError as OutputErr};
use thiserror::Error;

use crate::{
    git::{self, MergeStatus},
    metadata::{SandboxMetadata, SandboxMetadataStore},
    session::{LEASE_DIR_NAME, ReleaseOutcome, SessionManager},
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

    /// The selected output backend reported an error.
    #[error("Output operation failed: {0}")]
    OutputError(#[from] OutputErr),

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
            Self::BaseError { .. } => 3,
            Self::GitError(_) => 4,
            _ => 1,
        }
    }
}

/// Follow-up action to take after executing a sandboxed command.
#[derive(Clone)]
enum PostRunAction {
    /// Commit all tracked changes within the sandbox.
    Commit,
    /// Re-open an interactive shell in the sandbox.
    Shell,
    /// Leave the sandbox intact without committing.
    Keep,
    /// Discard the sandbox entirely.
    Discard,
    /// Create a branch from the sandbox contents.
    Branch,
}

/// Status information for a sandbox
struct Sandbox {
    /// The name of the sandbox
    name: String,
    /// Whether the branch exists
    has_branch: bool,
    /// Whether the worktree exists
    has_worktree: bool,
    /// Whether the worktree directory path exists
    has_worktree_dir: bool,
    /// Branch currently checked out in the worktree, sans refs prefix, when known.
    worktree_branch: Option<String>,
    /// Whether the worktree is in detached HEAD state.
    worktree_detached: bool,
    /// Whether the worktree is checking out the expected sandbox branch when attached.
    worktree_branch_matches: bool,
    /// Whether there are any staged or unstaged uncommitted changes in the worktree
    has_uncommitted_changes: bool,
    /// Diff statistics for uncommitted changes (lines added/removed)
    diff_stats: Option<git::DiffStats>,
    /// Merge relationship between the sandbox branch and its integration target
    merge_status: MergeStatus,
    /// Commits not yet merged into the integration target
    unmerged_commits: Vec<git::CommitInfo>,
    /// Whether the worktree is dangling (no backing directory)
    is_dangling: bool,
}

impl Sandbox {
    /// Returns true if the sandbox has both a worktree and a branch
    fn is_live(&self) -> bool {
        self.has_branch
            && self.has_worktree
            && self.has_worktree_dir
            && (self.worktree_detached || self.worktree_branch_matches)
    }

    /// Summarize which sandbox components are currently present.
    fn component_status(&self) -> String {
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

    /// Display the sandbox status using the provided output.
    /// Only shows information that requires user attention.
    fn show(&self, output: &dyn Output, connections: usize) -> Result<()> {
        // Check if there are any issues to report
        let has_unmerged = self.has_branch && matches!(self.merge_status, MergeStatus::Diverged);
        let has_uncommitted = self.has_worktree && self.has_uncommitted_changes;

        // Always use section for consistent visual hierarchy (bold name)
        let section = output.section(&self.name);

        // Show the branch name
        if let Some(branch) = &self.worktree_branch {
            section.item("branch", branch)?;
        } else if self.worktree_detached {
            section.item("branch", "(detached HEAD)")?;
        } else if self.has_branch {
            section.item("branch", &format!("godo/{}", self.name))?;
        }

        if connections > 0 {
            let label = if connections == 1 {
                "1 active connection".to_string()
            } else {
                format!("{connections} active connections")
            };
            section.message(&label)?;
        }
        if has_unmerged {
            for commit in &self.unmerged_commits {
                section.commit(
                    &commit.short_hash,
                    &commit.subject,
                    commit.insertions,
                    commit.deletions,
                )?;
            }
        }
        if has_uncommitted {
            if let Some(stats) = &self.diff_stats {
                section.diff_stat("uncommitted changes", stats.insertions, stats.deletions)?;
            } else {
                section.warn("uncommitted changes")?;
            }
        }
        if self.is_dangling {
            section.fail("dangling worktree")?;
        }

        Ok(())
    }
}

/// Target ref used when falling back to merge-base for diff resolution.
const DIFF_MERGE_BASE_TARGET: &str = "origin/main";

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

/// Pager configuration for diff commands.
struct DiffPager {
    /// Pager command override, if any.
    pager: Option<String>,
    /// Whether paging is disabled.
    no_pager: bool,
}

impl DiffPager {
    /// Create a new pager configuration.
    fn new(pager: Option<String>, no_pager: bool) -> Self {
        Self { pager, no_pager }
    }

    /// Apply pager arguments to a git command.
    fn apply(&self, command: &mut Command) {
        if self.no_pager {
            command.arg("--no-pager");
        }
        if let Some(pager) = &self.pager {
            command.arg("-c");
            command.arg(format!("core.pager={pager}"));
        }
    }
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
/// operations to create a sandbox, run commands inside it, and to list,
/// remove, or clean existing sandboxes.
pub struct Godo {
    /// Base directory where per-project sandbox directories live.
    godo_dir: PathBuf,
    /// Root of the Git repository the sandboxes operate on.
    repo_dir: PathBuf,
    /// Whether user prompts should be skipped in favor of defaults.
    no_prompt: bool,
    /// Output channel used for rendering status and prompts.
    output: Arc<dyn Output>,
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
    /// - `output`: output implementation used for user-facing messages
    /// - `no_prompt`: when `true`, avoid interactive prompts and assume
    ///   conservative defaults
    pub fn new(
        godo_dir: PathBuf,
        repo_dir: Option<PathBuf>,
        output: Arc<dyn Output>,
        no_prompt: bool,
    ) -> anyhow::Result<Self> {
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

        Ok(Self {
            godo_dir,
            repo_dir,
            no_prompt,
            output,
        })
    }

    /// Get the project directory path within the godo directory
    fn project_dir(&self) -> Result<PathBuf> {
        let project = project_name(&self.repo_dir)?;
        Ok(self.godo_dir.join(&project))
    }

    /// Calculate the path for a sandbox given its name
    fn sandbox_path(&self, sandbox_name: &str) -> Result<PathBuf> {
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

    /// Wrapper around `Output::select` that returns `None` if the user cancels.
    fn prompt_select_opt(&self, prompt: &str, options: Vec<String>) -> Result<Option<usize>> {
        match self.output.select(prompt, options) {
            Ok(selection) => Ok(Some(selection)),
            Err(OutputErr::Cancelled) => Ok(None),
            Err(other) => Err(GodoError::OutputError(other)),
        }
    }

    /// Wrapper around `Output::select` that maps cancellation to `UserAborted`.
    fn prompt_select(&self, prompt: &str, options: Vec<String>) -> Result<usize> {
        self.prompt_select_opt(prompt, options)?
            .ok_or(GodoError::UserAborted)
    }

    /// Wrapper around `Output::confirm` that maps cancellation to `UserAborted`.
    fn prompt_confirm(&self, prompt: &str) -> Result<bool> {
        self.output.confirm(prompt).map_err(|err| match err {
            OutputErr::Cancelled => GodoError::UserAborted,
            other => GodoError::OutputError(other),
        })
    }

    /// Prompt the user for what to do after command execution
    fn prompt_for_action(&self, sandbox_path: &Path, sandbox_name: &str) -> Result<PostRunAction> {
        // Check current state
        let has_uncommitted =
            git::has_uncommitted_changes(sandbox_path).map_err(|e| git_error(&e))?;

        let branch = branch_name(sandbox_name);
        let merge_status =
            git::branch_merge_status(&self.repo_dir, &branch).unwrap_or(MergeStatus::Unknown);
        let has_unmerged = matches!(merge_status, MergeStatus::Diverged);

        // Build prompt message
        let prompt = match (has_uncommitted, has_unmerged) {
            (true, true) => "Uncommitted changes and unmerged commits. What next?",
            (true, false) => "Uncommitted changes. What next?",
            (false, true) => "Unmerged commits. What next?",
            (false, false) => "What next?",
        };

        // Build options based on state - only show relevant actions
        let mut options = Vec::new();
        let mut actions = Vec::new();

        // Commit only makes sense if there are uncommitted changes
        if has_uncommitted {
            options.push("Commit all changes".to_string());
            actions.push(PostRunAction::Commit);
        }

        // Shell is always available
        options.push("Drop to shell".to_string());
        actions.push(PostRunAction::Shell);

        // Keep is always available
        options.push("Keep sandbox".to_string());
        actions.push(PostRunAction::Keep);

        // Discard is always available (discards everything)
        options.push("Discard everything".to_string());
        actions.push(PostRunAction::Discard);

        // Branch only makes sense if there are unmerged commits worth keeping
        if has_unmerged {
            options.push("Keep branch only".to_string());
            actions.push(PostRunAction::Branch);
        }

        let selection = self.prompt_select_opt(prompt, options)?;
        match selection {
            Some(idx) => Ok(actions[idx].clone()),
            None => Ok(PostRunAction::Shell),
        }
    }

    /// Create or reuse a sandbox and run a command or interactive shell in it.
    ///
    /// - `keep`: keep the sandbox after the command completes when `true`
    /// - `commit`: optional commit message to commit all changes automatically
    /// - `excludes`: glob patterns to exclude when cloning the tree into the sandbox
    /// - `sandbox_name`: the logical name of the sandbox (used for branch/worktree)
    /// - `command`: command to execute; an empty slice starts an interactive shell
    pub fn run(
        &self,
        keep: bool,
        commit: Option<String>,
        force_shell: bool,
        excludes: &[String],
        sandbox_name: &str,
        command: &[String],
    ) -> Result<()> {
        // Validate sandbox name
        validate_sandbox_name(sandbox_name)?;

        let sandbox_path = self.sandbox_path(sandbox_name)?;
        let project_dir = self.project_dir()?;
        let session_manager = SessionManager::new(&project_dir);

        // Acquire lock to ensure exclusive access during creation/verification
        let locked_session = session_manager.lock(sandbox_name)?;

        let existing_sandbox = self.get_sandbox(sandbox_name)?;

        let mut use_clean_branch = false;

        if let Some(sandbox) = existing_sandbox {
            // Sandbox exists, check if it's live
            if sandbox.is_live() {
                self.output.message(&format!(
                    "Using existing sandbox {sandbox_name} at {sandbox_path:?}"
                ))?;
            } else {
                let status = sandbox.component_status();
                return Err(GodoError::SandboxError {
                    name: sandbox_name.to_string(),
                    message: format!("exists but is not live - remove it first ({status})"),
                });
            }
        } else {
            // Sandbox doesn't exist, create it
            // Check for uncommitted changes
            if git::has_uncommitted_changes(&self.repo_dir).map_err(|e| git_error(&e))? {
                self.output.warn("You have uncommitted changes.")?;

                if !self.no_prompt {
                    let options = vec![
                        "Abort".to_string(),
                        "Include uncommitted changes".to_string(),
                        "Start clean (HEAD only)".to_string(),
                    ];

                    match self.prompt_select("Uncommitted changes in working tree", options)? {
                        0 => return Err(GodoError::UserAborted), // Abort
                        1 => {} // Continue - do nothing, proceed with normal flow
                        2 => {
                            // Use clean branch - we'll reset after creating the worktree
                            use_clean_branch = true;
                        }
                        _ => unreachable!("Invalid selection"),
                    }
                }
            }

            // Ensure project directory exists
            let project_dir = self.project_dir()?;
            fs::create_dir_all(&project_dir)?;

            let base_commit = git::rev_parse(&self.repo_dir, "HEAD").map_err(|e| git_error(&e))?;
            let base_ref = git::head_ref(&self.repo_dir).map_err(|e| git_error(&e))?;

            let branch = branch_name(sandbox_name);
            self.output.message(&format!(
                "Creating sandbox {sandbox_name} with branch {branch} at {sandbox_path:?}"
            ))?;
            git::create_worktree(&self.repo_dir, &sandbox_path, &branch)
                .map_err(|e| git_error(&e))?;

            let spinner = self.output.spinner("Cloning tree to sandbox...");

            // Clone each top-level entry from repo to sandbox, skipping .git.
            // We do this entry-by-entry because clone_tree requires the destination
            // not to exist, but the worktree already created the sandbox with .git.
            let clone_result: Result<()> = (|| {
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
                                std::os::windows::fs::symlink_dir(&target, &dest)?;
                            } else {
                                std::os::windows::fs::symlink_file(&target, &dest)?;
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
                Ok(())
            })();

            match clone_result {
                Ok(()) => spinner.finish_success("Sandbox ready"),
                Err(e) => {
                    spinner.finish_fail("Clone failed");
                    return Err(e);
                }
            }

            // If user chose to use clean branch, reset the sandbox to remove uncommitted changes
            if use_clean_branch {
                self.output.message("Resetting sandbox to clean state...")?;
                git::reset_hard(&sandbox_path)
                    .map_err(|e| GodoError::GitError(format!("Failed to reset sandbox: {e}")))?;
                git::clean(&sandbox_path)
                    .map_err(|e| GodoError::GitError(format!("Failed to clean sandbox: {e}")))?;
                self.output.success("Sandbox is now in a clean state")?;
            }

            self.record_metadata(sandbox_name, base_commit, base_ref)?;
        }

        let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());

        // Acquire session lease to track concurrent connections.
        let lease = locked_session.acquire_lease()?;
        let status = if command.is_empty() {
            // Interactive shell
            Command::new(&shell)
                .current_dir(&sandbox_path)
                .status()
                .map_err(|e| GodoError::OperationError(format!("Failed to start shell: {e}")))?
        } else if force_shell {
            // Force shell evaluation (e.g., pipes, globs). Users should quote the entire command
            // in their outer shell, e.g.: --sh 'echo "a b" | wc -w'
            let command_string = command.join(" ");
            Command::new(&shell)
                .arg("-c")
                .arg(&command_string)
                .current_dir(&sandbox_path)
                .status()
                .map_err(|e| GodoError::OperationError(format!("Failed to run command: {e}")))?
        } else {
            // Exec program directly to preserve argument boundaries and quoting
            let program = &command[0];
            let args = &command[1..];
            match Command::new(program)
                .args(args)
                .current_dir(&sandbox_path)
                .status()
            {
                Ok(s) => s,
                Err(e) => {
                    // Map command-not-found to standard 127 like shells do
                    if e.kind() == io::ErrorKind::NotFound {
                        return Err(GodoError::CommandExit { code: 127 });
                    }
                    return Err(GodoError::OperationError(format!(
                        "Failed to run command: {e}"
                    )));
                }
            }
        };

        if !status.success() {
            // Extract the actual exit code
            let exit_code = status.code().unwrap_or(1);
            return Err(GodoError::CommandExit { code: exit_code });
        }

        let _cleanup_guard = match lease.release()? {
            ReleaseOutcome::NotLast => {
                self.output
                    .message("Another godo session is still attached; skipping cleanup.")?;
                return Ok(());
            }
            ReleaseOutcome::Last(guard) => guard,
        };

        // Check if sandbox is clean (no uncommitted changes, no unmerged commits)
        // If so, auto-delete without prompting
        if !keep && commit.is_none() {
            let has_uncommitted = git::has_uncommitted_changes(&sandbox_path).unwrap_or(false);
            let merge_status = git::branch_merge_status(&self.repo_dir, &branch_name(sandbox_name))
                .unwrap_or(MergeStatus::Unknown);
            let is_clean = !has_uncommitted && matches!(merge_status, MergeStatus::Clean);

            if is_clean {
                self.remove_sandbox(sandbox_name)?;
                return Ok(());
            }
        }

        // Handle automatic commit if specified
        if let Some(commit_message) = commit {
            self.output.message("Staging and committing changes...")?;
            // Stage all changes
            git::add_all(&sandbox_path).map_err(|e| git_error(&e))?;
            // Commit with the provided message
            git::commit(&sandbox_path, &commit_message).map_err(|e| git_error(&e))?;
            self.output
                .success(&format!("Committed with message: {commit_message}"))?;
            // Clean up after commit
            self.cleanup_sandbox(sandbox_name)?;
        } else if !keep {
            // If --no-prompt, choose a safe default: keep the sandbox.
            if self.no_prompt {
                self.output.success(&format!(
                    "Keeping sandbox. You can return to it at: {}",
                    sandbox_path.display()
                ))?;
            } else {
                // Prompt user for action if not explicitly keeping or committing
                loop {
                    let action = self.prompt_for_action(&sandbox_path, sandbox_name)?;
                    match action {
                        PostRunAction::Commit => {
                            self.output.message("Staging and committing changes...")?;
                            // Stage all changes
                            git::add_all(&sandbox_path).map_err(|e| git_error(&e))?;
                            // Commit with verbose flag
                            git::commit_interactive(&sandbox_path).map_err(|e| git_error(&e))?;
                            // Clean up after commit
                            self.cleanup_sandbox(sandbox_name)?;
                            break; // Exit the loop after commit
                        }
                        PostRunAction::Shell => {
                            self.output.message("Opening shell in sandbox...")?;
                            // Run interactive shell
                            let shell_status = Command::new(&shell)
                                .current_dir(&sandbox_path)
                                .status()
                                .map_err(|e| {
                                    GodoError::OperationError(format!("Failed to start shell: {e}"))
                                })?;

                            if !shell_status.success() {
                                self.output.warn("Shell exited with non-zero status")?;
                            }
                            // Continue the loop to show prompt again
                        }
                        PostRunAction::Keep => {
                            self.output.success(&format!(
                                "Keeping sandbox. You can return to it at: {}",
                                sandbox_path.display()
                            ))?;
                            break; // Exit the loop after keep
                        }
                        PostRunAction::Discard => {
                            // Confirm discard action
                            if !self.no_prompt
                                && !self.prompt_confirm("Discard all changes and delete branch?")?
                            {
                                // User cancelled, continue the loop to show prompt again
                                continue;
                            }
                            self.remove_sandbox(sandbox_name)?;
                            break; // Exit the loop after discard
                        }
                        PostRunAction::Branch => {
                            self.output
                                .message("Keeping branch but removing worktree...")?;
                            // Remove only the worktree, keeping the branch
                            git::remove_worktree(&self.repo_dir, &sandbox_path, true)
                                .map_err(|e| git_error(&e))?;
                            if sandbox_path.exists() {
                                fs::remove_dir_all(&sandbox_path).map_err(|e| {
                                    GodoError::OperationError(format!(
                                        "Failed to remove sandbox directory: {e}"
                                    ))
                                })?;
                            }
                            self.remove_metadata(sandbox_name)?;
                            let branch = branch_name(sandbox_name);
                            self.output
                                .success(&format!("Worktree removed, branch {branch} kept"))?;
                            break; // Exit the loop after branch
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Diff a sandbox against its recorded base commit.
    pub fn diff(
        &self,
        sandbox_name: &str,
        base_override: Option<&str>,
        pager: Option<String>,
        no_pager: bool,
    ) -> Result<()> {
        validate_sandbox_name(sandbox_name)?;

        if no_pager && pager.is_some() {
            return Err(GodoError::OperationError(
                "Cannot combine --pager with --no-pager".to_string(),
            ));
        }

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

        if base.used_fallback {
            let target = base
                .fallback_target
                .as_deref()
                .unwrap_or(DIFF_MERGE_BASE_TARGET);
            self.output.warn(&format!(
                "Recorded base commit missing; using merge-base with {target}"
            ))?;
        }

        let pager = DiffPager::new(pager, no_pager);

        let tracked_args = vec!["diff".to_string(), base.commit];
        self.run_git_diff_command(&sandbox_path, &pager, &tracked_args)?;

        let untracked = git::untracked_files(&sandbox_path).map_err(|e| git_error(&e))?;
        for path in untracked {
            let diff_args = vec![
                "diff".to_string(),
                "--no-index".to_string(),
                "--".to_string(),
                "/dev/null".to_string(),
                path.to_string_lossy().to_string(),
            ];
            self.run_git_diff_command(&sandbox_path, &pager, &diff_args)?;
        }

        Ok(())
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
                if let Some(base_ref) = metadata.base_ref.as_ref() {
                    candidates.push(base_ref.clone());
                }
                if !candidates
                    .iter()
                    .any(|candidate| candidate == DIFF_MERGE_BASE_TARGET)
                {
                    candidates.push(DIFF_MERGE_BASE_TARGET.to_string());
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

    /// Run a git diff command, treating exit codes 0 and 1 as success.
    fn run_git_diff_command(
        &self,
        sandbox_path: &Path,
        pager: &DiffPager,
        args: &[String],
    ) -> Result<()> {
        let mut command = Command::new("git");
        command.current_dir(sandbox_path);
        pager.apply(&mut command);
        command.args(args);
        command.stdin(Stdio::inherit());
        command.stdout(Stdio::inherit());
        command.stderr(Stdio::inherit());

        let status = command.status().map_err(|e| {
            GodoError::GitError(format!("Failed to run git {}: {e}", args.join(" ")))
        })?;

        match status.code() {
            Some(0) | Some(1) => Ok(()),
            Some(code) => Err(GodoError::GitError(format!(
                "Git diff failed with exit code {code}"
            ))),
            None => Err(GodoError::GitError(
                "Git diff terminated by signal".to_string(),
            )),
        }
    }

    /// Get the status of a sandbox by name
    fn get_sandbox(&self, name: &str) -> Result<Option<Sandbox>> {
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

        Ok(Some(Sandbox {
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
    pub fn list(&self) -> Result<()> {
        let sorted_names = self.all_sandbox_names()?;
        let project_dir = self.project_dir()?;
        let session_manager = SessionManager::new(&project_dir);

        if sorted_names.is_empty() {
            self.output.message("No sandboxes found.")?;
            return Ok(());
        }

        // Display each sandbox with its attributes
        for name in &sorted_names {
            match self.get_sandbox(name)? {
                Some(status) => {
                    let connections = session_manager.active_connections(name)?;
                    status.show(&*self.output, connections)?;
                }
                None => {
                    // Skip sandboxes that don't exist
                    continue;
                }
            }
        }

        Ok(())
    }

    /// Remove a sandbox's worktree and branch.
    ///
    /// If `force` is `false`, the user is prompted when uncommitted changes or
    /// unmerged commits are detected.
    pub fn remove(&self, name: &str, force: bool) -> Result<()> {
        let status = match self.get_sandbox(name)? {
            Some(s) => s,
            None => {
                return Err(GodoError::SandboxError {
                    name: name.to_string(),
                    message: "does not exist".to_string(),
                });
            }
        };

        if !force {
            if status.has_uncommitted_changes {
                if self.no_prompt {
                    return Err(GodoError::SandboxError {
                        name: name.to_string(),
                        message: "has uncommitted changes (use --force to remove)".to_string(),
                    });
                }
                if !self.prompt_confirm("Uncommitted changes will be lost. Continue?")? {
                    return Err(GodoError::UserAborted);
                }
            }
            if matches!(
                status.merge_status,
                MergeStatus::Diverged | MergeStatus::Unknown
            ) {
                let warning = if matches!(status.merge_status, MergeStatus::Unknown) {
                    "branch merge status is unknown (use --force to remove)"
                } else {
                    "branch has unmerged commits (use --force to remove)"
                };
                if self.no_prompt {
                    return Err(GodoError::SandboxError {
                        name: name.to_string(),
                        message: warning.to_string(),
                    });
                }
                let prompt = if matches!(status.merge_status, MergeStatus::Unknown) {
                    "Merge status unknown (commits may be lost). Continue?"
                } else {
                    "Unmerged commits will be lost. Continue?"
                };
                if !self.prompt_confirm(prompt)? {
                    return Err(GodoError::UserAborted);
                }
            }
        }

        self.remove_sandbox(name)
    }

    /// Clean one sandbox or all sandboxes by removing stale worktrees/branches
    /// when safe to do so.
    pub fn clean(&self, name: Option<&str>) -> Result<()> {
        match name {
            Some(name) => {
                // Clean specific sandbox
                let status = match self.get_sandbox(name)? {
                    Some(s) => s,
                    None => {
                        return Err(GodoError::SandboxError {
                            name: name.to_string(),
                            message: "does not exist".to_string(),
                        });
                    }
                };

                // Warn if there are uncommitted changes (only if worktree exists)
                if status.has_worktree
                    && status.has_uncommitted_changes
                    && !self.no_prompt
                    && !self.prompt_confirm("Uncommitted changes will be lost. Continue?")?
                {
                    return Err(GodoError::UserAborted);
                }

                self.cleanup_sandbox(name)
            }
            None => {
                // Clean all sandboxes
                let all_names = self.all_sandbox_names()?;

                if all_names.is_empty() {
                    self.output.message("No sandboxes to clean")?;
                    return Ok(());
                }

                self.output
                    .message(&format!("Cleaning {} sandboxes...", all_names.len()))?;

                for sandbox_name in all_names {
                    if let Err(e) = self.cleanup_sandbox(&sandbox_name) {
                        self.output
                            .warn(&format!("Failed to clean {sandbox_name}: {e}"))?;
                    }
                }

                Ok(())
            }
        }
    }

    /// Clean up a sandbox by removing worktree if no uncommitted changes and branch if no unmerged commits
    fn cleanup_sandbox(&self, name: &str) -> Result<()> {
        let status = match self.get_sandbox(name)? {
            Some(s) => s,
            None => {
                self.output.message(&format!("no such sandbox: {name}"))?;
                return Ok(());
            }
        };

        let section = self.output.section(&format!("cleaning sandbox: {name}"));

        let sandbox_path = self.sandbox_path(name)?;
        let branch = branch_name(name);

        let mut worktree_removed = false;
        let mut branch_removed = false;
        let mut directory_removed = false;

        // Remove the worktree if it exists and has no uncommitted changes
        if status.has_worktree && !status.has_uncommitted_changes {
            git::remove_worktree(&self.repo_dir, &sandbox_path, false)
                .map_err(|e| git_error(&e))?;
            section.message("removed unmodified worktree")?;
            worktree_removed = true;
        } else if status.has_worktree && status.has_uncommitted_changes {
            section.message("skipping worktree with uncommitted changes")?;
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

        // Report what was done
        if worktree_removed && branch_removed {
            section.success("unmodified sandbox and branch cleaned up")?;
        } else if worktree_removed && !branch_removed {
            section.success(&format!("worktree removed, branch {branch} kept"))?;
        } else if !status.has_worktree && branch_removed {
            section.success(&format!("fully merged branch {branch} removed"))?;
        } else if status.has_worktree && status.has_uncommitted_changes {
            section.warn("not cleaned: has uncommitted changes")?;
        } else if status.has_branch && matches!(status.merge_status, MergeStatus::Diverged) {
            section.warn(&format!("branch {branch} has unmerged commits"))?;
        } else if status.has_branch && matches!(status.merge_status, MergeStatus::Unknown) {
            section.warn(&format!(
                "branch {branch} kept because merge status could not be determined"
            ))?;
        } else {
            section.message("unchanged")?;
        }

        Ok(())
    }

    /// Remove a sandbox forcefully or fail if it has changes
    fn remove_sandbox(&self, name: &str) -> Result<()> {
        // Get sandbox status to check current state
        let status = match self.get_sandbox(name)? {
            Some(s) => s,
            None => {
                return Err(GodoError::SandboxError {
                    name: name.to_string(),
                    message: "does not exist".to_string(),
                });
            }
        };

        let sandbox_path = self.sandbox_path(name)?;
        let branch = branch_name(name);

        let spinner = self.output.spinner("Removing sandbox...");

        let result: Result<()> = (|| {
            if status.has_worktree {
                git::remove_worktree(&self.repo_dir, &sandbox_path, true)
                    .map_err(|e| git_error(&e))?;
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
        })();

        match result {
            Ok(()) => {
                spinner.finish_success("Sandbox removed");
                Ok(())
            }
            Err(e) => {
                spinner.finish_fail("Failed to remove sandbox");
                Err(e)
            }
        }
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
    use std::path::{Path, PathBuf};

    use liboutput::{Output, Quiet, Result as OutputResult, Spinner};
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
        let sandbox = Sandbox {
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
        let output: Arc<dyn Output> = Arc::new(Quiet);
        let manager = Godo::new(godo_dir, Some(repo_dir), output, true).unwrap();

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
        let output: Arc<dyn Output> = Arc::new(Quiet);
        let manager = Godo::new(godo_dir, Some(repo_dir.clone()), output, true).unwrap();

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
        let output: Arc<dyn Output> = Arc::new(Quiet);
        let manager = Godo::new(godo_dir, Some(repo_dir.clone()), output, true).unwrap();

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
        let output: Arc<dyn Output> = Arc::new(Quiet);
        let manager = Godo::new(godo_dir, Some(repo_dir.clone()), output, true).unwrap();

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
        let expected =
            git::merge_base(&repo_dir, &branch_name("box"), DIFF_MERGE_BASE_TARGET).unwrap();
        assert_eq!(resolved.commit, expected);
        assert!(resolved.used_fallback);
        assert_eq!(
            resolved.fallback_target.as_deref(),
            Some(DIFF_MERGE_BASE_TARGET)
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
        let output: Arc<dyn Output> = Arc::new(Quiet);
        let manager = Godo::new(godo_dir, Some(repo_dir.clone()), output, true).unwrap();

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
        let output: Arc<dyn Output> = Arc::new(Quiet);
        let manager = Godo::new(godo_dir, Some(repo_dir.clone()), output, true).unwrap();

        let sandbox_path = manager.sandbox_path("box").unwrap();
        git::create_worktree(&repo_dir, &sandbox_path, &branch_name("box")).unwrap();

        fs::remove_dir_all(&sandbox_path).unwrap();

        let sandbox = manager.get_sandbox("box").unwrap().unwrap();
        assert!(sandbox.is_dangling);
        assert!(!sandbox.is_live());
    }

    struct CancelOnSelectOutput;

    impl Output for CancelOnSelectOutput {
        fn message(&self, _msg: &str) -> OutputResult<()> {
            Ok(())
        }

        fn success(&self, _msg: &str) -> OutputResult<()> {
            Ok(())
        }

        fn warn(&self, _msg: &str) -> OutputResult<()> {
            Ok(())
        }

        fn fail(&self, _msg: &str) -> OutputResult<()> {
            Ok(())
        }

        fn item(&self, _key: &str, _value: &str) -> OutputResult<()> {
            Ok(())
        }

        fn diff_stat(
            &self,
            _label: &str,
            _insertions: usize,
            _deletions: usize,
        ) -> OutputResult<()> {
            Ok(())
        }

        fn commit(
            &self,
            _hash: &str,
            _subject: &str,
            _insertions: usize,
            _deletions: usize,
        ) -> OutputResult<()> {
            Ok(())
        }

        fn confirm(&self, _prompt: &str) -> OutputResult<bool> {
            Ok(false)
        }

        fn select(&self, _prompt: &str, _options: Vec<String>) -> OutputResult<usize> {
            Err(liboutput::OutputError::Cancelled)
        }

        fn finish(&self) -> OutputResult<()> {
            Ok(())
        }

        fn section(&self, _header: &str) -> Box<dyn Output> {
            Box::new(Self)
        }

        fn spinner(&self, _msg: &str) -> Box<dyn Spinner> {
            Box::new(QuietSpinner)
        }
    }

    struct QuietSpinner;

    impl Spinner for QuietSpinner {
        fn finish_success(self: Box<Self>, _msg: &str) {}

        fn finish_fail(self: Box<Self>, _msg: &str) {}

        fn finish_clear(self: Box<Self>) {}
    }

    #[test]
    fn cancel_in_post_run_action_reopens_shell() {
        let tmp = tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        fs::create_dir(&repo_dir).unwrap();

        Command::new("git")
            .arg("init")
            .current_dir(&repo_dir)
            .status()
            .unwrap();

        let godo_dir = tmp.path().join("godo");
        let output: Arc<dyn Output> = Arc::new(CancelOnSelectOutput);
        let manager = Godo::new(godo_dir, Some(repo_dir.clone()), output, false).unwrap();

        let action = manager.prompt_for_action(&repo_dir, "box").unwrap();
        assert!(matches!(action, PostRunAction::Shell));
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
        let output: Arc<dyn Output> = Arc::new(Quiet);
        let godo = Godo::new(godo_dir.clone(), Some(repo_dir), output, false).unwrap();

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
    fn run_reuses_sandbox_with_relative_godo_dir() {
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
        let output: Arc<dyn Output> = Arc::new(Quiet);
        let godo = Godo::new(godo_dir, Some(repo_dir), output, true).unwrap();

        let command = vec!["echo".to_string(), "hi".to_string()];
        godo.run(true, None, false, &[], "test-sandbox", &command)
            .unwrap();

        let sandbox_path = godo.sandbox_path("test-sandbox").unwrap();
        Command::new("git")
            .current_dir(&sandbox_path)
            .args(["checkout", "--detach", "HEAD"])
            .status()
            .unwrap();

        godo.run(true, None, false, &[], "test-sandbox", &command)
            .unwrap();

        godo.remove("test-sandbox", true).unwrap();
    }

    // Mock spinner for testing
    struct MockSpinner;

    impl Spinner for MockSpinner {
        fn finish_success(self: Box<Self>, _msg: &str) {}
        fn finish_fail(self: Box<Self>, _msg: &str) {}
        fn finish_clear(self: Box<Self>) {}
    }

    // Mock output for testing prompt_for_action
    struct MockOutput {
        selection: usize,
    }

    impl Output for MockOutput {
        fn message(&self, _msg: &str) -> OutputResult<()> {
            Ok(())
        }

        fn success(&self, _msg: &str) -> OutputResult<()> {
            Ok(())
        }

        fn warn(&self, _msg: &str) -> OutputResult<()> {
            Ok(())
        }

        fn fail(&self, _msg: &str) -> OutputResult<()> {
            Ok(())
        }

        fn item(&self, _key: &str, _value: &str) -> OutputResult<()> {
            Ok(())
        }

        fn diff_stat(
            &self,
            _label: &str,
            _insertions: usize,
            _deletions: usize,
        ) -> OutputResult<()> {
            Ok(())
        }

        fn commit(
            &self,
            _hash: &str,
            _subject: &str,
            _insertions: usize,
            _deletions: usize,
        ) -> OutputResult<()> {
            Ok(())
        }

        fn confirm(&self, _prompt: &str) -> OutputResult<bool> {
            Ok(true)
        }

        fn select(&self, _prompt: &str, _options: Vec<String>) -> OutputResult<usize> {
            Ok(self.selection)
        }

        fn finish(&self) -> OutputResult<()> {
            Ok(())
        }

        fn section(&self, _header: &str) -> Box<dyn Output> {
            Box::new(Self {
                selection: self.selection,
            })
        }

        fn spinner(&self, _msg: &str) -> Box<dyn Spinner> {
            Box::new(MockSpinner)
        }
    }

    #[test]
    fn test_prompt_for_action() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let godo_dir = temp_dir.path().join(".godo");
        let repo_dir = temp_dir.path().join("repo");

        // Initialize a git repository with an initial commit
        fs::create_dir_all(&repo_dir).unwrap();
        Command::new("git")
            .current_dir(&repo_dir)
            .args(["init"])
            .output()
            .unwrap();
        Command::new("git")
            .current_dir(&repo_dir)
            .args(["config", "user.email", "test@test.com"])
            .output()
            .unwrap();
        Command::new("git")
            .current_dir(&repo_dir)
            .args(["config", "user.name", "Test"])
            .output()
            .unwrap();
        fs::write(repo_dir.join("README.md"), "test").unwrap();
        Command::new("git")
            .current_dir(&repo_dir)
            .args(["add", "."])
            .output()
            .unwrap();
        Command::new("git")
            .current_dir(&repo_dir)
            .args(["commit", "-m", "init"])
            .output()
            .unwrap();

        // For a clean sandbox (no uncommitted, no unmerged), options are:
        // 0: shell, 1: keep, 2: discard
        // (no commit option since nothing to commit, no branch option since nothing unmerged)

        // Test selecting "Shell" (index 0 for clean sandbox)
        let output: Arc<dyn Output> = Arc::new(MockOutput { selection: 0 });
        let godo = Godo::new(godo_dir.clone(), Some(repo_dir.clone()), output, false).unwrap();
        let action = godo.prompt_for_action(&repo_dir, "test").unwrap();
        assert!(matches!(action, PostRunAction::Shell));

        // Test selecting "Keep" (index 1 for clean sandbox)
        let output: Arc<dyn Output> = Arc::new(MockOutput { selection: 1 });
        let godo = Godo::new(godo_dir.clone(), Some(repo_dir.clone()), output, false).unwrap();
        let action = godo.prompt_for_action(&repo_dir, "test").unwrap();
        assert!(matches!(action, PostRunAction::Keep));

        // Test selecting "Discard" (index 2 for clean sandbox)
        let output: Arc<dyn Output> = Arc::new(MockOutput { selection: 2 });
        let godo = Godo::new(godo_dir.clone(), Some(repo_dir.clone()), output, false).unwrap();
        let action = godo.prompt_for_action(&repo_dir, "test").unwrap();
        assert!(matches!(action, PostRunAction::Discard));

        // Now test with uncommitted changes - options become:
        // 0: commit, 1: shell, 2: keep, 3: discard
        fs::write(repo_dir.join("new_file.txt"), "changes").unwrap();

        // Test selecting "Commit" (index 0 with uncommitted changes)
        let output: Arc<dyn Output> = Arc::new(MockOutput { selection: 0 });
        let godo = Godo::new(godo_dir.clone(), Some(repo_dir.clone()), output, false).unwrap();
        let action = godo.prompt_for_action(&repo_dir, "test").unwrap();
        assert!(matches!(action, PostRunAction::Commit));

        // Test selecting "Shell" (index 1 with uncommitted changes)
        let output: Arc<dyn Output> = Arc::new(MockOutput { selection: 1 });
        let godo = Godo::new(godo_dir, Some(repo_dir.clone()), output, false).unwrap();
        let action = godo.prompt_for_action(&repo_dir, "test").unwrap();
        assert!(matches!(action, PostRunAction::Shell));
    }
}
