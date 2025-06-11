use anyhow::{Context, Result};
use clonetree::{Options, clone_tree};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use crate::git;
use crate::output::Output;

/// Validates that a sandbox name contains only allowed characters (a-zA-Z0-9-_)
fn validate_sandbox_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("Sandbox name cannot be empty");
    }

    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!(
            "Invalid sandbox name '{}': names can only contain letters, numbers, hyphens, and underscores",
            name
        );
    }

    Ok(())
}

/// Cleans a name to only contain allowed characters (a-zA-Z0-9-_)
/// Non-allowed characters are replaced with hyphens
fn clean_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Extracts and cleans the project name from a git repository path
fn project_name(repo_path: &Path) -> Result<String> {
    let name = repo_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_else(|| {
            // For root paths or paths without a file name, try to use the last component
            repo_path
                .components()
                .next_back()
                .and_then(|c| match c {
                    std::path::Component::Normal(name) => name.to_str(),
                    _ => None,
                })
                .unwrap_or("root")
        });

    let cleaned = clean_name(name);

    if cleaned.is_empty() {
        anyhow::bail!("Could not derive a valid project name from repository path");
    }

    Ok(cleaned)
}

/// Returns the git branch name for a given sandbox name
fn branch_name(sandbox_name: &str) -> String {
    format!("godo/{sandbox_name}")
}

pub struct Godo {
    godo_dir: PathBuf,
    repo_dir: PathBuf,
    no_prompt: bool,
    output: Arc<dyn Output>,
}

impl Godo {
    pub fn new(
        godo_dir: PathBuf,
        repo_dir: Option<PathBuf>,
        output: Arc<dyn Output>,
        no_prompt: bool,
    ) -> Result<Self> {
        // Ensure godo directory exists
        ensure_godo_directory(&godo_dir)?;

        // Determine repository directory
        let repo_dir = if let Some(dir) = repo_dir {
            dir
        } else {
            // Find git root from current directory
            let current_dir = std::env::current_dir().context("Failed to get current directory")?;
            git::find_root(&current_dir).context("Not in a git repository")?
        };

        Ok(Self {
            godo_dir,
            repo_dir,
            no_prompt,
            output,
        })
    }

    /// Get the project directory path within the godo directory
    fn project_dir(&self) -> Result<PathBuf> {
        let project = project_name(&self.repo_dir)?;
        Ok(self.godo_dir.join(&project))
    }

    /// Calculate the path for a sandbox given its name
    fn sandbox_path(&self, sandbox_name: &str) -> Result<PathBuf> {
        Ok(self.project_dir()?.join(sandbox_name))
    }

    pub fn run(
        &self,
        keep: bool,
        autocommit: bool,
        excludes: &[String],
        sandbox_name: &str,
        command: &[String],
    ) -> Result<()> {
        // Validate sandbox name
        validate_sandbox_name(sandbox_name)?;

        // Check for uncommitted changes
        if git::has_uncommitted_changes(&self.repo_dir)? {
            self.output.warn("Warning: You have uncommitted changes:")?;
            if !self.no_prompt && !self.output.confirm("Continue creating worktree?")? {
                anyhow::bail!("Aborted by user");
            }
        }

        let sandbox_path = self.sandbox_path(sandbox_name)?;

        // Ensure project directory exists
        let project_dir = self.project_dir()?;
        fs::create_dir_all(&project_dir)?;

        let branch = branch_name(sandbox_name);
        self.output.message(&format!(
            "Creating sandbox {sandbox_name} with branch {branch} at {sandbox_path:?}"
        ))?;
        git::create_worktree(&self.repo_dir, &sandbox_path, &branch)?;

        self.output.message("Copying files to sandbox...")?;

        let mut clone_options = Options::new().overwrite(true).glob("!.git/**");

        // Add user-specified excludes with "!" prefix
        for exclude in excludes {
            clone_options = clone_options.glob(format!("!{exclude}"));
        }

        clone_tree(&self.repo_dir, &sandbox_path, &clone_options)
            .context("Failed to copy files to sandbox")?;

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        let status = if command.is_empty() {
            // Interactive shell
            Command::new(&shell)
                .current_dir(&sandbox_path)
                .status()
                .context("Failed to start shell")?
        } else {
            // Run the specified command
            let command_string = command.join(" ");
            Command::new(&shell)
                .arg("-c")
                .arg(&command_string)
                .current_dir(&sandbox_path)
                .status()
                .context("Failed to run command")?
        };

        if !status.success() {
            anyhow::bail!("Command exited with status: {status}");
        }

        // Handle autocommit if requested
        if autocommit && git::has_uncommitted_changes(&sandbox_path)? {
            self.output.message("Auto-committing changes...")?;

            // Stage all changes
            git::add_all(&sandbox_path)?;

            // Commit with verbose flag
            git::commit_interactive(&sandbox_path)?;
        }

        // Clean up if not keeping
        if !keep {
            self.cleanup_sandbox(sandbox_name, false)?;
        }

        Ok(())
    }

