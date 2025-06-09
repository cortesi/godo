use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

pub fn has_uncommitted_changes(repo_path: &Path) -> Result<bool> {
    let status = Command::new("git")
        .current_dir(repo_path)
        .args(["status", "--porcelain"])
        .output()
        .context("Failed to check git status")?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        anyhow::bail!("Failed to check git status: {}", stderr);
    }

    let status_output = String::from_utf8_lossy(&status.stdout);
    Ok(!status_output.trim().is_empty())
}

pub fn create_worktree(repo_path: &Path, worktree_path: &Path, branch_name: &str) -> Result<()> {
    let output = Command::new("git")
        .current_dir(repo_path)
        .args(["worktree", "add", "--quiet", "-b", branch_name])
        .arg(worktree_path)
        .output()
        .context("Failed to create git worktree")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create worktree: {}", stderr);
    }

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
        Command::new("git")
            .current_dir(&repo_path)
            .args(["init"])
            .output()?;

        // Configure git user for commits
        Command::new("git")
            .current_dir(&repo_path)
            .args(["config", "user.email", "test@example.com"])
            .output()?;

        Command::new("git")
            .current_dir(&repo_path)
            .args(["config", "user.name", "Test User"])
            .output()?;

        Ok((temp_dir, repo_path))
    }

    #[test]
    fn test_has_uncommitted_changes_clean_repo() -> Result<()> {
        let (_temp_dir, repo_path) = setup_test_repo()?;

        // Create and commit a file
        fs::write(repo_path.join("test.txt"), "initial content")?;
        Command::new("git")
            .current_dir(&repo_path)
            .args(["add", "test.txt"])
            .output()?;
        Command::new("git")
            .current_dir(&repo_path)
            .args(["commit", "-m", "Initial commit"])
            .output()?;

        // Should have no uncommitted changes
        assert!(!has_uncommitted_changes(&repo_path)?);

        Ok(())
    }

    #[test]
    fn test_has_uncommitted_changes_modified_file() -> Result<()> {
        let (_temp_dir, repo_path) = setup_test_repo()?;

        // Create and commit a file
        fs::write(repo_path.join("test.txt"), "initial content")?;
        Command::new("git")
            .current_dir(&repo_path)
            .args(["add", "test.txt"])
            .output()?;
        Command::new("git")
            .current_dir(&repo_path)
            .args(["commit", "-m", "Initial commit"])
            .output()?;

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
        Command::new("git")
            .current_dir(&repo_path)
            .args(["add", "test.txt"])
            .output()?;
        Command::new("git")
            .current_dir(&repo_path)
            .args(["commit", "-m", "Initial commit"])
            .output()?;

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
        Command::new("git")
            .current_dir(&repo_path)
            .args(["add", "test.txt"])
            .output()?;
        Command::new("git")
            .current_dir(&repo_path)
            .args(["commit", "-m", "Initial commit"])
            .output()?;

        // Create a new file and stage it
        fs::write(repo_path.join("staged.txt"), "staged content")?;
        Command::new("git")
            .current_dir(&repo_path)
            .args(["add", "staged.txt"])
            .output()?;

        // Should detect uncommitted changes (staged files)
        assert!(has_uncommitted_changes(&repo_path)?);

        Ok(())
    }

    #[test]
    fn test_create_worktree() -> Result<()> {
        let (_temp_dir, repo_path) = setup_test_repo()?;

        // Create an initial commit
        fs::write(repo_path.join("README.md"), "# Test Repo")?;
        Command::new("git")
            .current_dir(&repo_path)
            .args(["add", "README.md"])
            .output()?;
        Command::new("git")
            .current_dir(&repo_path)
            .args(["commit", "-m", "Initial commit"])
            .output()?;

        // Create a worktree
        let worktree_path = repo_path.parent().unwrap().join("test-worktree");
        create_worktree(&repo_path, &worktree_path, "test-branch")?;

        // Verify worktree was created
        assert!(worktree_path.exists());
        assert!(worktree_path.join(".git").exists());
        assert!(worktree_path.join("README.md").exists());

        // Verify branch was created
        let branches = Command::new("git")
            .current_dir(&repo_path)
            .args(["branch", "--list", "test-branch"])
            .output()?;
        let branch_output = String::from_utf8_lossy(&branches.stdout);
        assert!(branch_output.contains("test-branch"));

        // Clean up worktree
        Command::new("git")
            .current_dir(&repo_path)
            .args(["worktree", "remove", worktree_path.to_str().unwrap()])
            .output()?;

        Ok(())
    }

    #[test]
    fn test_create_worktree_duplicate_branch() -> Result<()> {
        let (_temp_dir, repo_path) = setup_test_repo()?;

        // Create an initial commit
        fs::write(repo_path.join("README.md"), "# Test Repo")?;
        Command::new("git")
            .current_dir(&repo_path)
            .args(["add", "README.md"])
            .output()?;
        Command::new("git")
            .current_dir(&repo_path)
            .args(["commit", "-m", "Initial commit"])
            .output()?;

        // Create first worktree
        let worktree_path1 = repo_path.parent().unwrap().join("test-worktree-1");
        create_worktree(&repo_path, &worktree_path1, "duplicate-branch")?;

        // Try to create second worktree with same branch name
        let worktree_path2 = repo_path.parent().unwrap().join("test-worktree-2");
        let result = create_worktree(&repo_path, &worktree_path2, "duplicate-branch");

        // Should fail
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to create worktree")
        );

        // Clean up
        Command::new("git")
            .current_dir(&repo_path)
            .args(["worktree", "remove", worktree_path1.to_str().unwrap()])
            .output()?;

        Ok(())
    }
}
