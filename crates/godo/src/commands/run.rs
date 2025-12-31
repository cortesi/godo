use anyhow::Result;
use godo_term::Output;
use libgodo::{
    Godo, GodoError, MergeStatus, PrepareSandboxOptions, ReleaseOutcome, RemovalOptions,
    RemovalOutcome, UncommittedPolicy,
};
use std::{
    env, io,
    path::Path,
    process::{Command, Stdio},
};

use crate::{
    args::RunRequest,
    commands::remove::remove_with_spinner,
    ui::{emit, prompt_confirm, prompt_select, prompt_select_optional, render_cleanup_batch},
};

/// Follow-up action to take after executing a sandboxed command.
#[derive(Clone, Copy)]
enum PostRunAction {
    /// Commit all changes.
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

/// Run the `godo run` command logic.
pub fn run(
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