    pub fn list(&self) -> Result<()> {
        let project_dir = self.project_dir()?;

        // Get all worktrees from git
        let worktrees = git::list_worktrees(&self.repo_dir)?;

        // Get all branches
        let all_branches = git::list_branches(&self.repo_dir)?;

        // Filter godo branches (those that start with "godo/")
        let godo_branches: Vec<String> = all_branches
            .into_iter()
            .filter(|branch| branch.starts_with("godo/"))
            .collect();

        // Get directories in the project directory
        let mut dangling_dirs = Vec::new();
        if project_dir.exists() {
            for entry in fs::read_dir(&project_dir)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    let dir_name = entry.file_name().to_string_lossy().to_string();
                    dangling_dirs.push(dir_name);
                }
            }
        }

        // Find active worktrees (those with directories)
        let mut active_sandboxes = Vec::new();
        for worktree in &worktrees {
            let branch = worktree
                .branch
                .strip_prefix("refs/heads/")
                .unwrap_or(&worktree.branch);
            if branch.starts_with("godo/") {
                if let Some(sandbox_name) = branch.strip_prefix("godo/") {
                    active_sandboxes.push(sandbox_name.to_string());
                    // Remove from dangling dirs since it's active
                    dangling_dirs.retain(|d| d != sandbox_name);
                }
            }
        }

        // Find branches without worktrees (ready for merge)
        let mut ready_for_merge = Vec::new();
        for branch in &godo_branches {
            if let Some(sandbox_name) = branch.strip_prefix("godo/") {
                let has_worktree = worktrees
                    .iter()
                    .any(|w| w.branch == format!("refs/heads/{branch}") || &w.branch == branch);

                if !has_worktree {
                    ready_for_merge.push(sandbox_name.to_string());
                    // Remove from dangling dirs since it has a branch
                    dangling_dirs.retain(|d| d != sandbox_name);
                }
            }
        }

        // Active sandboxes
        if !active_sandboxes.is_empty() {
            self.output.success("Running (with worktrees):")?;
            for sandbox in &active_sandboxes {
                self.output.message(&format!("  - {sandbox}"))?;
            }
            self.output.message("")?;
        }

        // Ready for merge
        if !ready_for_merge.is_empty() {
            self.output
                .warn("Ready for merge (branches without worktrees):")?;
            for sandbox in &ready_for_merge {
                self.output
                    .message(&format!("  - {sandbox} (branch: godo/{sandbox})"))?;
            }
            self.output.message("")?;
        }

        // Dangling directories
        if !dangling_dirs.is_empty() {
            self.output
                .fail("Dangling (directories without branches):")?;
            for dir in &dangling_dirs {
                let dir_path = project_dir.join(dir);
                self.output
                    .message(&format!("  - {dir} (at {dir_path:?})"))?;
            }
            self.output.message("")?;
        }

        if active_sandboxes.is_empty() && ready_for_merge.is_empty() && dangling_dirs.is_empty() {
            self.output.message("No sandboxes found.")?;
        }

        Ok(())
    }

    pub fn remove(&self, name: &str, force: bool) -> Result<()> {
        // Validate sandbox name
        validate_sandbox_name(name)?;

        let sandbox_path = self.sandbox_path(name)?;

        // Check if sandbox exists
        if !sandbox_path.exists() {
            anyhow::bail!("Sandbox '{}' does not exist", name);
        }

        // Check for uncommitted changes in the worktree
        if !force && git::has_uncommitted_changes(&sandbox_path)? {
            self.output.warn(&format!(
                "Warning: Sandbox '{name}' has uncommitted changes"
            ))?;

            if !self.no_prompt && !self.output.confirm("Continue with removal? [y/N]")? {
                anyhow::bail!("Aborted by user");
            }
        }

        self.output
            .warn(&format!("Removing sandbox {name} from {:?}", self.godo_dir))?;

        self.cleanup_sandbox(name, force)
    }

    pub fn prune(&self) -> Result<()> {
        let project = project_name(&self.repo_dir)?;
        let project_dir = self.project_dir()?;

        self.output.warn(&format!(
            "Pruning sandboxes for project '{project}' in {project_dir:?}"
        ))?;

        // TODO: Implement prune functionality
        // 1. List all sandboxes
        // 2. Check which branches still exist
        // 3. Remove sandboxes whose branches are gone

        Ok(())
    }

    /// Clean up a sandbox by removing worktree and optionally the branch
    fn cleanup_sandbox(&self, name: &str, force: bool) -> Result<()> {
        self.output
            .message(&format!("Cleaning up sandbox {name}..."))?;

        let sandbox_path = self.sandbox_path(name)?;
        let branch = branch_name(name);

        // Check if the worktree exists and has commits before trying to remove it
        let has_commits = if sandbox_path.exists() {
            git::worktree_has_commits(&sandbox_path).unwrap_or(false)
        } else {
            false
        };

        // Try to remove the worktree
        match git::remove_worktree(&self.repo_dir, &sandbox_path, force) {
            Ok(_) => {
                // Worktree removed successfully
            }
            Err(e) => {
                let error_msg = e.to_string();
                let trimmed_msg = error_msg.trim();
                self.output
                    .warn(&format!("Cannot remove worktree: {trimmed_msg}"))?;
                if !force && error_msg.contains("uncommitted") {
                    self.output.warn(&format!(
                        "Worktree has uncommitted changes. Use 'godo rm {name} --force' to force removal."
                    ))?;
                }
                return Ok(());
            }
        }

        // Clean up the directory if it still exists (in case git didn't remove it completely)
        if sandbox_path.exists() {
            fs::remove_dir_all(&sandbox_path).context("Failed to remove sandbox directory")?;
        }

        // Only delete the branch if there were no commits or if forced
        if !has_commits || force {
            match git::delete_branch(&self.repo_dir, &branch, force) {
                Ok(_) => {
                    self.output
                        .success("Sandbox and branch cleaned up successfully.")?;
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    let trimmed_msg = error_msg.trim();
                    self.output
                        .warn(&format!("Warning: Failed to delete branch: {trimmed_msg}"))?;
                }
            }
        } else {
            self.output.success(&format!(
                "Sandbox removed. Branch {branch} kept (has commits)."
            ))?;
        }

        Ok(())
    }
}

