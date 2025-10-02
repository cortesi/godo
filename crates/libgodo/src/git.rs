use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use anyhow::{Context, Result};

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

/// Walk up from `start_dir` to find the nearest repository root containing a `.git` directory.
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
/// Check whether the repository has staged or unstaged changes.
pub fn has_uncommitted_changes(repo_path: &Path) -> Result<bool> {
    let output = run_git(repo_path, &["status", "--porcelain"])?;
    let status_output = String::from_utf8_lossy(&output.stdout);
    Ok(!status_output.trim().is_empty())
}
/// Determine if a branch named `branch_name` exists in the repository.
pub fn has_branch(repo_path: &Path, branch_name: &str) -> Result<bool> {
    let output = run_git(repo_path, &["branch", "--list", branch_name])?;
    let branch_output = String::from_utf8_lossy(&output.stdout);
    Ok(!branch_output.trim().is_empty())
}
/// Create a new worktree for `branch_name` under `worktree_path`.
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
/// Remove the worktree located at `worktree_path`, optionally forcing removal.
pub fn remove_worktree(repo_path: &Path, worktree_path: &Path, force: bool) -> Result<()> {
    // First, check if the worktree path is currently registered by Git
    let exists = list_worktrees(repo_path)?
        .into_iter()
        .any(|wt| paths_match(&wt.path, worktree_path));

    if !exists {
        // Treat non-existent worktree as success, since the desired end state is achieved
        return Ok(());
    }

    let worktree_path_str = worktree_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid worktree path"))?;

    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(worktree_path_str);

    run_git(repo_path, &args)?;
    Ok(())
}

/// Best-effort path comparison that tolerates absolute vs relative inputs.
fn paths_match(a: &Path, b: &Path) -> bool {
    // Fast path equality
    if a == b {
        return true;
    }

    // Try canonicalizing both; if that fails (e.g., missing path), fall back to
    // string comparison of absolute forms.
    let can_a = a.canonicalize().ok();
    let can_b = b.canonicalize().ok();
    match (can_a, can_b) {
        (Some(x), Some(y)) => x == y,
        _ => {
            // Join relative paths against the current working directory if available.
            let cwd = env::current_dir().unwrap_or_default();
            let abs_a = if a.is_absolute() {
                a.to_path_buf()
            } else {
                cwd.join(a)
            };
            let abs_b = if b.is_absolute() {
                b.to_path_buf()
            } else {
                cwd.join(b)
            };
            abs_a == abs_b
        }
    }
}

/// Delete the branch named `branch_name`, forcing the deletion when `force` is `true`.
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

/// Stage all tracked and untracked changes in the repository.
pub fn add_all(repo_path: &Path) -> Result<()> {
    run_git(repo_path, &["add", "."])?;
    Ok(())
}

/// Launch an interactive `git commit --verbose` session for the repository.
pub fn commit_interactive(repo_path: &Path) -> Result<()> {
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

/// Create a commit with the provided `message`.
pub fn commit(repo_path: &Path, message: &str) -> Result<()> {
    run_git(repo_path, &["commit", "-m", message])?;
    Ok(())
}

/// Metadata describing a Git worktree as reported by `git worktree list --porcelain`.
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    /// Filesystem path where the worktree is checked out.
    pub path: PathBuf,
    /// Fully-qualified ref backing the worktree, when the worktree is attached to a branch.
    pub branch: Option<String>,
    /// Whether the worktree is currently checked out in detached HEAD state.
    pub is_detached: bool,
}

/// Return all worktrees known to the repository together with their metadata.
pub fn list_worktrees(repo_path: &Path) -> Result<Vec<WorktreeInfo>> {
    let output = run_git(repo_path, &["worktree", "list", "--porcelain"])?;
    let output_str = String::from_utf8_lossy(&output.stdout);

    let mut worktrees = Vec::new();
    let mut current_worktree = None;
    let mut current_branch = None;
    let mut current_detached = false;

    for line in output_str.lines() {
        if let Some(path_str) = line.strip_prefix("worktree ") {
            // Save previous worktree if exists
            if let Some(path) = current_worktree.take() {
                worktrees.push(WorktreeInfo {
                    path,
                    branch: current_branch.take(),
                    is_detached: current_detached,
                });
            }
            // Start new worktree
            current_worktree = Some(PathBuf::from(path_str));
            current_detached = false;
        } else if let Some(branch) = line.strip_prefix("branch ") {
            current_branch = Some(branch.to_string());
        } else if line == "detached" {
            current_detached = true;
        }
    }

    // Save last worktree
    if let Some(path) = current_worktree {
        worktrees.push(WorktreeInfo {
            path,
            branch: current_branch,
            is_detached: current_detached,
        });
    }

    Ok(worktrees)
}

