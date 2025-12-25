#![deny(missing_docs)]
#![deny(rustdoc::missing_crate_level_docs)]
//! Command-line interface for managing godo sandboxes via the libgodo crate.

use std::{
    env,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
    result::Result as StdResult,
    sync::Arc,
};

use anyhow::{Context, Result};
use clap::{ArgGroup, Parser, Subcommand};
use godo_term::{Output, OutputError, Quiet, Terminal};
use libgodo::{
    CleanupBatch, CleanupReport, DiffPlan, Godo, GodoError, MergeStatus, PrepareSandboxOptions,
    ReleaseOutcome, RemovalBlocker, RemovalOptions, RemovalOutcome, RemovalPlan, SandboxListEntry,
    UncommittedPolicy,
};

/// Default directory for storing godo-managed sandboxes.
const DEFAULT_GODO_DIR: &str = "~/.godo";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(group(
    ArgGroup::new("color_mode")
        .args(["color", "no_color"])
))]
/// Top-level CLI options for godo.
struct Cli {
    /// Override the godo directory location
    #[arg(long, global = true, value_name = "DIR")]
    dir: Option<String>,

    /// Override the repository directory (defaults to current git project)
    #[arg(long, global = true, value_name = "DIR")]
    repo_dir: Option<String>,

    /// Enable colored output
    #[arg(long, global = true)]
    color: bool,

    /// Disable colored output
    #[arg(long = "no-color", global = true)]
    no_color: bool,

    /// Suppress all output
    #[arg(long, global = true)]
    quiet: bool,

    /// Skip confirmation prompts
    #[arg(long, global = true)]
    no_prompt: bool,

    #[command(subcommand)]
    /// The primary command to execute.
    command: Commands,
}

#[derive(Subcommand)]
/// CLI subcommands supported by godo.
enum Commands {
    /// Run a command in an isolated workspace
    Run {
        /// Keep the sandbox after the command exits
        #[arg(long)]
        keep: bool,

        /// Automatically commit all changes with the specified message after command exits
        #[arg(long)]
        commit: Option<String>,

        /// Force shell evaluation with $SHELL -c
        #[arg(long = "sh")]
        sh: bool,

        /// Exclude directories that match glob (can be specified multiple times)
        #[arg(long = "exclude", value_name = "GLOB")]
        excludes: Vec<String>,

        /// Name of the sandbox
        name: String,

        /// Command to execute (if omitted, opens interactive shell)
        command: Vec<String>,
    },

    /// Show existing sandboxes
    #[command(alias = "ls")]
    List,

    /// Diff a sandbox against its recorded base commit
    Diff {
        /// Name of the sandbox to diff (auto-detected if running from within a sandbox)
        name: Option<String>,

        /// Override the base commit used for diffing
        #[arg(long, value_name = "COMMIT")]
        base: Option<String>,

        /// Override the pager command for diff output
        #[arg(long, value_name = "CMD", conflicts_with = "no_pager")]
        pager: Option<String>,

        /// Disable paging for diff output
        #[arg(long = "no-pager", conflicts_with = "pager")]
        no_pager: bool,
    },

    /// Delete a named sandbox
    #[command(alias = "rm")]
    Remove {
        /// Name of the sandbox to remove
        name: String,

        /// Force removal even if there are uncommitted changes
        #[arg(long)]
        force: bool,
    },

    /// Clean up a sandbox by removing worktree but keeping the branch
    Clean {
        /// Name of the sandbox to clean (if not specified, cleans all sandboxes)
        name: Option<String>,
    },
}

/// Follow-up action to take after executing a sandboxed command.
#[derive(Clone, Copy)]
enum PostRunAction {
    /// Commit all tracked changes within the sandbox.
    Commit,
    /// Re-open an interactive shell in the sandbox.
    Shell,
    /// Leave the sandbox intact without committing.
    Keep,
    /// Discard the sandbox entirely.
    Discard,
    /// Keep the branch but remove the worktree.
    Branch,
}

