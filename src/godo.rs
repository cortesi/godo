use anyhow::{Context, Result};
use clonetree::{Options, clone_tree};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

use crate::git;

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
                .last()
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
    format!("godo/{}", sandbox_name)
}

macro_rules! out {
    ($self:expr, $($arg:tt)*) => {{
        use std::io::Write;
        let mut stdout = $self.stdout();
        write!(stdout, $($arg)*)?;
    }};
}

macro_rules! outlnc {
    ($self:expr, $color:expr, $($arg:tt)*) => {{
        use std::io::Write;
        let mut stdout = $self.stdout();
        stdout.set_color(ColorSpec::new().set_fg(Some($color)))?;
        writeln!(stdout, $($arg)*)?;
        stdout.reset()?;
    }};
}

pub struct Godo {
    godo_dir: PathBuf,
    repo_dir: PathBuf,
    color: bool,
    quiet: bool,
    no_prompt: bool,
}

impl Godo {
    pub fn new(
        godo_dir: PathBuf,
        repo_dir: Option<PathBuf>,
        color: bool,
        quiet: bool,
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
            color,
            quiet,
            no_prompt,
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
        excludes: &[String],
        sandbox_name: &str,
        command: &[String],
    ) -> Result<()> {
        // Validate sandbox name
        validate_sandbox_name(sandbox_name)?;

        // Check for uncommitted changes
        if git::has_uncommitted_changes(&self.repo_dir)? {
            outlnc!(
                self,
                Color::Yellow,
                "Warning: You have uncommitted changes:"
            );
            if !self.no_prompt {
                out!(self, "Continue creating worktree? [y/N] ");
                io::stdout().flush()?;

                let mut response = String::new();
                io::stdin().read_line(&mut response)?;

                if !response.trim().to_lowercase().starts_with('y') {
                    outlnc!(self, Color::Red, "Aborted.");
                    return Ok(());
                }
            }
        }

        let sandbox_path = self.sandbox_path(sandbox_name)?;

        // Ensure project directory exists
        let project_dir = self.project_dir()?;
        fs::create_dir_all(&project_dir)?;

        outlnc!(
            self,
            Color::Cyan,
            "Creating sandbox {} with branch {} at {:?}",
            sandbox_name,
            branch_name(sandbox_name),
            sandbox_path
        );

        let branch = branch_name(sandbox_name);
        git::create_worktree(&self.repo_dir, &sandbox_path, &branch)?;

        outlnc!(self, Color::Cyan, "Copying files to sandbox...");

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
            outlnc!(self, Color::Red, "Command exited with status: {status}");
        }

        // Clean up if not keeping
        if !keep {
            self.cleanup_sandbox(sandbox_name, false)?;
        }