/// Enumerate every branch in the repository, returning their short names.
pub fn list_branches(repo_path: &Path) -> Result<Vec<String>> {
    let output = run_git(repo_path, &["branch", "--format=%(refname:short)"])?;
    let output_str = String::from_utf8_lossy(&output.stdout);

    Ok(output_str
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|branch| !branch.is_empty())
        .collect())
}

/// Merge relationship between a sandbox branch and its integration target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStatus {
    /// The branch contains commits that are not on the integration target.
    Diverged,
    /// The branch tip is fully merged into the integration target.
    Clean,
    /// The relationship could not be determined (missing upstream, missing remote, etc.).
    Unknown,
}

/// Determine if a branch is ahead of its integration target.
pub fn branch_merge_status(repo_path: &Path, branch_name: &str) -> Result<MergeStatus> {
    // If the branch itself is missing, we cannot establish a relationship.
    if !has_branch(repo_path, branch_name)? {
        return Ok(MergeStatus::Unknown);
    }

    let mut candidates = Vec::new();

    if let Some(upstream) = upstream_of(repo_path, branch_name)? {
        candidates.push(upstream);
    }

    if let Some(default_target) = default_integration_target(repo_path)?
        && !candidates.contains(&default_target)
    {
        candidates.push(default_target);
    }

    // Fall back to common branch names if everything else failed.
    for fallback in ["main", "master"] {
        if candidates.iter().any(|c| c == fallback) {
            continue;
        }
        if has_branch(repo_path, fallback)? {
            candidates.push(fallback.to_string());
        }
    }

    for target in candidates {
        match run_git(
            repo_path,
            &["rev-list", "--count", &format!("{target}..{branch_name}")],
        ) {
            Ok(output) => {
                let count = String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .parse::<u32>()
                    .unwrap_or(0);
                return if count > 0 {
                    Ok(MergeStatus::Diverged)
                } else {
                    Ok(MergeStatus::Clean)
                };
            }
            Err(_) => {
                // Try next candidate; failing to look at one target shouldn't abort the search.
                continue;
            }
        }
    }

    Ok(MergeStatus::Unknown)
}

/// Determine the configured upstream for a given branch, if any.
fn upstream_of(repo_path: &Path, branch_name: &str) -> Result<Option<String>> {
    let ref_name = format!("refs/heads/{branch_name}");
    let output = run_git(
        repo_path,
        &["for-each-ref", "--format=%(upstream:short)", &ref_name],
    )?;
    let upstream = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if upstream.is_empty() {
        Ok(None)
    } else {
        Ok(Some(upstream))
    }
}

/// Discover a reasonable default integration target for the repository.
fn default_integration_target(repo_path: &Path) -> Result<Option<String>> {
    if let Ok(output) = run_git(
        repo_path,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) {
        let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !head.is_empty() {
            return Ok(Some(head));
        }
    }

    if let Ok(output) = run_git(repo_path, &["config", "--get", "init.defaultBranch"]) {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !branch.is_empty() {
            return Ok(Some(branch));
        }
    }

    Ok(None)
}

/// Reset the working directory to match `HEAD`, removing all uncommitted changes.
pub fn reset_hard(repo_path: &Path) -> Result<()> {
    run_git(repo_path, &["reset", "--hard", "HEAD"])?;
    Ok(())
}