fn ensure_godo_directory(godo_dir: &Path) -> Result<()> {
    // Create main godo directory
    fs::create_dir_all(godo_dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::Quiet;
    use std::path::PathBuf;

    #[test]
    fn test_clean_name() {
        let test_cases = vec![
            // (input, expected)
            // Valid characters (should not change)
            ("valid-name", "valid-name"),
            ("valid_name", "valid_name"),
            ("ValidName123", "ValidName123"),
            ("a-b_c123", "a-b_c123"),
            // Special characters replacement
            ("my.project", "my-project"),
            ("my project", "my-project"),
            ("my@project!", "my-project-"),
            ("my/project", "my-project"),
            ("my\\project", "my-project"),
            ("my:project", "my-project"),
            ("my*project", "my-project"),
            ("my?project", "my-project"),
            ("my\"project", "my-project"),
            ("my<project>", "my-project-"),
            ("my|project", "my-project"),
            // Unicode and non-ASCII characters
            ("café", "caf-"),
            ("项目", "--"),
            ("проект", "------"),
            ("🚀rocket", "-rocket"),
            // Multiple consecutive special characters
            ("my...project", "my---project"),
            ("my   project", "my---project"),
            ("my@#$%project", "my----project"),
            // Edge cases
            ("", ""),
            ("-", "-"),
            ("_", "_"),
            ("---", "---"),
            ("@#$%", "----"),
            // Beginning and ending special characters
            (".project", "-project"),
            ("project.", "project-"),
            (".project.", "-project-"),
            // Real-world examples
            ("my-awesome-project", "my-awesome-project"),
            ("MyCompany.Project", "MyCompany-Project"),
            ("2024-project", "2024-project"),
            ("project (copy)", "project--copy-"),
            ("project [v2]", "project--v2-"),
        ];

        for (input, expected) in test_cases {
            assert_eq!(clean_name(input), expected, "Failed for input: '{input}'");
        }
    }

    #[test]
    fn test_project_name() {
        let test_cases = vec![
            // (path, expected)
            // Normal project names
            ("/home/user/projects/my-project", "my-project"),
            ("/home/user/projects/my_project", "my_project"),
            ("/home/user/projects/MyProject123", "MyProject123"),
            // Names with special characters that get replaced
            ("/home/user/projects/my.project", "my-project"),
            ("/home/user/projects/my project", "my-project"),
            ("/home/user/projects/my@project!", "my-project-"),
            // Edge cases
            ("/", "root"),
            ("project", "project"),
            // Paths that result in all special chars being replaced
            ("/home/user/projects/...", "---"),
            ("/home/user/projects/@#$%^&*()", "---------"),
        ];

        for (path, expected) in test_cases {
            let result = project_name(&PathBuf::from(path)).unwrap();
            assert_eq!(result, expected, "Failed for path: '{path}'");
        }
    }

    #[test]
    fn test_validate_sandbox_name() {
        let valid_names = vec![
            "test",
            "test-123",
            "test_123",
            "TEST",
            "a1b2c3",
            "my-feature-branch",
            "bug_fix_123",
            "RELEASE-2024",
        ];

        let invalid_names = vec![
            "",              // empty
            "test space",    // contains space
            "test.dot",      // contains dot
            "test@symbol",   // contains @
            "test/slash",    // contains /
            "test\\back",    // contains backslash
            "test:colon",    // contains colon
            "test*star",     // contains asterisk
            "test?question", // contains question mark
            "test|pipe",     // contains pipe
        ];

        for name in valid_names {
            assert!(
                validate_sandbox_name(name).is_ok(),
                "Expected '{name}' to be valid"
            );
        }

        for name in invalid_names {
            assert!(
                validate_sandbox_name(name).is_err(),
                "Expected '{name}' to be invalid"
            );
        }
    }

    #[test]
    fn test_branch_name() {
        let test_cases = vec![
            ("test", "godo/test"),
            ("feature-123", "godo/feature-123"),
            ("my_sandbox", "godo/my_sandbox"),
            ("bugfix", "godo/bugfix"),
            ("RELEASE-2024", "godo/RELEASE-2024"),
        ];

        for (sandbox, expected) in test_cases {
            assert_eq!(
                branch_name(sandbox),
                expected,
                "Failed for sandbox name: '{sandbox}'"
            );
        }
    }

    #[test]
    fn test_sandbox_and_project_paths() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let godo_dir = temp_dir.path().join(".godo");
        let repo_dir = PathBuf::from("/home/user/projects/my-project");

        // Create a Godo instance
        let output: Arc<dyn Output> = Arc::new(Quiet);
        let godo = Godo::new(godo_dir.clone(), Some(repo_dir), output, false).unwrap();

        // Test project_dir method
        let project_dir = godo.project_dir().unwrap();
        assert_eq!(project_dir, godo_dir.join("my-project"));

        // Test sandbox_path method
        let sandbox_path = godo.sandbox_path("test-sandbox").unwrap();
        assert_eq!(
            sandbox_path,
            godo_dir.join("my-project").join("test-sandbox")
        );
    }
}