/// Parameters for the `godo run` command.
struct RunRequest {
    /// Keep the sandbox after command completion.
    keep: bool,
    /// Optional commit message for automatic commit.
    commit: Option<String>,
    /// Force shell execution.
    force_shell: bool,
    /// Directory exclusions to apply when cloning.
    excludes: Vec<String>,
    /// Name of the sandbox to operate on.
    sandbox_name: String,
    /// Command to execute inside the sandbox.
    command: Vec<String>,
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

/// Expand a leading `~` in a filesystem path using the `HOME` environment variable.
fn expand_tilde(path: &str) -> PathBuf {
    if path.starts_with("~")
        && let Ok(home) = env::var("HOME")
    {
        return PathBuf::from(path.replacen("~", &home, 1));
    }
    PathBuf::from(path)
}

/// If running from within a godo sandbox, returns the sandbox name.
/// Returns None if not in a sandbox.
fn current_sandbox_name(godo_dir: &Path) -> Result<Option<String>> {
    let current_dir = env::current_dir()?;
    let canonical_godo = godo_dir
        .canonicalize()
        .unwrap_or_else(|_| godo_dir.to_path_buf());

    let canonical_cwd = match current_dir.canonicalize() {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };

    let relative = match canonical_cwd.strip_prefix(&canonical_godo) {
        Ok(rel) => rel,
        Err(_) => return Ok(None), // Not in godo dir
    };

    let components: Vec<_> = relative.components().collect();
    if components.len() < 2 {
        return Ok(None); // In godo dir but not in a sandbox
    }

    // components[0] = project, components[1] = sandbox
    let sandbox_name = components[1].as_os_str().to_string_lossy().to_string();

    // Filter out meta-directories
    if sandbox_name.starts_with(".godo-") {
        return Ok(None);
    }

    Ok(Some(sandbox_name))
}

/// Convert output-layer failures into domain errors.
fn map_output_error(err: OutputError) -> GodoError {
    match err {
        OutputError::Cancelled => GodoError::UserAborted,
        other => GodoError::OperationError(format!("Output operation failed: {other}")),
    }
}

/// Emit an output result, mapping errors into `GodoError`.
fn emit(result: StdResult<(), OutputError>) -> Result<()> {
    result.map_err(map_output_error)?;
    Ok(())
}

/// Prompt for confirmation, mapping cancellation to `UserAborted`.
fn prompt_confirm(output: &dyn Output, prompt: &str) -> Result<bool> {
    match output.confirm(prompt) {
        Ok(value) => Ok(value),
        Err(OutputError::Cancelled) => Err(GodoError::UserAborted.into()),
        Err(err) => {
            Err(GodoError::OperationError(format!("Output operation failed: {err}")).into())
        }
    }
}

/// Prompt for selection, returning `None` on cancellation.
fn prompt_select_optional(
    output: &dyn Output,
    prompt: &str,
    options: Vec<String>,
) -> Result<Option<usize>> {
    match output.select(prompt, options) {
        Ok(selection) => Ok(Some(selection)),
        Err(OutputError::Cancelled) => Ok(None),
        Err(err) => {
            Err(GodoError::OperationError(format!("Output operation failed: {err}")).into())
        }
    }
}

/// Prompt for selection, mapping cancellation to `UserAborted`.
fn prompt_select(output: &dyn Output, prompt: &str, options: Vec<String>) -> Result<usize> {
    prompt_select_optional(output, prompt, options)?.ok_or_else(|| GodoError::UserAborted.into())
}

/// Prompt for the next action after a sandboxed command finishes.
fn prompt_for_action(
    godo: &Godo,
    output: &dyn Output,
    sandbox_name: &str,
) -> Result<PostRunAction> {
    let status = godo
        .sandbox_status(sandbox_name)?
        .ok_or_else(|| GodoError::SandboxError {
            name: sandbox_name.to_string(),
            message: "does not exist".to_string(),
        })?;

    let has_uncommitted = status.has_uncommitted_changes;
    let has_unmerged = matches!(status.merge_status, MergeStatus::Diverged);

    let prompt = match (has_uncommitted, has_unmerged) {
        (true, true) => "Uncommitted changes and unmerged commits. What next?",
        (true, false) => "Uncommitted changes. What next?",
        (false, true) => "Unmerged commits. What next?",
        (false, false) => "What next?",
    };

    let mut options = Vec::new();
    let mut actions = Vec::new();

    if has_uncommitted {
        options.push("Commit all changes".to_string());
        actions.push(PostRunAction::Commit);
    }

    options.push("Drop to shell".to_string());
    actions.push(PostRunAction::Shell);

    options.push("Keep sandbox".to_string());
    actions.push(PostRunAction::Keep);

    options.push("Discard everything".to_string());
    actions.push(PostRunAction::Discard);

    if has_unmerged {
        options.push("Keep branch only".to_string());
        actions.push(PostRunAction::Branch);
    }

    let selection = prompt_select_optional(output, prompt, options)?;
    Ok(match selection {
        Some(idx) => actions[idx],
        None => PostRunAction::Shell,
    })
}

/// Run a command (or shell) inside the sandbox and propagate its exit code.
fn run_command_in_sandbox(
    sandbox_path: &Path,
    command: &[String],
    force_shell: bool,
) -> Result<()> {
    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());