        Ok(())
    }

    pub fn list(&self) -> Result<()> {
        let project = project_name(&self.repo_dir)?;
        let project_dir = self.project_dir()?;

        outlnc!(
            self,
            Color::Cyan,
            "Listing sandboxes for project '{}' in {:?}",
            project,
            project_dir
        );

        // TODO: Implement list functionality
        // 1. Read sandboxes directory
        // 2. Check which sandboxes exist
        // 3. Show their status (running, kept)

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
            outlnc!(
                self,
                Color::Yellow,
                "Warning: Sandbox '{}' has uncommitted changes",
                name
            );

            if !self.no_prompt {
                out!(self, "Continue with removal? [y/N] ");
                io::stdout().flush()?;

                let mut response = String::new();
                io::stdin().read_line(&mut response)?;

                if !response.trim().to_lowercase().starts_with('y') {
                    outlnc!(self, Color::Red, "Aborted.");
                    return Ok(());
                }
            }
        }

        outlnc!(
            self,
            Color::Yellow,
            "Removing sandbox {} from {:?}",
            name,
            self.godo_dir
        );

        self.cleanup_sandbox(name, force)
    }

    pub fn prune(&self) -> Result<()> {
        let project = project_name(&self.repo_dir)?;
        let project_dir = self.project_dir()?;

        outlnc!(
            self,
            Color::Yellow,
            "Pruning sandboxes for project '{}' in {:?}",
            project,
            project_dir
        );

        // TODO: Implement prune functionality
        // 1. List all sandboxes
        // 2. Check which branches still exist
        // 3. Remove sandboxes whose branches are gone

        Ok(())
    }

    /// Clean up a sandbox by removing worktree and optionally the branch
    fn cleanup_sandbox(&self, name: &str, force: bool) -> Result<()> {
        outlnc!(self, Color::Cyan, "Cleaning up sandbox {}...", name);

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
                outlnc!(
                    self,
                    Color::Yellow,
                    "Cannot remove worktree: {}",
                    error_msg.trim()
                );
                if !force && error_msg.contains("uncommitted") {
                    outlnc!(
                        self,
                        Color::Yellow,
                        "Worktree has uncommitted changes. Use 'godo rm {} --force' to force removal.",
                        name
                    );
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
                    outlnc!(
                        self,
                        Color::Green,
                        "Sandbox and branch cleaned up successfully."
                    );
                }
                Err(e) => {
                    outlnc!(
                        self,
                        Color::Yellow,
                        "Warning: Failed to delete branch: {}",
                        e.to_string().trim()
                    );
                }
            }
        } else {
            outlnc!(
                self,
                Color::Green,
                "Sandbox removed. Branch {} kept (has commits).",
                branch
            );
        }

        Ok(())
    }

    /// Create a writer for stdout that respects quiet and color settings
    fn stdout(&self) -> Box<dyn WriteColor> {
        if self.quiet {
            Box::new(NoopWriter)
        } else {
            let color_choice = if self.color {
                ColorChoice::Always
            } else {
                ColorChoice::Never
            };

            Box::new(StandardStream::stdout(color_choice))
        }
    }
}

fn ensure_godo_directory(godo_dir: &Path) -> Result<()> {
    // Create main godo directory
    fs::create_dir_all(godo_dir)?;
    Ok(())
}

/// A no-op writer that discards all output
struct NoopWriter;

impl Write for NoopWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl WriteColor for NoopWriter {
    fn supports_color(&self) -> bool {
        false
    }

    fn set_color(&mut self, _spec: &termcolor::ColorSpec) -> io::Result<()> {
        Ok(())
    }

