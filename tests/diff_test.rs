mod common;

use anyhow::Result;
use common::{create_repo, git, run_godo};
use std::{fs, path::PathBuf};
use tempfile::TempDir;

fn sandbox_path(godo_dir: &TempDir, repo_path: &PathBuf, name: &str) -> PathBuf {
    let project = repo_path
        .file_name()
        .expect("repo path should have a file name")
        .to_string_lossy()
        .to_string();
    godo_dir.path().join(project).join(name)
}

#[test]
fn test_diff_includes_tracked_and_untracked_changes() -> Result<()> {
    let (_tmp, repo_path) = create_repo("diff-project")?;
    let godo_dir = TempDir::new()?;

    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &[
            "run",
            "--keep",
            "diff-sandbox",
            "true",
        ],
    )?;
    assert!(output.status.success());

    let sandbox_path = sandbox_path(&godo_dir, &repo_path, "diff-sandbox");

    fs::write(sandbox_path.join("README.md"), "unstaged change\n")?;

    fs::write(sandbox_path.join("staged.txt"), "staged\n")?;
    git(&sandbox_path, &["add", "staged.txt"])?;

    fs::write(sandbox_path.join("committed.txt"), "committed\n")?;
    git(&sandbox_path, &["add", "committed.txt"])?;
    git(&sandbox_path, &["commit", "-m", "Commit in sandbox"])?;

    fs::write(sandbox_path.join("untracked.txt"), "untracked\n")?;

    let diff_output = run_godo(
        &repo_path,
        godo_dir.path(),
        &["diff", "--no-pager", "diff-sandbox"],
    )?;
    assert!(diff_output.status.success());

    let stdout = String::from_utf8_lossy(&diff_output.stdout);
    assert!(stdout.contains("README.md"));
    assert!(stdout.contains("staged.txt"));
    assert!(stdout.contains("committed.txt"));
    assert!(stdout.contains("untracked.txt"));

    let _ = run_godo(
        &repo_path,
        godo_dir.path(),
        &["remove", "--force", "diff-sandbox"],
    );

    Ok(())
}

#[test]
fn test_diff_base_override_includes_older_changes() -> Result<()> {
    let (_tmp, repo_path) = create_repo("diff-base")?;
    let godo_dir = TempDir::new()?;

    fs::write(repo_path.join("base.txt"), "base change\n")?;
    git(&repo_path, &["add", "base.txt"])?;
    git(&repo_path, &["commit", "-m", "Add base.txt"])?;

    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &[
            "run",
            "--keep",
            "base-sandbox",
            "true",
        ],
    )?;
    assert!(output.status.success());

    let diff_output = run_godo(
        &repo_path,
        godo_dir.path(),
        &["diff", "--no-pager", "--base", "HEAD~1", "base-sandbox"],
    )?;
    assert!(diff_output.status.success());

    let stdout = String::from_utf8_lossy(&diff_output.stdout);
    assert!(stdout.contains("base.txt"));

    let _ = run_godo(
        &repo_path,
        godo_dir.path(),
        &["remove", "--force", "base-sandbox"],
    );

    Ok(())
}