    let status = if command.is_empty() {
        Command::new(&shell)
            .current_dir(sandbox_path)
            .status()
            .map_err(|e| GodoError::OperationError(format!("Failed to start shell: {e}")))?
    } else if force_shell {
        let command_string = command.join(" ");
        Command::new(&shell)
            .arg("-c")
            .arg(&command_string)
            .current_dir(sandbox_path)
            .status()
            .map_err(|e| GodoError::OperationError(format!("Failed to run command: {e}")))?
    } else {
        let program = &command[0];
        let args = &command[1..];
        match Command::new(program)
            .args(args)
            .current_dir(sandbox_path)
            .status()
        {
            Ok(status) => status,
            Err(err) => {
                if err.kind() == io::ErrorKind::NotFound {
                    return Err(GodoError::CommandExit { code: 127 }.into());
                }
                return Err(
                    GodoError::OperationError(format!("Failed to run command: {err}")).into(),
                );
            }
        }
    };

    if !status.success() {
        let exit_code = status.code().unwrap_or(1);
        return Err(GodoError::CommandExit { code: exit_code }.into());
    }

    Ok(())
}

/// Run an interactive `git commit --verbose` after staging all changes.
fn run_interactive_commit(sandbox_path: &Path) -> Result<()> {
    let status = Command::new("git")
        .current_dir(sandbox_path)
        .args(["add", "."])
        .status()
        .map_err(|e| GodoError::GitError(format!("Failed to run git add: {e}")))?;

    if !status.success() {
        return Err(GodoError::GitError("Git add failed".to_string()).into());
    }

    let status = Command::new("git")
        .current_dir(sandbox_path)
        .args(["commit", "--verbose"])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| GodoError::GitError(format!("Failed to run git commit: {e}")))?;

    if !status.success() {
        return Err(GodoError::GitError("Git commit failed".to_string()).into());
    }

    Ok(())
}

/// Run a git diff command, treating exit codes 0 and 1 as success.
fn run_git_diff_command(sandbox_path: &Path, pager: &DiffPager, args: &[String]) -> Result<()> {
    let mut command = Command::new("git");
    command.current_dir(sandbox_path);
    pager.apply(&mut command);
    command.args(args);
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());

    let status = command
        .status()
        .map_err(|e| GodoError::GitError(format!("Failed to run git {}: {e}", args.join(" "))))?;

    match status.code() {
        Some(0) | Some(1) => Ok(()),
        Some(code) => {
            Err(GodoError::GitError(format!("Git diff failed with exit code {code}")).into())
        }
        None => Err(GodoError::GitError("Git diff terminated by signal".to_string()).into()),
    }
}

