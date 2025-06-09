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
}