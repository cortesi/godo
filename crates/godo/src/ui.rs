use anyhow::Result;
use godo_term::{Output, OutputError};
use libgodo::{CleanupBatch, CleanupReport, GodoError, MergeStatus};
use std::result::Result as StdResult;

/// Convert output-layer failures into domain errors.
pub fn map_output_error(err: OutputError) -> GodoError {
    match err {
        OutputError::Cancelled => GodoError::UserAborted,
        other => GodoError::OperationError(format!("Output operation failed: {other}")),
    }
}

/// Emit an output result, mapping errors into `GodoError`.
pub fn emit(result: StdResult<(), OutputError>) -> Result<()> {
    result.map_err(map_output_error)?;
    Ok(())
}

/// Prompt for confirmation, mapping cancellation to `UserAborted`.
pub fn prompt_confirm(output: &dyn Output, prompt: &str) -> Result<bool> {
    match output.confirm(prompt) {
        Ok(value) => Ok(value),
        Err(OutputError::Cancelled) => Err(GodoError::UserAborted.into()),
        Err(err) => {
            Err(GodoError::OperationError(format!("Output operation failed: {err}")).into())
        }
    }
}

/// Prompt for selection, returning `None` on cancellation.
pub fn prompt_select_optional(
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
pub fn prompt_select(output: &dyn Output, prompt: &str, options: Vec<String>) -> Result<usize> {
    prompt_select_optional(output, prompt, options)?.ok_or_else(|| GodoError::UserAborted.into())
}

/// Render the cleanup report for a sandbox.
pub fn render_cleanup_report(output: &dyn Output, report: CleanupReport) -> Result<()> {
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

/// Render cleanup batch results and surface failures.
pub fn render_cleanup_batch(
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