/// Render a sandbox entry in list output.
fn render_sandbox_entry(output: &dyn Output, entry: SandboxListEntry) -> Result<()> {
    let status = entry.status;
    let connections = entry.active_connections;

    let has_unmerged = status.has_branch && matches!(status.merge_status, MergeStatus::Diverged);
    let has_uncommitted = status.has_worktree && status.has_uncommitted_changes;

    let section = output.section(&status.name);

    if let Some(branch) = &status.worktree_branch {
        emit(section.item("branch", branch))?;
    } else if status.worktree_detached {
        emit(section.item("branch", "(detached HEAD)"))?;
    } else if status.has_branch {
        emit(section.item("branch", &format!("godo/{}", status.name)))?;
    }

    if connections > 0 {
        let label = if connections == 1 {
            "1 active connection".to_string()
        } else {
            format!("{connections} active connections")
        };
        emit(section.message(&label))?;
    }

    if has_unmerged {
        for commit in status.unmerged_commits {
            emit(section.commit(
                &commit.short_hash,
                &commit.subject,
                commit.insertions,
                commit.deletions,
            ))?;
        }
    }

    if has_uncommitted {
        if let Some(stats) = status.diff_stats {
            emit(section.diff_stat("uncommitted changes", stats.insertions, stats.deletions))?;
        } else {
            emit(section.warn("uncommitted changes"))?;
        }
    }

    if status.is_dangling {
        emit(section.fail("dangling worktree"))?;
    }

    Ok(())
}

/// Render the cleanup report for a sandbox.
fn render_cleanup_report(output: &dyn Output, report: CleanupReport) -> Result<()> {
    let status = report.status;
    let section = output.section(&format!("cleaning sandbox: {}", status.name));
    let branch = format!("godo/{}", status.name);

    if status.has_worktree && !status.has_uncommitted_changes && report.worktree_removed {
        emit(section.message("removed unmodified worktree"))?;
    } else if status.has_worktree && status.has_uncommitted_changes {
        emit(section.message("skipping worktree with uncommitted changes"))?;
    }

    if report.worktree_removed && report.branch_removed {
        emit(section.success("unmodified sandbox and branch cleaned up"))?;
    } else if report.worktree_removed && !report.branch_removed {
        emit(section.success(&format!("worktree removed, branch {branch} kept")))?;
    } else if !status.has_worktree && report.branch_removed {
        emit(section.success(&format!("fully merged branch {branch} removed")))?;
    } else if status.has_worktree && status.has_uncommitted_changes {
        emit(section.warn("not cleaned: has uncommitted changes"))?;
    } else if status.has_branch && matches!(status.merge_status, MergeStatus::Diverged) {
        emit(section.warn(&format!("branch {branch} has unmerged commits")))?;
    } else if status.has_branch && matches!(status.merge_status, MergeStatus::Unknown) {
        emit(section.warn(&format!(
            "branch {branch} kept because merge status could not be determined"
        )))?;
    } else {
        emit(section.message("unchanged"))?;
    }

    Ok(())
}

/// Handle removal with spinner feedback.
fn remove_with_spinner(
    godo: &Godo,
    output: &dyn Output,
    plan: &RemovalPlan,
    options: &RemovalOptions,
) -> Result<RemovalOutcome> {
    let spinner = output.spinner("Removing sandbox...");
    let result = godo.remove(plan, options);

    match result {
        Ok(RemovalOutcome::Removed) => {
            spinner.finish_success("Sandbox removed");
            Ok(RemovalOutcome::Removed)
        }
        Ok(RemovalOutcome::Blocked(blockers)) => {
            spinner.finish_fail("Failed to remove sandbox");
            Ok(RemovalOutcome::Blocked(blockers))
        }
        Err(err) => {
            spinner.finish_fail("Failed to remove sandbox");
            Err(err.into())
        }
    }
}

/// Compute removal options based on confirmation responses.
fn removal_options_from_confirmations(
    plan: &RemovalPlan,
    output: &dyn Output,
    no_prompt: bool,
) -> Result<RemovalOptions> {
    let mut options = RemovalOptions {
        allow_uncommitted_changes: false,
        allow_unmerged_commits: false,
        allow_unknown_merge_status: false,
    };

    if plan.blockers.contains(&RemovalBlocker::UncommittedChanges) {
        if no_prompt {
            return Err(GodoError::SandboxError {
                name: plan.status.name.clone(),
                message: "has uncommitted changes (use --force to remove)".to_string(),
            }
            .into());
        }
        if !prompt_confirm(output, "Uncommitted changes will be lost. Continue?")? {
            return Err(GodoError::UserAborted.into());
        }
        options.allow_uncommitted_changes = true;
    }

    if plan.blockers.contains(&RemovalBlocker::UnmergedCommits) {
        if no_prompt {
            return Err(GodoError::SandboxError {
                name: plan.status.name.clone(),
                message: "branch has unmerged commits (use --force to remove)".to_string(),
            }
            .into());
        }
        if !prompt_confirm(output, "Unmerged commits will be lost. Continue?")? {
            return Err(GodoError::UserAborted.into());
        }
        options.allow_unmerged_commits = true;
    }

    if plan.blockers.contains(&RemovalBlocker::MergeStatusUnknown) {
        if no_prompt {
            return Err(GodoError::SandboxError {
                name: plan.status.name.clone(),
                message: "branch merge status is unknown (use --force to remove)".to_string(),
            }
            .into());
        }
        if !prompt_confirm(
            output,
            "Merge status unknown (commits may be lost). Continue?",
        )? {
            return Err(GodoError::UserAborted.into());
        }
        options.allow_unknown_merge_status = true;
    }

    Ok(options)
}

