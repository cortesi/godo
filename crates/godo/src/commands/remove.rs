use anyhow::Result;
use godo_term::Output;
use libgodo::{Godo, GodoError, RemovalBlocker, RemovalOptions, RemovalOutcome, RemovalPlan};

use crate::ui::prompt_confirm;

/// Run the `godo remove` command logic.
pub fn remove(
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

/// Handle removal with spinner feedback.
pub fn remove_with_spinner(
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
