use anyhow::Result;
use godo_term::Output;
use libgodo::{DiffPlan, Godo, GodoError};
use std::process::{Command, Stdio};
use std::path::Path;

use crate::ui::emit;

/// Run the `godo diff` command logic.
pub fn diff(
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