/// Run the `godo run` command logic.
fn run_command(
    godo: &Godo,
    output: &dyn Output,
    no_prompt: bool,
    request: RunRequest,
) -> Result<()> {
    let RunRequest {
        keep,
        commit,
        force_shell,
        excludes,
        sandbox_name,
        command,
    } = request;
    let existing = godo.sandbox_status(&sandbox_name)?;
    let sandbox_path = godo.sandbox_path(&sandbox_name)?;

    if let Some(status) = existing.as_ref() {
        if status.is_live() {
            emit(output.message(&format!(
                "Using existing sandbox {sandbox_name} at {sandbox_path:?}"
            )))?;
        } else {
            let status = status.component_status();
            return Err(GodoError::SandboxError {
                name: sandbox_name,
                message: format!("exists but is not live - remove it first ({status})"),
            }
            .into());
        }
    }

    let uncommitted_policy = if existing.is_none() {
        let has_uncommitted = godo.repo_has_uncommitted_changes()?;
        let mut policy = UncommittedPolicy::Include;

        if has_uncommitted {
            emit(output.warn("You have uncommitted changes."))?;
            if !no_prompt {
                let options = vec![
                    "Abort".to_string(),
                    "Include uncommitted changes".to_string(),
                    "Start clean (HEAD only)".to_string(),
                ];
                match prompt_select(output, "Uncommitted changes in working tree", options)? {
                    0 => return Err(GodoError::UserAborted.into()),
                    1 => {}
                    2 => policy = UncommittedPolicy::Clean,
                    _ => unreachable!("Invalid selection"),
                }
            }
        }

        policy
    } else {
        UncommittedPolicy::Include
    };

    let prepare_options = PrepareSandboxOptions {
        uncommitted_policy,
        excludes,
    };

    let plan = if existing.is_none() {
        let branch = format!("godo/{sandbox_name}");
        emit(output.message(&format!(
            "Creating sandbox {sandbox_name} with branch {branch} at {sandbox_path:?}"
        )))?;

        let spinner = output.spinner("Cloning tree to sandbox...");
        match godo.prepare_sandbox(&sandbox_name, prepare_options) {
            Ok(plan) => {
                spinner.finish_success("Sandbox ready");
                plan
            }
            Err(err) => {
                spinner.finish_fail("Clone failed");
                return Err(err.into());
            }
        }
    } else {
        godo.prepare_sandbox(&sandbox_name, prepare_options)?
    };

    if plan.cleaned {
        emit(output.message("Resetting sandbox to clean state..."))?;
        emit(output.success("Sandbox is now in a clean state"))?;
    }

    let sandbox_path = plan.session.path.clone();
    run_command_in_sandbox(&sandbox_path, &command, force_shell)?;

    let _cleanup_guard = match plan.session.release()? {
        ReleaseOutcome::NotLast => {
            emit(output.message("Another godo session is still attached; skipping cleanup."))?;
            return Ok(());
        }
        ReleaseOutcome::Last(guard) => guard,
    };

    if !keep && commit.is_none() {
        let removal_plan = godo.removal_plan(&sandbox_name)?;
        if removal_plan.blockers.is_empty() {
            let options = RemovalOptions {
                allow_uncommitted_changes: false,
                allow_unmerged_commits: false,
                allow_unknown_merge_status: false,
            };
            let outcome = remove_with_spinner(godo, output, &removal_plan, &options)?;
            if matches!(outcome, RemovalOutcome::Removed) {
                return Ok(());
            }
        }
    }

    if let Some(commit_message) = commit {
        emit(output.message("Staging and committing changes..."))?;
        godo.commit_all(&sandbox_name, &commit_message)?;
        emit(output.success(&format!("Committed with message: {commit_message}")))?;
        let batch = godo.clean(Some(&sandbox_name))?;
        return render_cleanup_batch(output, batch, Some(&sandbox_name));
    }

    if keep {
        return Ok(());
    }

    if no_prompt {
        emit(output.success(&format!(
            "Keeping sandbox. You can return to it at: {}",
            sandbox_path.display()
        )))?;
        return Ok(());
    }

    loop {
        match prompt_for_action(godo, output, &sandbox_name)? {
            PostRunAction::Commit => {
                emit(output.message("Staging and committing changes..."))?;
                run_interactive_commit(&sandbox_path)?;
                let batch = godo.clean(Some(&sandbox_name))?;
                return render_cleanup_batch(output, batch, Some(&sandbox_name));
            }
            PostRunAction::Shell => {
                emit(output.message("Opening shell in sandbox..."))?;
                let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
                let status = Command::new(&shell)
                    .current_dir(&sandbox_path)
                    .status()
                    .map_err(|e| {
                        GodoError::OperationError(format!("Failed to start shell: {e}"))
                    })?;
                if !status.success() {
                    emit(output.warn("Shell exited with non-zero status"))?;
                }
            }
            PostRunAction::Keep => {
                emit(output.success(&format!(
                    "Keeping sandbox. You can return to it at: {}",
                    sandbox_path.display()
                )))?;
                return Ok(());
            }
            PostRunAction::Discard => {
                if !prompt_confirm(output, "Discard all changes and delete branch?")? {
                    continue;
                }
                let removal_plan = godo.removal_plan(&sandbox_name)?;
                let outcome =
                    remove_with_spinner(godo, output, &removal_plan, &RemovalOptions::force())?;
                if matches!(outcome, RemovalOutcome::Blocked(_)) {
                    return Err(GodoError::SandboxError {
                        name: sandbox_name,
                        message: "remove blocked".to_string(),
                    }
                    .into());
                }
                return Ok(());
            }
            PostRunAction::Branch => {
                emit(output.message("Keeping branch but removing worktree..."))?;
                godo.remove_worktree_keep_branch(&sandbox_name)?;
                emit(output.success(&format!(
                    "Worktree removed, branch godo/{sandbox_name} kept"
                )))?;
                return Ok(());
            }
        }
    }
}

