use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Command, Output};

/// Run a git command with the given arguments in the specified directory.
/// Returns the output if successful, otherwise returns an error with the full command details.
fn run_git(repo_path: &Path, args: &[&str]) -> Result<Output> {
    let output = Command::new("git")
        .current_dir(repo_path)
        .args(args)
        .output()
        .with_context(|| format!("Failed to execute git command: git {}", args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let command = format!("git {}", args.join(" "));
        anyhow::bail!("Git command failed: {}\nError: {}", command, stderr.trim());
    }

    Ok(output)
}

pub fn has_uncommitted_changes(repo_path: &Path) -> Result<bool> {
    let output = run_git(repo_path, &["status", "--porcelain"])?;
    let status_output = String::from_utf8_lossy(&output.stdout);
    Ok(!status_output.trim().is_empty())
}

pub fn has_branch(repo_path: &Path, branch_name: &str) -> Result<bool> {
    let output = run_git(repo_path, &["branch", "--list", branch_name])?;
    let branch_output = String::from_utf8_lossy(&output.stdout);
    Ok(!branch_output.trim().is_empty())
}

pub fn create_worktree(repo_path: &Path, worktree_path: &Path, branch_name: &str) -> Result<()> {
    if has_branch(repo_path, branch_name)? {
        anyhow::bail!("Branch '{}' already exists", branch_name);
    }

    let worktree_path_str = worktree_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid worktree path"))?;

    run_git(
        repo_path,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            branch_name,
            worktree_path_str,
        ],
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_repo() -> Result<(TempDir, std::path::PathBuf)> {
        let temp_dir = TempDir::new()?;
        let repo_path = temp_dir.path().to_path_buf();

        // Initialize git repository
        run_git(&repo_path, &["init"])?;

        // Configure git user for commits
        run_git(&repo_path, &["config", "user.email", "test@example.com"])?;
        run_git(&repo_path, &["config", "user.name", "Test User"])?;

        Ok((temp_dir, repo_path))
    }

    #[test]
    fn test_has_uncommitted_changes_clean_repo() -> Result<()> {
        let (_temp_dir, repo_path) = setup_test_repo()?;

        // Create and commit a file
        fs::write(repo_path.join("test.txt"), "initial content")?;
        run_git(&repo_path, &["add", "test.txt"])?;
        run_git(&repo_path, &["commit", "-m", "Initial commit"])?;

        // Should have no uncommitted changes
        assert!(!has_uncommitted_changes(&repo_path)?);

        Ok(())
    }

    #[test]
    fn test_has_uncommitted_changes_modified_file() -> Result<()> {
        let (_temp_dir, repo_path) = setup_test_repo()?;

        // Create and commit a file
        fs::write(repo_path.join("test.txt"), "initial content")?;
        run_git(&repo_path, &["add", "test.txt"])?;
        run_git(&repo_path, &["commit", "-m", "Initial commit"])?;

        // Modify the file
        fs::write(repo_path.join("test.txt"), "modified content")?;

        // Should detect uncommitted changes
        assert!(has_uncommitted_changes(&repo_path)?);

        Ok(())
    }

    #[test]
    fn test_has_uncommitted_changes_untracked_file() -> Result<()> {
        let (_temp_dir, repo_path) = setup_test_repo()?;

        // Create and commit a file
        fs::write(repo_path.join("test.txt"), "initial content")?;
        run_git(&repo_path, &["add", "test.txt"])?;
        run_git(&repo_path, &["commit", "-m", "Initial commit"])?;

        // Create a new untracked file
        fs::write(repo_path.join("untracked.txt"), "new file")?;

        // Should detect uncommitted changes (untracked files)
        assert!(has_uncommitted_changes(&repo_path)?);

        Ok(())
    }

    #[test]
    fn test_has_uncommitted_changes_staged_file() -> Result<()> {
        let (_temp_dir, repo_path) = setup_test_repo()?;

        // Create and commit a file
        fs::write(repo_path.join("test.txt"), "initial content")?;
        run_git(&repo_path, &["add", "test.txt"])?;
        run_git(&repo_path, &["commit", "-m", "Initial commit"])?;

        // Create a new file and stage it
        fs::write(repo_path.join("staged.txt"), "staged content")?;
        run_git(&repo_path, &["add", "staged.txt"])?;

        // Should detect uncommitted changes (staged files)
        assert!(has_uncommitted_changes(&repo_path)?);

        Ok(())
    }

    #[test]
    fn test_create_worktree() -> Result<()> {
        let (_temp_dir, repo_path) = setup_test_repo()?;

        // Create an initial commit
        fs::write(repo_path.join("README.md"), "# Test Repo")?;
        run_git(&repo_path, &["add", "README.md"])?;
        run_git(&repo_path, &["commit", "-m", "Initial commit"])?;

        // Create a worktree
        let worktree_path = repo_path.parent().unwrap().join("test-worktree");
        create_worktree(&repo_path, &worktree_path, "test-branch")?;

        // Verify worktree was created
        assert!(worktree_path.exists());
        assert!(worktree_path.join(".git").exists());
        assert!(worktree_path.join("README.md").exists());

        // Verify branch was created
        let branches = run_git(&repo_path, &["branch", "--list", "test-branch"])?;
        let branch_output = String::from_utf8_lossy(&branches.stdout);
        assert!(branch_output.contains("test-branch"));

        // Clean up worktree
        run_git(
            &repo_path,
            &["worktree", "remove", worktree_path.to_str().unwrap()],
        )?;

        Ok(())
    }

    #[test]
    fn test_has_branch() -> Result<()> {
        let (_temp_dir, repo_path) = setup_test_repo()?;

        // Create an initial commit
        fs::write(repo_path.join("README.md"), "# Test Repo")?;
        run_git(&repo_path, &["add", "README.md"])?;
        run_git(&repo_path, &["commit", "-m", "Initial commit"])?;

        // Main branch should exist
        assert!(has_branch(&repo_path, "main")?);

        // Non-existent branch should not exist
        assert!(!has_branch(&repo_path, "non-existent-branch")?);

        // Create a new branch
        run_git(&repo_path, &["branch", "test-branch"])?;
        assert!(has_branch(&repo_path, "test-branch")?);

        Ok(())
    }

    #[test]
    fn test_create_worktree_duplicate_branch() -> Result<()> {
        let (temp_dir, repo_path) = setup_test_repo()?;

        // Create an initial commit
        fs::write(repo_path.join("README.md"), "# Test Repo")?;
        run_git(&repo_path, &["add", "README.md"])?;
        run_git(&repo_path, &["commit", "-m", "Initial commit"])?;

        // Create first worktree using temp_dir as the base
        let worktree_path1 = temp_dir.path().join("test-worktree-1");
        create_worktree(&repo_path, &worktree_path1, "duplicate-branch")?;

        // Try to create second worktree with same branch name but different path
        let worktree_path2 = temp_dir.path().join("test-worktree-2");
        let result = create_worktree(&repo_path, &worktree_path2, "duplicate-branch");

        // Should fail because branch already exists
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(
            error_msg.contains("Branch 'duplicate-branch' already exists"),
            "Expected error about branch already existing, got: {error_msg}",
        );

        // Clean up
        run_git(
            &repo_path,
            &["worktree", "remove", worktree_path1.to_str().unwrap()],
        )?;

        Ok(())
    }
}
