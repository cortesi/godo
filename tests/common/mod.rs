use anyhow::{ensure, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

/// Return the path to the compiled `godo` binary for integration-style tests.
pub fn godo_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_godo"))
}

/// Run a git command inside `repo_path`, ensuring it succeeds.
pub fn git(repo_path: &Path, args: &[&str]) -> Result<Output> {
    let output = Command::new("git")
        .current_dir(repo_path)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;

    ensure!(
        output.status.success(),
        "git command failed: git {}\nstdout: {}\nstderr: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(output)
}

/// Initialise a new repository at `repo_path` with a README commit.
pub fn init_repository(repo_path: &Path) -> Result<()> {
    if !repo_path.exists() {
        fs::create_dir_all(repo_path)?;
    }

    git(repo_path, &["init"])?;
    git(repo_path, &["config", "user.email", "test@example.com"])?;
    git(repo_path, &["config", "user.name", "Test User"])?;

    fs::write(repo_path.join("README.md"), "# Test Project")?;
    git(repo_path, &["add", "README.md"])?;
    git(repo_path, &["commit", "-m", "Initial commit"])?;

    Ok(())
}

/// Create a temporary repository with the provided name relative to the temp dir.
pub fn create_repo(repo_name: &str) -> Result<(TempDir, PathBuf)> {
    let temp_dir = TempDir::new()?;
    let repo_path = temp_dir.path().join(repo_name);
    init_repository(&repo_path)?;
    Ok((temp_dir, repo_path))
}

/// Prepare a `Command` configured to run `godo` against the provided project.
pub fn godo_command(repo_path: &Path, godo_dir: &Path) -> Command {
    let mut cmd = Command::new(godo_binary());
    cmd.current_dir(repo_path);
    cmd.arg("--dir");
    cmd.arg(godo_dir);
    cmd
}

/// Run `godo` with the provided arguments, returning the command output.
pub fn run_godo(repo_path: &Path, godo_dir: &Path, args: &[&str]) -> Result<Output> {
    let mut cmd = godo_command(repo_path, godo_dir);
    cmd.args(args);
    Ok(cmd
        .output()
        .with_context(|| format!("failed to run godo {}", args.join(" ")))?
    )
}