/// Render cleanup batch results and surface failures.
fn render_cleanup_batch(
    output: &dyn Output,
    batch: CleanupBatch,
    single_name: Option<&str>,
) -> Result<()> {
    let CleanupBatch { reports, failures } = batch;
    if let Some(failure) = failures.into_iter().next() {
        return Err(failure.error.into());
    }

    let report_count = reports.len();
    for report in reports {
        render_cleanup_report(output, report)?;
    }

    if let Some(name) = single_name
        && report_count == 0
    {
        return Err(GodoError::SandboxError {
            name: name.to_string(),
            message: "does not exist".to_string(),
        }
        .into());
    }

    Ok(())
}

/// Run the `godo list` command logic.
fn list_command(godo: &Godo, output: &dyn Output) -> Result<()> {
    let entries = godo.list()?;
    if entries.is_empty() {
        emit(output.message("No sandboxes found."))?;
        return Ok(());
    }

    for entry in entries {
        render_sandbox_entry(output, entry)?;
    }

    Ok(())
}

/// Run the `godo diff` command logic.
fn diff_command(
    godo: &Godo,
    output: &dyn Output,
    name: Option<&str>,
    base: Option<&str>,
    pager: Option<String>,
    no_pager: bool,
    current_sandbox: Option<&str>,
) -> Result<()> {
    let effective_name = match (name, current_sandbox) {
        (Some(name), _) => name,
        (None, Some(name)) => name,
        (None, None) => {
            return Err(GodoError::OperationError(
                "No sandbox name provided and not inside a sandbox".to_string(),
            )
            .into());
        }
    };

    let plan = godo.diff_plan(effective_name, base)?;

    if plan.used_fallback {
        if let Some(target) = &plan.fallback_target {
            emit(output.warn(&format!(
                "Recorded base commit missing; using merge-base with {target}"
            )))?;
        } else {
            emit(output.warn("Recorded base commit missing; using merge-base fallback"))?;
        }
    }

    run_diff_plan(&plan, pager, no_pager)?;
    Ok(())
}

