use anyhow::{Context, Result};
use clonetree::{Options, clone_tree};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use crate::git;
use crate::output::Output;

enum PostRunAction {
    Commit,
    Shell,
    Keep,
}

/// Status information for a sandbox
struct Sandbox {
    /// The name of the sandbox
    name: String,
    /// Whether the branch exists
    has_branch: bool,
    /// Whether the worktree exists
    has_worktree: bool,
    /// Whether the worktree directory path exists
    has_worktree_dir: bool,
    /// Whether there are any staged or unstaged uncommitted changes in the worktree
    has_uncommitted_changes: bool,
    /// Whether the branch has commits that have not been merged yet
    has_unmerged_commits: bool,
    /// Whether the worktree is dangling (no backing directory)
    is_dangling: bool,
}

impl Sandbox {
    /// Returns true if the sandbox has both a worktree and a branch
    fn is_live(&self) -> bool {
        self.has_worktree && self.has_branch
    }

    /// Display the sandbox status using the provided output
    fn show(&self, output: &dyn Output) -> Result<()> {
        let mut attributes = Vec::new();

        if self.is_live() {
            attributes.push("live");
        } else if self.has_branch {
            attributes.push("branch only");
        }

        if self.has_uncommitted_changes {
            attributes.push("uncommitted changes");
        }

        if self.has_unmerged_commits {
            attributes.push("unmerged");
        }

        if self.is_dangling {
            attributes.push("dangling worktree");
        }

        let attributes_str = if attributes.is_empty() {
            String::new()
        } else {
            format!(" [{}]", attributes.join(", "))
        };

        output.message(&format!("{}{}", self.name, attributes_str))?;
        Ok(())
    }
}