/// Remove untracked files and directories from the working tree.
pub fn clean(repo_path: &Path) -> Result<()> {
    run_git(repo_path, &["clean", "-fd"])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn setup_test_repo() -> Result<(TempDir, PathBuf)> {
        let temp_dir = TempDir::new()?;
        let repo_path = temp_dir.path().to_path_buf();

        // Initialize git repository
        run_git(&repo_path, &["init", "-b", "main"])?;

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
        assert!(
            find_root(&root_path)
                .as_ref()
                .is_some_and(|path| path == &root_path)
        );

        // Create nested directories
        let sub_dir = root_path.join("src");
        fs::create_dir(&sub_dir)?;
        let nested_dir = sub_dir.join("nested");
        fs::create_dir(&nested_dir)?;

        // Test from subdirectory
        assert!(
            find_root(&sub_dir)
                .as_ref()
                .is_some_and(|path| path == &root_path)
        );

        // Test from deeply nested directory
        assert!(
            find_root(&nested_dir)
                .as_ref()
                .is_some_and(|path| path == &root_path)
        );

        // Test from non-git directory
        let non_git_dir = temp_dir.path().parent().unwrap();
        assert_eq!(find_root(non_git_dir), None);

        Ok(())
    }

    #[test]
    fn test_commit() -> Result<()> {
        let (_temp_dir, repo_path) = setup_test_repo()?;

        // Create and stage a file
        fs::write(repo_path.join("test.txt"), "test content")?;
        add_all(&repo_path)?;

        // Commit with a message
        commit(&repo_path, "Test commit message")?;

        // Verify the commit was created
        let log_output = run_git(&repo_path, &["log", "--oneline", "-1"])?;
        let log_str = String::from_utf8_lossy(&log_output.stdout);
        assert!(log_str.contains("Test commit message"));

        // Verify no uncommitted changes remain
        assert!(!has_uncommitted_changes(&repo_path)?);

        Ok(())
    }

    #[test]
    fn test_reset_hard_and_clean() -> Result<()> {
        let (_temp_dir, repo_path) = setup_test_repo()?;

        // Create and commit a file
        fs::write(repo_path.join("test.txt"), "initial content")?;
        run_git(&repo_path, &["add", "test.txt"])?;
        run_git(&repo_path, &["commit", "-m", "Initial commit"])?;

        // Modify the file and create a new untracked file
        fs::write(repo_path.join("test.txt"), "modified content")?;
        fs::write(repo_path.join("untracked.txt"), "untracked content")?;

        // Should have uncommitted changes
        assert!(has_uncommitted_changes(&repo_path)?);

        // Reset to HEAD
        reset_hard(&repo_path)?;

        // Modified file should be back to original state
        let content = fs::read_to_string(repo_path.join("test.txt"))?;
        assert_eq!(content, "initial content");

        // Untracked file should still exist
        assert!(repo_path.join("untracked.txt").exists());

        // Should still have uncommitted changes (untracked file)
        assert!(has_uncommitted_changes(&repo_path)?);

        // Clean untracked files
        clean(&repo_path)?;

        // Should have no uncommitted changes after cleaning
        assert!(!has_uncommitted_changes(&repo_path)?);
        assert!(!repo_path.join("untracked.txt").exists());

        Ok(())
    }

    #[test]
    fn test_branch_merge_status_detects_diverged_and_clean() -> Result<()> {
        let (_temp_dir, repo_path) = setup_test_repo()?;

        fs::write(repo_path.join("base.txt"), "base")?;
        run_git(&repo_path, &["add", "base.txt"])?;
        run_git(&repo_path, &["commit", "-m", "Base commit"])?;

        run_git(&repo_path, &["checkout", "-b", "feature"])?;
        fs::write(repo_path.join("feature.txt"), "work in progress")?;
        run_git(&repo_path, &["add", "feature.txt"])?;
        run_git(&repo_path, &["commit", "-m", "Feature work"])?;

        assert_eq!(
            branch_merge_status(&repo_path, "feature")?,
            MergeStatus::Diverged
        );

        run_git(&repo_path, &["checkout", "main"])?;
        run_git(&repo_path, &["merge", "feature"])?;

        assert_eq!(
            branch_merge_status(&repo_path, "feature")?,
            MergeStatus::Clean
        );

        Ok(())
    }

    #[test]
    fn test_branch_merge_status_unknown_without_baseline() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let repo_path = temp_dir.path().to_path_buf();

        run_git(&repo_path, &["init", "-b", "release"])?;
        run_git(&repo_path, &["config", "user.email", "test@example.com"])?;
        run_git(&repo_path, &["config", "user.name", "Test User"])?;
        run_git(
            &repo_path,
            &["commit", "--allow-empty", "-m", "Initial commit"],
        )?;

        assert_eq!(
            branch_merge_status(&repo_path, "release")?,
            MergeStatus::Unknown
        );

        Ok(())
    }
}