/// Execute a diff plan with the provided pager options.
fn run_diff_plan(plan: &DiffPlan, pager: Option<String>, no_pager: bool) -> Result<()> {
    let pager = DiffPager::new(pager, no_pager);
    let tracked_args = vec!["diff".to_string(), plan.base_commit.clone()];
    run_git_diff_command(&plan.sandbox_path, &pager, &tracked_args)?;

    for path in &plan.untracked_files {
        let diff_args = vec![
            "diff".to_string(),
            "--no-index".to_string(),
            "--".to_string(),
            "/dev/null".to_string(),
            path.to_string_lossy().to_string(),
        ];
        run_git_diff_command(&plan.sandbox_path, &pager, &diff_args)?;
    }

    Ok(())
}

/// Run the `godo remove` command logic.
fn remove_command(
    godo: &Godo,
    output: &dyn Output,
    name: String,
    force: bool,
    no_prompt: bool,
) -> Result<()> {
    let plan = godo.removal_plan(&name)?;

    let options = if force {
        RemovalOptions::force()
    } else {
        removal_options_from_confirmations(&plan, output, no_prompt)?
    };

    match remove_with_spinner(godo, output, &plan, &options)? {
        RemovalOutcome::Removed => Ok(()),
        RemovalOutcome::Blocked(blockers) => Err(GodoError::SandboxError {
            name,
            message: format!("removal blocked by: {blockers:?}"),
        }
        .into()),
    }
}

/// Run the `godo clean` command logic.
fn clean_command(
    godo: &Godo,
    output: &dyn Output,
    name: Option<&str>,
    no_prompt: bool,
) -> Result<()> {
    if let Some(name) = name {
        if let Some(status) = godo.sandbox_status(name)? {
            if status.has_worktree
                && status.has_uncommitted_changes
                && !no_prompt
                && !prompt_confirm(output, "Uncommitted changes will be lost. Continue?")?
            {
                return Err(GodoError::UserAborted.into());
            }
        } else {
            return Err(GodoError::SandboxError {
                name: name.to_string(),
                message: "does not exist".to_string(),
            }
            .into());
        }
    }

    let batch = godo.clean(name)?;

    if name.is_none() {
        let total = batch.reports.len() + batch.failures.len();
        if total == 0 {
            emit(output.message("No sandboxes to clean"))?;
            return Ok(());
        }

        emit(output.message(&format!("Cleaning {total} sandboxes...")))?;
    }

    for report in batch.reports {
        render_cleanup_report(output, report)?;
    }

    for failure in batch.failures {
        emit(output.warn(&format!(
            "Failed to clean {}: {}",
            failure.sandbox_name, failure.error
        )))?;
        if name.is_some() {
            return Err(failure.error.into());
        }
    }

    Ok(())
}

