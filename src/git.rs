use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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

pub fn find_root(start_dir: &Path) -> Option<PathBuf> {
    let mut current = start_dir;
    loop {
        if current.join(".git").exists() {
            return Some(current.to_path_buf());
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => return None,
        }
    }
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

pub fn remove_worktree(repo_path: &Path, worktree_path: &Path, force: bool) -> Result<()> {
    let worktree_path_str = worktree_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid worktree path"))?;

    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(worktree_path_str);

    match run_git(repo_path, &args) {
        Ok(_) => Ok(()),
        Err(e) => {
            // Check if the error is because the worktree doesn't exist
            let error_msg = e.to_string();
            if error_msg.contains("is not a working tree") {
                // Worktree already removed, not an error
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

pub fn delete_branch(repo_path: &Path, branch_name: &str, force: bool) -> Result<()> {
    let mut args = vec!["branch"];
    if force {
        args.push("-D");
    } else {
        args.push("-d");
    }
    args.push(branch_name);

    run_git(repo_path, &args)?;
    Ok(())
}

pub fn add_all(repo_path: &Path) -> Result<()> {
    run_git(repo_path, &["add", "."])?;
    Ok(())
}

pub fn commit_verbose(repo_path: &Path) -> Result<()> {
    let status = Command::new("git")
        .current_dir(repo_path)
        .args(["commit", "--verbose"])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| "Failed to execute git commit --verbose")?;

    if !status.success() {
        anyhow::bail!("Git commit failed");
    }

    Ok(())
}

pub fn worktree_has_commits(worktree_path: &Path) -> Result<bool> {
    // Get the current branch name
    let branch_output = run_git(worktree_path, &["branch", "--show-current"])?;
    let branch_name = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_string();

    if branch_name.is_empty() {
        // Detached HEAD state
        return Ok(false);
    }

    // Find the fork point of this branch by looking at all branches and finding the merge-base
    // First, get all branches except the current one
    let all_branches_output = run_git(
        worktree_path,
        &["branch", "-a", "--format=%(refname:short)"],
    )?;
    let all_branches = String::from_utf8_lossy(&all_branches_output.stdout);

    let mut fork_point = None;

    for branch_line in all_branches.lines() {
        let other_branch = branch_line.trim();

        // Skip the current branch and remote branches
        if other_branch.is_empty()
            || other_branch == branch_name
            || other_branch.starts_with("origin/")
        {
            continue;
        }

        // Try to find the merge-base with this branch
        if let Ok(merge_base_output) =
            run_git(worktree_path, &["merge-base", &branch_name, other_branch])
        {
            let merge_base = String::from_utf8_lossy(&merge_base_output.stdout)
                .trim()
                .to_string();

            if !merge_base.is_empty() {
                // Check if this merge-base is the same as the branch point
                // We want the most recent common ancestor
                if fork_point.is_none() {
                    fork_point = Some(merge_base);
                    break; // For godo worktrees, we can assume the first valid merge-base is the fork point
                }
            }
        }
    }

    if let Some(fork_commit) = fork_point {
        // Count commits from fork point to HEAD
        match run_git(
            worktree_path,
            &["rev-list", "--count", &format!("{fork_commit}..HEAD")],
        ) {
            Ok(output) => {
                let count_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let count = count_str.parse::<u32>().unwrap_or(0);
                Ok(count > 0)
            }
            Err(_) => Ok(false),
        }
    } else {
        // No fork point found - check if this is an orphan branch with commits
        match run_git(worktree_path, &["rev-list", "--count", "HEAD"]) {
            Ok(output) => {
                let count_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let count = count_str.parse::<u32>().unwrap_or(0);
                Ok(count > 0)
            }
            Err(_) => Ok(false),
        }
    }
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

    #[test]
    fn test_worktree_has_commits() -> Result<()> {
        let (temp_dir, repo_path) = setup_test_repo()?;

        // Create an initial commit in main
        fs::write(repo_path.join("README.md"), "# Test Repo")?;
        run_git(&repo_path, &["add", "README.md"])?;
        run_git(&repo_path, &["commit", "-m", "Initial commit"])?;

        // Create a worktree with a new branch
        let worktree_path = temp_dir.path().join("test-worktree");
        create_worktree(&repo_path, &worktree_path, "new-branch")?;

        // New branch should have no new commits yet
        assert!(!worktree_has_commits(&worktree_path)?);

        // Make a commit in the worktree
        fs::write(worktree_path.join("test.txt"), "test content")?;
        run_git(&worktree_path, &["add", "test.txt"])?;
        run_git(
            &worktree_path,
            &["commit", "-m", "First commit in worktree"],
        )?;

        // Now the worktree should have commits
        assert!(worktree_has_commits(&worktree_path)?);

        // Clean up
        run_git(
            &repo_path,
            &["worktree", "remove", worktree_path.to_str().unwrap()],
        )?;

        Ok(())
    }

    #[test]
    fn test_worktree_has_commits_orphan_branch() -> Result<()> {
        let (temp_dir, repo_path) = setup_test_repo()?;

        // Create an initial commit in main
        fs::write(repo_path.join("README.md"), "# Test Repo")?;
        run_git(&repo_path, &["add", "README.md"])?;
        run_git(&repo_path, &["commit", "-m", "Initial commit"])?;

        // Create a worktree with an orphan branch (no commit history)
        let worktree_path = temp_dir.path().join("orphan-worktree");
        run_git(
            &repo_path,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                worktree_path.to_str().unwrap(),
            ],
        )?;

        // Switch to orphan branch in worktree
        run_git(&worktree_path, &["checkout", "--orphan", "orphan-branch"])?;

        // Orphan branch should have no commits
        assert!(!worktree_has_commits(&worktree_path)?);

        // Make a commit in the orphan branch
        fs::write(worktree_path.join("orphan.txt"), "orphan content")?;
        run_git(&worktree_path, &["add", "orphan.txt"])?;
        run_git(&worktree_path, &["commit", "-m", "First orphan commit"])?;

        // Now it should have commits
        assert!(worktree_has_commits(&worktree_path)?);

        // Clean up
        run_git(
            &repo_path,
            &["worktree", "remove", worktree_path.to_str().unwrap()],
        )?;

        Ok(())
    }

    #[test]
    fn test_worktree_has_commits_uncommitted_changes() -> Result<()> {
        let (temp_dir, repo_path) = setup_test_repo()?;

        // Create an initial commit in main
        fs::write(repo_path.join("README.md"), "# Test Repo")?;
        run_git(&repo_path, &["add", "README.md"])?;
        run_git(&repo_path, &["commit", "-m", "Initial commit"])?;

        // Create a worktree with a new branch
        let worktree_path = temp_dir.path().join("test-worktree");
        create_worktree(&repo_path, &worktree_path, "uncommitted-branch")?;

        // Add uncommitted changes
        fs::write(worktree_path.join("uncommitted.txt"), "uncommitted content")?;
        run_git(&worktree_path, &["add", "uncommitted.txt"])?;

        // Should still return false because there are no commits
        assert!(!worktree_has_commits(&worktree_path)?);

        // Also test with untracked files
        fs::write(worktree_path.join("untracked.txt"), "untracked content")?;

        // Should still return false
        assert!(!worktree_has_commits(&worktree_path)?);

        // Clean up (need --force because of uncommitted changes)
        run_git(
            &repo_path,
            &[
                "worktree",
                "remove",
                "--force",
                worktree_path.to_str().unwrap(),
            ],
        )?;

        Ok(())
    }

    #[test]
    fn test_worktree_has_commits_from_feature_branch() -> Result<()> {
        let (temp_dir, repo_path) = setup_test_repo()?;

        // Create initial commits in main
        fs::write(repo_path.join("README.md"), "# Test Repo")?;
        run_git(&repo_path, &["add", "README.md"])?;
        run_git(&repo_path, &["commit", "-m", "Initial commit"])?;

        // Create a feature branch and switch to it
        run_git(&repo_path, &["checkout", "-b", "feature-branch"])?;

        // Add a commit to the feature branch
        fs::write(repo_path.join("feature.txt"), "feature content")?;
        run_git(&repo_path, &["add", "feature.txt"])?;
        run_git(&repo_path, &["commit", "-m", "Feature commit"])?;

        // Create a worktree from the feature branch
        let worktree_path = temp_dir.path().join("feature-worktree");
        create_worktree(&repo_path, &worktree_path, "sub-feature")?;

        // Should have no new commits yet (relative to feature-branch)
        assert!(!worktree_has_commits(&worktree_path)?);

        // Make a commit in the worktree
        fs::write(worktree_path.join("sub-feature.txt"), "sub-feature content")?;
        run_git(&worktree_path, &["add", "sub-feature.txt"])?;
        run_git(&worktree_path, &["commit", "-m", "Sub-feature commit"])?;

        // Now should have commits
        assert!(worktree_has_commits(&worktree_path)?);

        // Clean up
        run_git(
            &repo_path,
            &["worktree", "remove", worktree_path.to_str().unwrap()],
        )?;

        Ok(())
    }

    #[test]
    fn test_remove_worktree() -> Result<()> {
        let (temp_dir, repo_path) = setup_test_repo()?;

        // Create an initial commit
        fs::write(repo_path.join("README.md"), "# Test Repo")?;
        run_git(&repo_path, &["add", "README.md"])?;
        run_git(&repo_path, &["commit", "-m", "Initial commit"])?;

        // Create a worktree
        let worktree_path = temp_dir.path().join("test-worktree");
        create_worktree(&repo_path, &worktree_path, "test-branch")?;

        // Verify worktree exists
        assert!(worktree_path.exists());

        // Remove the worktree
        remove_worktree(&repo_path, &worktree_path, false)?;

        // Verify worktree is removed
        assert!(!worktree_path.exists());

        Ok(())
    }

    #[test]
    fn test_remove_worktree_with_uncommitted_changes() -> Result<()> {
        let (temp_dir, repo_path) = setup_test_repo()?;

        // Create an initial commit
        fs::write(repo_path.join("README.md"), "# Test Repo")?;
        run_git(&repo_path, &["add", "README.md"])?;
        run_git(&repo_path, &["commit", "-m", "Initial commit"])?;

        // Create a worktree
        let worktree_path = temp_dir.path().join("test-worktree");
        create_worktree(&repo_path, &worktree_path, "test-branch")?;

        // Add uncommitted changes
        fs::write(worktree_path.join("uncommitted.txt"), "uncommitted content")?;
        run_git(&worktree_path, &["add", "uncommitted.txt"])?;

        // Try to remove without force - should fail
        let result = remove_worktree(&repo_path, &worktree_path, false);
        assert!(result.is_err());

        // Verify worktree still exists
        assert!(worktree_path.exists());

        // Remove with force - should succeed
        remove_worktree(&repo_path, &worktree_path, true)?;

        // Verify worktree is removed
        assert!(!worktree_path.exists());

        Ok(())
    }

    #[test]
    fn test_remove_worktree_already_removed() -> Result<()> {
        let (temp_dir, repo_path) = setup_test_repo()?;

        // Create an initial commit
        fs::write(repo_path.join("README.md"), "# Test Repo")?;
        run_git(&repo_path, &["add", "README.md"])?;
        run_git(&repo_path, &["commit", "-m", "Initial commit"])?;

        // Try to remove a non-existent worktree - should not error
        let worktree_path = temp_dir.path().join("non-existent-worktree");
        let result = remove_worktree(&repo_path, &worktree_path, false);

        // Should succeed (we handle "is not a working tree" as success)
        assert!(result.is_ok());

        Ok(())
    }

    #[test]
    fn test_delete_branch() -> Result<()> {
        let (_temp_dir, repo_path) = setup_test_repo()?;

        // Create an initial commit
        fs::write(repo_path.join("README.md"), "# Test Repo")?;
        run_git(&repo_path, &["add", "README.md"])?;
        run_git(&repo_path, &["commit", "-m", "Initial commit"])?;

        // Create a new branch
        run_git(&repo_path, &["branch", "test-branch"])?;

        // Verify branch exists
        assert!(has_branch(&repo_path, "test-branch")?);

        // Delete the branch
        delete_branch(&repo_path, "test-branch", false)?;

        // Verify branch is deleted
        assert!(!has_branch(&repo_path, "test-branch")?);

        Ok(())
    }

    #[test]
    fn test_delete_branch_with_unmerged_commits() -> Result<()> {
        let (_temp_dir, repo_path) = setup_test_repo()?;

        // Create an initial commit
        fs::write(repo_path.join("README.md"), "# Test Repo")?;
        run_git(&repo_path, &["add", "README.md"])?;
        run_git(&repo_path, &["commit", "-m", "Initial commit"])?;

        // Create and switch to a new branch
        run_git(&repo_path, &["checkout", "-b", "feature-branch"])?;

        // Make a commit on the feature branch
        fs::write(repo_path.join("feature.txt"), "feature content")?;
        run_git(&repo_path, &["add", "feature.txt"])?;
        run_git(&repo_path, &["commit", "-m", "Feature commit"])?;

        // Switch back to main
        run_git(&repo_path, &["checkout", "main"])?;

        // Try to delete without force - should fail
        let result = delete_branch(&repo_path, "feature-branch", false);
        assert!(result.is_err());

        // Verify branch still exists
        assert!(has_branch(&repo_path, "feature-branch")?);

        // Delete with force - should succeed
        delete_branch(&repo_path, "feature-branch", true)?;

        // Verify branch is deleted
        assert!(!has_branch(&repo_path, "feature-branch")?);

        Ok(())
    }

    #[test]
    fn test_delete_branch_nonexistent() -> Result<()> {
        let (_temp_dir, repo_path) = setup_test_repo()?;

        // Create an initial commit
        fs::write(repo_path.join("README.md"), "# Test Repo")?;
        run_git(&repo_path, &["add", "README.md"])?;
        run_git(&repo_path, &["commit", "-m", "Initial commit"])?;

        // Try to delete a non-existent branch - should fail
        let result = delete_branch(&repo_path, "nonexistent-branch", false);
        assert!(result.is_err());

        Ok(())
    }

    #[test]
    fn test_find_root() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let root_path = temp_dir.path().to_path_buf();

        // Create a git repo
        run_git(&root_path, &["init"])?;

        // Test from root directory
        assert_eq!(find_root(&root_path), Some(root_path.clone()));

        // Create nested directories
        let sub_dir = root_path.join("src");
        fs::create_dir(&sub_dir)?;
        let nested_dir = sub_dir.join("nested");
        fs::create_dir(&nested_dir)?;

        // Test from subdirectory
        assert_eq!(find_root(&sub_dir), Some(root_path.clone()));

        // Test from deeply nested directory
        assert_eq!(find_root(&nested_dir), Some(root_path.clone()));

        // Test from non-git directory
        let non_git_dir = temp_dir.path().parent().unwrap();
        assert_eq!(find_root(non_git_dir), None);

        Ok(())
    }
}
