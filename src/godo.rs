use anyhow::{Context, Result};
use clonetree::{Options, clone_tree};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

use crate::git;

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
            find_git_root(&current_dir).context("Not in a git repository")?
        };

        Ok(Self {
            godo_dir,
            repo_dir,
            color,
            quiet,
            no_prompt,
        })
    }

    pub fn run(
        &self,
        keep: bool,
        excludes: &[String],
        name: &str,
        command: &[String],
    ) -> Result<()> {
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

        let sandbox_path = self.godo_dir.join(name);
        outlnc!(
            self,
            Color::Cyan,
            "Creating sandbox {} with branch godo/{} at {:?}",
            name,
            name,
            sandbox_path
        );

        let branch_name = format!("godo/{name}");
        git::create_worktree(&self.repo_dir, &sandbox_path, &branch_name)?;

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
            self.cleanup_sandbox(name, false)?;
        }

        Ok(())
    }

    pub fn list(&self) -> Result<()> {
        outlnc!(
            self,
            Color::Cyan,
            "Listing sandboxes in {:?}",
            self.godo_dir
        );

        // TODO: Implement list functionality
        // 1. Read sandboxes directory
        // 2. Check which sandboxes exist
        // 3. Show their status (running, kept)

        Ok(())
    }

    pub fn remove(&self, name: &str, force: bool) -> Result<()> {
        let sandbox_path = self.godo_dir.join(name);

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
        outlnc!(
            self,
            Color::Yellow,
            "Pruning sandboxes in {:?}",
            self.godo_dir
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

        let sandbox_path = self.godo_dir.join(name);
        let branch_name = format!("godo/{name}");

        // Get the initial commit of the branch to check if any commits were made
        let initial_commit = Command::new("git")
            .current_dir(&self.repo_dir)
            .args(["rev-parse", &branch_name])
            .output()
            .context("Failed to get branch commit")?;

        if !initial_commit.status.success() {
            outlnc!(
                self,
                Color::Yellow,
                "Branch {} not found, skipping branch cleanup",
                branch_name
            );
            // Still try to remove the worktree directory if it exists
            if sandbox_path.exists() {
                fs::remove_dir_all(&sandbox_path).context("Failed to remove sandbox directory")?;
            }
            return Ok(());
        }

        let initial_commit_sha = String::from_utf8_lossy(&initial_commit.stdout)
            .trim()
            .to_string();

        // Try to remove the worktree
        let sandbox_path_str = sandbox_path.to_string_lossy();
        let mut remove_args = vec!["worktree", "remove"];
        if force {
            remove_args.push("--force");
        }
        remove_args.push(&sandbox_path_str);

        let remove_output = Command::new("git")
            .current_dir(&self.repo_dir)
            .args(&remove_args)
            .output()
            .context("Failed to remove worktree")?;

        if !remove_output.status.success() {
            let stderr = String::from_utf8_lossy(&remove_output.stderr);
            outlnc!(
                self,
                Color::Yellow,
                "Cannot remove worktree: {}",
                stderr.trim()
            );
            if !force {
                outlnc!(
                    self,
                    Color::Yellow,
                    "Worktree has uncommitted changes. Use 'godo rm {} --force' to force removal.",
                    name
                );
            }
            return Ok(());
        }

        // Check if the branch has any new commits
        let current_commit = Command::new("git")
            .current_dir(&self.repo_dir)
            .args(["rev-parse", &branch_name])
            .output()
            .context("Failed to get current branch commit")?;

        let current_commit_sha = String::from_utf8_lossy(&current_commit.stdout)
            .trim()
            .to_string();

        if initial_commit_sha == current_commit_sha || force {
            // No new commits or forced deletion, delete the branch
            let mut delete_args = vec!["branch"];
            if force {
                delete_args.push("-D");
            } else {
                delete_args.push("-d");
            }
            delete_args.push(&branch_name);

            let delete_output = Command::new("git")
                .current_dir(&self.repo_dir)
                .args(&delete_args)
                .output()
                .context("Failed to delete branch")?;

            if !delete_output.status.success() {
                let stderr = String::from_utf8_lossy(&delete_output.stderr);
                outlnc!(
                    self,
                    Color::Yellow,
                    "Warning: Failed to delete branch: {}",
                    stderr.trim()
                );
            } else {
                outlnc!(
                    self,
                    Color::Green,
                    "Sandbox and branch cleaned up successfully."
                );
            }
        } else {
            outlnc!(
                self,
                Color::Green,
                "Sandbox removed. Branch {} kept (has commits).",
                branch_name
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

fn find_git_root(start_dir: &Path) -> Option<PathBuf> {
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

fn ensure_godo_directory(godo_dir: &Path) -> Result<()> {
    // Create main godo directory
    fs::create_dir_all(godo_dir)?;

    // Create sandboxes subdirectory
    let sandboxes_dir = godo_dir.join("sandboxes");
    fs::create_dir_all(sandboxes_dir)?;

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