/// CLI entrypoint.
fn main() -> Result<()> {
    let cli = Cli::parse();

    // Determine color output preference early for error handling
    let color = if cli.color {
        true
    } else if cli.no_color {
        false
    } else {
        // Auto-detect based on terminal
        io::stdout().is_terminal()
    };

    // Create output handler for potential error messages
    let output: Arc<dyn Output> = if cli.quiet {
        Arc::new(Quiet)
    } else {
        Arc::new(Terminal::new(color))
    };

    // Handle errors with custom formatting
    if let Err(e) = run(cli, &output) {
        // Reset any existing colors only if color was enabled and stdout is a TTY
        if color && io::stdout().is_terminal() {
            print!("\x1b[0m");
            if let Err(flush_err) = io::stdout().flush() {
                eprintln!("Failed to flush stdout while resetting colors: {flush_err}");
            }
        }

        let exit_code = match e.downcast_ref::<GodoError>() {
            Some(err @ GodoError::CommandExit { .. }) => err.exit_code(),
            Some(err @ GodoError::UserAborted) => {
                if let Err(finish_err) = output.finish() {
                    eprintln!("Failed to flush output handler: {finish_err:#}");
                }
                err.exit_code()
            }
            Some(err) => {
                // Use the output handler to display the error
                if let Err(display_err) = output.fail(&format!("{e:#}")) {
                    eprintln!("Failed to report error via output handler: {display_err:#}");
                }
                if let Err(finish_err) = output.finish() {
                    eprintln!("Failed to flush output handler: {finish_err:#}");
                }
                err.exit_code()
            }
            None => {
                // Use the output handler to display the error
                if let Err(display_err) = output.fail(&format!("{e:#}")) {
                    eprintln!("Failed to report error via output handler: {display_err:#}");
                }
                if let Err(finish_err) = output.finish() {
                    eprintln!("Failed to flush output handler: {finish_err:#}");
                }
                1
            }
        };

        process::exit(exit_code);
    }
    Ok(())
}

/// Execute the selected CLI command using the provided output implementation.
fn run(cli: Cli, output: &Arc<dyn Output>) -> Result<()> {
    // Determine godo directory (priority: CLI flag > env var > default)
    let godo_dir = if let Some(dir) = &cli.dir {
        expand_tilde(dir)
    } else if let Ok(env_dir) = env::var("GODO_DIR") {
        expand_tilde(&env_dir)
    } else {
        expand_tilde(DEFAULT_GODO_DIR)
    };

    // Detect if we're running from within a sandbox
    let current_sandbox = current_sandbox_name(&godo_dir)?;

    // Per-command sandbox context checks
    match &cli.command {
        Commands::List => {}
        Commands::Diff { .. } => {}
        Commands::Run { name, .. } => {
            if let Some(ref current) = current_sandbox
                && current == name
            {
                anyhow::bail!(
                    "Cannot run sandbox '{}' from within itself. Exit the sandbox first.",
                    name
                );
            }
        }
        Commands::Remove { name, .. } => {
            if let Some(ref current) = current_sandbox
                && current == name
            {
                anyhow::bail!(
                    "Cannot remove sandbox '{}' while inside it. Exit the sandbox first.",
                    name
                );
            }
        }
        Commands::Clean { name } => {
            if let Some(ref current) = current_sandbox {
                if name.is_none() {
                    anyhow::bail!(
                        "Cannot run 'godo clean' (all sandboxes) from within a sandbox. \
                         Exit the sandbox first, or specify a sandbox name."
                    );
                }
                if name.as_ref() == Some(current) {
                    anyhow::bail!(
                        "Cannot clean sandbox '{}' while inside it. Exit the sandbox first.",
                        current
                    );
                }
            }
        }
    }

    // Determine repository directory
    let repo_dir = cli.repo_dir.as_ref().map(|repo| expand_tilde(repo));

    // Create Godo instance
    let godo = Godo::new(godo_dir, repo_dir).context("Failed to initialize godo")?;

    match cli.command {
        Commands::Run {
            keep,
            commit,
            sh,
            excludes,
            name,
            command,
        } => {
            run_command(
                &godo,
                output.as_ref(),
                cli.no_prompt,
                RunRequest {
                    keep,
                    commit,
                    force_shell: sh,
                    excludes,
                    sandbox_name: name,
                    command,
                },
            )?;
        }
        Commands::List => {
            list_command(&godo, output.as_ref())?;
        }
        Commands::Diff {
            name,
            base,
            pager,
            no_pager,
        } => {
            diff_command(
                &godo,
                output.as_ref(),
                name.as_deref(),
                base.as_deref(),
                pager,
                no_pager,
                current_sandbox.as_deref(),
            )?;
        }
        Commands::Remove { name, force } => {
            remove_command(&godo, output.as_ref(), name, force, cli.no_prompt)?;
        }
        Commands::Clean { name } => {
            clean_command(&godo, output.as_ref(), name.as_deref(), cli.no_prompt)?;
        }
    }

    output.finish()?;
    Ok(())
}