/// Validates that a sandbox name contains only allowed characters (a-zA-Z0-9-_)
fn validate_sandbox_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("sandbox name cannot be empty");
    }

    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!(
            "Invalid sandbox name '{name}': names can only contain letters, numbers, hyphens, and underscores",
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

    /// Prompt the user for what to do after command execution
    fn prompt_for_action(&self, sandbox_path: &Path) -> Result<PostRunAction> {
        // Check if there are uncommitted changes
        let has_changes = git::has_uncommitted_changes(sandbox_path)?;

        let prompt = if has_changes {
            "Command completed. You have uncommitted changes. What would you like to do?"
        } else {
            "Command completed. What would you like to do?"
        };

        let options = vec![
            "commit - stage all changes and commit".to_string(),
            "shell - run a shell in the sandbox".to_string(),
            "keep - keep the sandbox and exit".to_string(),
        ];

        match self.output.select(prompt, options)? {
            0 => Ok(PostRunAction::Commit),
            1 => Ok(PostRunAction::Shell),
            2 => Ok(PostRunAction::Keep),
            _ => unreachable!("Invalid selection"),
        }
    }

    pub fn run(
        &self,
        keep: bool,
        excludes: &[String],
        sandbox_name: &str,
        command: &[String],
    ) -> Result<()> {
        // Validate sandbox name
        validate_sandbox_name(sandbox_name)?;

        let sandbox_path = self.sandbox_path(sandbox_name)?;

        let existing_sandbox = self.get_sandbox(sandbox_name)?;

        if let Some(sandbox) = existing_sandbox {
            // Sandbox exists, check if it's live
            if sandbox.is_live() {
                self.output.message(&format!(
                    "Using existing sandbox {sandbox_name} at {sandbox_path:?}"
                ))?;
            } else {
                anyhow::bail!("Sandbox '{sandbox_name}' exists but is not live - remove it first");
            }
        } else {
            // Sandbox doesn't exist, create it
            // Check for uncommitted changes
            if git::has_uncommitted_changes(&self.repo_dir)? {
                self.output.warn("You have uncommitted changes:")?;
                if !self.no_prompt && !self.output.confirm("Continue creating worktree?")? {
                    anyhow::bail!("Aborted by user");
                }
            }

            // Ensure project directory exists
            let project_dir = self.project_dir()?;
            fs::create_dir_all(&project_dir)?;

            let branch = branch_name(sandbox_name);
            self.output.message(&format!(
                "Creating sandbox {sandbox_name} with branch {branch} at {sandbox_path:?}"
            ))?;
            git::create_worktree(&self.repo_dir, &sandbox_path, &branch)?;

            self.output.message("Cloning tree to sandbox...")?;

            let mut clone_options = Options::new().overwrite(true).glob("!.git/**");

            // Add user-specified excludes with "!" prefix
            for exclude in excludes {
                clone_options = clone_options.glob(format!("!{exclude}"));
            }

            clone_tree(&self.repo_dir, &sandbox_path, &clone_options)
                .context("Failed to clone files to sandbox")?;
        }

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

        // Prompt user for action if not explicitly keeping
        if !keep {
            loop {
                let action = self.prompt_for_action(&sandbox_path)?;
                match action {
                    PostRunAction::Commit => {
                        self.output.message("Staging and committing changes...")?;
                        // Stage all changes
                        git::add_all(&sandbox_path)?;
                        // Commit with verbose flag
                        git::commit_interactive(&sandbox_path)?;
                        // Clean up after commit
                        self.cleanup_sandbox(sandbox_name)?;
                        break; // Exit the loop after commit
                    }
                    PostRunAction::Shell => {
                        self.output.message("Opening shell in sandbox...")?;
                        // Run interactive shell
                        let shell_status = Command::new(&shell)
                            .current_dir(&sandbox_path)
                            .status()
                            .context("Failed to start shell")?;

                        if !shell_status.success() {
                            self.output.warn("Shell exited with non-zero status")?;
                        }
                        // Continue the loop to show prompt again
                    }
                    PostRunAction::Keep => {
                        self.output.success(&format!(
                            "Keeping sandbox. You can return to it at: {}",
                            sandbox_path.display()
                        ))?;
                        break; // Exit the loop after keep
                    }
                }
            }
        }

        Ok(())
    }

    /// Get the status of a sandbox by name
    fn get_sandbox(&self, name: &str) -> Result<Option<Sandbox>> {
        let sandbox_path = self.sandbox_path(name)?;
        let branch_name = branch_name(name);

        // Check if branch exists
        let has_branch = git::has_branch(&self.repo_dir, &branch_name)?;

        // Get all worktrees to check if this sandbox has a worktree
        let worktrees = git::list_worktrees(&self.repo_dir)?;
        let has_worktree = worktrees.iter().any(|w| {
            let branch = w.branch.strip_prefix("refs/heads/").unwrap_or(&w.branch);
            branch == branch_name
        });

        // Check if the worktree directory exists
        let has_worktree_dir = sandbox_path.exists();

        // If neither branch, worktree, nor directory exists, the sandbox doesn't exist
        if !has_branch && !has_worktree && !has_worktree_dir {
            return Ok(None);
        }

        // Check for uncommitted changes (only if worktree exists)
        let has_uncommitted_changes = if has_worktree && has_worktree_dir {
            git::has_uncommitted_changes(&sandbox_path).unwrap_or(false)
        } else {
            false
        };

        // Check for unmerged commits (only if branch exists)
        let has_unmerged_commits = if has_branch {
            git::has_unmerged_commits(&self.repo_dir, &branch_name).unwrap_or(false)
        } else {
            false
        };

        // Check if dangling (directory exists but no branch)
        let is_dangling = has_worktree_dir && !has_branch;

        Ok(Some(Sandbox {
            name: name.to_string(),
            has_branch,
            has_worktree,
            has_worktree_dir,
            has_uncommitted_changes,
            has_unmerged_commits,
            is_dangling,
        }))
    }

    pub fn list(&self) -> Result<()> {
        let project_dir = self.project_dir()?;

        let mut all_names = std::collections::HashSet::new();

        let all_branches = git::list_branches(&self.repo_dir)?;
        for branch in &all_branches {
            if let Some(name) = branch.strip_prefix("godo/") {
                all_names.insert(name.to_string());
            }
        }

        for worktree in git::list_worktrees(&self.repo_dir)? {
            let branch = worktree
                .branch
                .strip_prefix("refs/heads/")
                .unwrap_or(&worktree.branch);
            if let Some(name) = branch.strip_prefix("godo/") {
                all_names.insert(name.to_string());
            }
        }

        if project_dir.exists() {
            for entry in fs::read_dir(&project_dir)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    let dir_name = entry.file_name().to_string_lossy().to_string();
                    all_names.insert(dir_name);
                }
            }
        }

        // Sort all names alphabetically
        let mut sorted_names: Vec<String> = all_names.into_iter().collect();
        sorted_names.sort();

        if sorted_names.is_empty() {
            self.output.message("No sandboxes found.")?;
            return Ok(());
        }

        // Display each sandbox with its attributes
        for name in sorted_names {
            match self.get_sandbox(&name)? {
                Some(status) => {
                    status.show(&*self.output)?;
                }
                None => {
                    // Skip sandboxes that don't exist
                    continue;
                }
            }
        }

        Ok(())
    }

    pub fn remove(&self, name: &str, force: bool) -> Result<()> {
        let status = match self.get_sandbox(name)? {
            Some(s) => s,
            None => anyhow::bail!("sandbox '{name}' does not exist"),
        };

        if !force {
            if status.has_uncommitted_changes
                && !self
                    .output
                    .confirm("Sandbox has uncommitted changes. Remove anyway?")?
            {
                anyhow::bail!("Aborted by user");
            }
            if status.has_unmerged_commits
                && !self
                    .output
                    .confirm("Branch has unmerged commits. Remove anyway?")?
            {
                anyhow::bail!("Aborted by user");
            }
        }

        self.output.message(&format!("Removing {name}"))?;
        self.remove_sandbox(name)
    }

    /// Clean up a sandbox by removing worktree if no uncommitted changes and branch if no unmerged commits
    fn cleanup_sandbox(&self, name: &str) -> Result<()> {
        let status = match self.get_sandbox(name)? {
            Some(s) => s,
            None => {
                self.output.message(&format!("no such sandbox: {name}"))?;
                return Ok(());
            }
        };

        self.output
            .message(&format!("cleaning up sandbox {name}..."))?;

        let sandbox_path = self.sandbox_path(name)?;
        let branch = branch_name(name);

        let mut worktree_removed = false;
        let mut branch_removed = false;

        // Remove the worktree if it exists and has no uncommitted changes
        if status.has_worktree && !status.has_uncommitted_changes {
            git::remove_worktree(&self.repo_dir, &sandbox_path, false)?;
            self.output.message("\tremoved unmodified worktree")?;
            worktree_removed = true;
        }

        // Clean up the directory if it still exists and worktree was removed
        if !status.has_worktree && status.has_worktree_dir {
            fs::remove_dir_all(&sandbox_path).context("Failed to remove sandbox directory")?;
        }

        // Only remove the branch if:
        // 1. It exists
        // 2. It has no unmerged commits
        // 3. We removed the worktree (or there was no worktree)
        if status.has_branch
            && !status.has_unmerged_commits
            && (worktree_removed || !status.has_worktree)
        {
            git::delete_branch(&self.repo_dir, &branch, false)?;
            branch_removed = true;
        }

        // Report what was done
        if worktree_removed && branch_removed {
            self.output
                .success("unmodified sandbox and branch cleaned up")?;
        } else if worktree_removed {
            self.output
                .success(&format!("worktree removed, branch {branch} kept"))?;
        } else {
            self.output.message("")?;
        }

        Ok(())
    }

    /// Remove a sandbox forcefully or fail if it has changes
    fn remove_sandbox(&self, name: &str) -> Result<()> {
        // Get sandbox status to check current state
        let status = match self.get_sandbox(name)? {
            Some(s) => s,
            None => anyhow::bail!("sandbox '{name}' does not exist"),
        };

        let sandbox_path = self.sandbox_path(name)?;
        let branch = branch_name(name);

        if status.has_worktree {
            git::remove_worktree(&self.repo_dir, &sandbox_path, true)?
        }
        if sandbox_path.exists() {
            fs::remove_dir_all(&sandbox_path).context("Failed to remove sandbox directory")?;
        }
        if status.has_branch {
            git::delete_branch(&self.repo_dir, &branch, true)?
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
    use crate::output::{Output, Quiet};
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

    // Mock output for testing prompt_for_action
    struct MockOutput {
        selection: usize,
    }

    impl Output for MockOutput {
        fn message(&self, _msg: &str) -> crate::output::Result<()> {
            Ok(())
        }

        fn success(&self, _msg: &str) -> crate::output::Result<()> {
            Ok(())
        }

        fn warn(&self, _msg: &str) -> crate::output::Result<()> {
            Ok(())
        }

        fn fail(&self, _msg: &str) -> crate::output::Result<()> {
            Ok(())
        }

        fn confirm(&self, _prompt: &str) -> crate::output::Result<bool> {
            Ok(true)
        }

        fn select(&self, _prompt: &str, _options: Vec<String>) -> crate::output::Result<usize> {
            Ok(self.selection)
        }

        fn finish(&self) -> crate::output::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn test_prompt_for_action() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let godo_dir = temp_dir.path().join(".godo");
        let repo_dir = temp_dir.path().join("repo");

        // Initialize a git repository
        std::fs::create_dir_all(&repo_dir).unwrap();
        std::process::Command::new("git")
            .current_dir(&repo_dir)
            .args(["init"])
            .output()
            .unwrap();

        // Test selecting "Commit"
        let output: Arc<dyn Output> = Arc::new(MockOutput { selection: 0 });
        let godo = Godo::new(godo_dir.clone(), Some(repo_dir.clone()), output, false).unwrap();
        let action = godo.prompt_for_action(&repo_dir).unwrap();
        assert!(matches!(action, PostRunAction::Commit));

        // Test selecting "Shell"
        let output: Arc<dyn Output> = Arc::new(MockOutput { selection: 1 });
        let godo = Godo::new(godo_dir.clone(), Some(repo_dir.clone()), output, false).unwrap();
        let action = godo.prompt_for_action(&repo_dir).unwrap();
        assert!(matches!(action, PostRunAction::Shell));

        // Test selecting "Keep"
        let output: Arc<dyn Output> = Arc::new(MockOutput { selection: 2 });
        let godo = Godo::new(godo_dir, Some(repo_dir.clone()), output, false).unwrap();
        let action = godo.prompt_for_action(&repo_dir).unwrap();
        assert!(matches!(action, PostRunAction::Keep));
    }
}
