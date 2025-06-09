use anyhow::{Context, Result};
use clonetree::{Options, clone_tree};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use termcolor::{ColorChoice, StandardStream, WriteColor};

pub struct RunOptions {
    #[allow(dead_code)]
    pub persist: bool,
    #[allow(dead_code)]
    pub copy: Vec<String>,
    pub name: String,
    pub command: Vec<String>,
}

pub struct Godo {
    godo_dir: PathBuf,
    repo_dir: PathBuf,
    color: bool,
    quiet: bool,
}

impl Godo {
    pub fn new(godo_dir: PathBuf, repo_dir: PathBuf, color: bool, quiet: bool) -> Result<Self> {
        // Ensure godo directory exists
        ensure_godo_directory(&godo_dir)?;

        Ok(Self {
            godo_dir,
            repo_dir,
            color,
            quiet,
        })
    }

    pub fn run(&self, options: &RunOptions) -> Result<()> {
        let sandbox_path = self.godo_dir.join("sandboxes").join(&options.name);

        // Step 1: Check we're in a git repo (already done in main.rs)

        // Step 2: Create worktree
        if !self.quiet {
            println!(
                "Creating worktree for '{}' at {:?}",
                options.name, sandbox_path
            );
        }

        let output = Command::new("git")
            .current_dir(&self.repo_dir)
            .args(["worktree", "add", "--quiet"])
            .arg(&sandbox_path)
            .arg("HEAD")
            .output()
            .context("Failed to create git worktree")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to create worktree: {}", stderr);
        }

        // Step 3: Copy resources using clonetree
        if !self.quiet {
            println!("Copying untracked files to sandbox...");
        }

        let clone_options = Options::new().overwrite(true).glob("!.git/**");

        clone_tree(&self.repo_dir, &sandbox_path, &clone_options)
            .context("Failed to copy files to sandbox")?;

        // Step 4: Run command or shell
        if !self.quiet {
            println!("Running in sandbox: {sandbox_path:?}");
        }

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());

        let status = if options.command.is_empty() {
            // Interactive shell
            Command::new(&shell)
                .current_dir(&sandbox_path)
                .status()
                .context("Failed to start shell")?
        } else {
            // Run the specified command
            let command_string = options.command.join(" ");
            Command::new(&shell)
                .arg("-c")
                .arg(&command_string)
                .current_dir(&sandbox_path)
                .status()
                .context("Failed to run command")?
        };

        if !status.success() && !self.quiet {
            println!("Command exited with status: {status}");
        }

        // TODO: Step 5: Commit results
        // TODO: Step 6: Cleanup (unless --persist)

        Ok(())
    }

    pub fn list(&self) -> Result<()> {
        use std::io::Write;
        let mut stdout = self.stdout();
        writeln!(stdout, "Listing sandboxes in {:?}", self.godo_dir)?;

        // TODO: Implement list functionality
        // 1. Read sandboxes directory
        // 2. Check which sandboxes exist
        // 3. Show their status (running, persisted)

        Ok(())
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        println!("Removing sandbox '{}' from {:?}", name, self.godo_dir);

        // TODO: Implement remove functionality
        // 1. Check if sandbox exists
        // 2. Remove git worktree
        // 3. Delete sandbox directory

        Ok(())
    }

    pub fn prune(&self) -> Result<()> {
        println!("Pruning sandboxes in {:?}", self.godo_dir);

        // TODO: Implement prune functionality
        // 1. List all sandboxes
        // 2. Check which branches still exist
        // 3. Remove sandboxes whose branches are gone

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