    fn reset(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_clean_name() {
        // Test valid characters
        assert_eq!(clean_name("valid-name"), "valid-name");
        assert_eq!(clean_name("valid_name"), "valid_name");
        assert_eq!(clean_name("ValidName123"), "ValidName123");
        assert_eq!(clean_name("a-b_c123"), "a-b_c123");

        // Test special characters replacement
        assert_eq!(clean_name("my.project"), "my-project");
        assert_eq!(clean_name("my project"), "my-project");
        assert_eq!(clean_name("my@project!"), "my-project-");
        assert_eq!(clean_name("my/project"), "my-project");
        assert_eq!(clean_name("my\\project"), "my-project");
        assert_eq!(clean_name("my:project"), "my-project");
        assert_eq!(clean_name("my*project"), "my-project");
        assert_eq!(clean_name("my?project"), "my-project");
        assert_eq!(clean_name("my\"project"), "my-project");
        assert_eq!(clean_name("my<project>"), "my-project-");
        assert_eq!(clean_name("my|project"), "my-project");

        // Test Unicode and non-ASCII characters
        assert_eq!(clean_name("café"), "caf-");
        assert_eq!(clean_name("项目"), "--");
        assert_eq!(clean_name("проект"), "------");
        assert_eq!(clean_name("🚀rocket"), "-rocket");

        // Test multiple consecutive special characters
        assert_eq!(clean_name("my...project"), "my---project");
        assert_eq!(clean_name("my   project"), "my---project");
        assert_eq!(clean_name("my@#$%project"), "my----project");

        // Test edge cases
        assert_eq!(clean_name(""), "");
        assert_eq!(clean_name("-"), "-");
        assert_eq!(clean_name("_"), "_");
        assert_eq!(clean_name("---"), "---");
        assert_eq!(clean_name("@#$%"), "----");

        // Test beginning and ending special characters
        assert_eq!(clean_name(".project"), "-project");
        assert_eq!(clean_name("project."), "project-");
        assert_eq!(clean_name(".project."), "-project-");

        // Test real-world examples
        assert_eq!(clean_name("my-awesome-project"), "my-awesome-project");
        assert_eq!(clean_name("MyCompany.Project"), "MyCompany-Project");
        assert_eq!(clean_name("2024-project"), "2024-project");
        assert_eq!(clean_name("project (copy)"), "project--copy-");
        assert_eq!(clean_name("project [v2]"), "project--v2-");
    }

    #[test]
    fn test_project_name() {
        // Test normal project names
        assert_eq!(
            project_name(&PathBuf::from("/home/user/projects/my-project")).unwrap(),
            "my-project"
        );
        assert_eq!(
            project_name(&PathBuf::from("/home/user/projects/my_project")).unwrap(),
            "my_project"
        );
        assert_eq!(
            project_name(&PathBuf::from("/home/user/projects/MyProject123")).unwrap(),
            "MyProject123"
        );

        // Test names with special characters that get replaced
        assert_eq!(
            project_name(&PathBuf::from("/home/user/projects/my.project")).unwrap(),
            "my-project"
        );
        assert_eq!(
            project_name(&PathBuf::from("/home/user/projects/my project")).unwrap(),
            "my-project"
        );
        assert_eq!(
            project_name(&PathBuf::from("/home/user/projects/my@project!")).unwrap(),
            "my-project-"
        );

        // Test edge cases
        assert_eq!(project_name(&PathBuf::from("/")).unwrap(), "root");
        assert_eq!(project_name(&PathBuf::from("project")).unwrap(), "project");

        // Test paths that would result in empty cleaned names
        // Note: "..." becomes "---" which is not empty, so it should succeed
        assert_eq!(
            project_name(&PathBuf::from("/home/user/projects/...")).unwrap(),
            "---"
        );

        // Test that a path with only special chars still produces a valid name
        assert_eq!(
            project_name(&PathBuf::from("/home/user/projects/@#$%^&*()")).unwrap(),
            "---------" // 9 special chars become 9 hyphens
        );
    }

    #[test]
    fn test_validate_sandbox_name() {
        // Valid names
        assert!(validate_sandbox_name("test").is_ok());
        assert!(validate_sandbox_name("test-123").is_ok());
        assert!(validate_sandbox_name("test_123").is_ok());
        assert!(validate_sandbox_name("TEST").is_ok());
        assert!(validate_sandbox_name("a1b2c3").is_ok());

        // Invalid names
        assert!(validate_sandbox_name("").is_err());
        assert!(validate_sandbox_name("test space").is_err());
        assert!(validate_sandbox_name("test.dot").is_err());
        assert!(validate_sandbox_name("test@symbol").is_err());
        assert!(validate_sandbox_name("test/slash").is_err());
    }

    #[test]
    fn test_branch_name() {
        assert_eq!(branch_name("test"), "godo/test");
        assert_eq!(branch_name("feature-123"), "godo/feature-123");
        assert_eq!(branch_name("my_sandbox"), "godo/my_sandbox");
    }

    #[test]
    fn test_sandbox_and_project_paths() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let godo_dir = temp_dir.path().join(".godo");
        let repo_dir = PathBuf::from("/home/user/projects/my-project");

        // Create a Godo instance
        let godo = Godo::new(godo_dir.clone(), Some(repo_dir), false, false, false).unwrap();

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
