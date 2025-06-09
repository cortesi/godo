use anyhow::{Context, Result};
use clonetree::{Options, clone_tree};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

macro_rules! out {
    ($self:expr, $($arg:tt)*) => {{
        use std::io::Write;
        let mut stdout = $self.stdout();
        write!(stdout, $($arg)*)?;
    }};
}

macro_rules! outln {
    ($self:expr, $($arg:tt)*) => {{
        use std::io::Write;
        let mut stdout = $self.stdout();
        writeln!(stdout, $($arg)*)?;
    }};
}

macro_rules! outc {
    ($self:expr, $color:expr, $($arg:tt)*) => {{
        use std::io::Write;
        let mut stdout = $self.stdout();
        stdout.set_color(ColorSpec::new().set_fg(Some($color)))?;
        write!(stdout, $($arg)*)?;
        stdout.reset()?;
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
}

impl Godo {
    pub fn new(
        godo_dir: PathBuf,
        repo_dir: Option<PathBuf>,
        color: bool,
        quiet: bool,
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
        })
    }

    pub fn run(
        &self,
        persist: bool,
        excludes: &[String],
        name: &str,
        command: &[String],
    ) -> Result<()> {
        let sandbox_path = self.godo_dir.join(name);
        outlnc!(
            self,
            Color::Cyan,
            "Creating sandbox {} at {:?}",
            name,
            sandbox_path
        );

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
        // 3. Show their status (running, persisted)

        Ok(())
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        outlnc!(
            self,
            Color::Yellow,
            "Removing sandbox {} from {:?}",
            name,
            self.godo_dir
        );

        // TODO: Implement remove functionality
        // 1. Check if sandbox exists
        // 2. Remove git worktree
        // 3. Delete sandbox directory

        Ok(())
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
