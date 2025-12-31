use anyhow::Result;
use godo_term::Output;
use libgodo::{Godo, MergeStatus, SandboxListEntry};

use crate::ui::emit;

/// Run the `godo list` command logic.
pub fn list(godo: &Godo, output: &dyn Output) -> Result<()> {
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
