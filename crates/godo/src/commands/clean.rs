use anyhow::Result;
use godo_term::Output;
use libgodo::{Godo, GodoError};

use crate::ui::{emit, prompt_confirm, render_cleanup_report};

/// Run the `godo clean` command logic.
pub fn clean(
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
