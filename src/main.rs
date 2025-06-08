use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_GODO_DIR: &str = "~/.godo";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Override the godo directory location
    #[arg(long, global = true, value_name = "DIR")]
    dir: Option<String>,

    /// Override the repository directory (defaults to current git project)
    #[arg(long, global = true, value_name = "DIR")]
    repo_dir: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

struct RunOptions {
    persist: bool,
    copy: Vec<String>,
    name: String,
    command: Vec<String>,
}

struct Godo {
    godo_dir: PathBuf,
    repo_dir: PathBuf,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a command in an isolated workspace
    Run {
        /// Keep the sandbox after the command exits
        #[arg(long)]
        persist: bool,

        /// Copy only directories that match glob (can be specified multiple times)
        #[arg(long, value_name = "GLOB")]
        copy: Vec<String>,

        /// Name of the sandbox
        name: String,

        /// Command to execute (if omitted, opens interactive shell)
        command: Vec<String>,
    },

    /// Show existing sandboxes
    List,

    /// Delete a named sandbox
    Rm {
        /// Name of the sandbox to remove
        name: String,
    },

    /// Remove sandboxes whose branch no longer exists
    Prune,
}

fn expand_tilde(path: &str) -> PathBuf {
    if path.starts_with("~") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(path.replacen("~", &home, 1));
        }
    }
    PathBuf::from(path)
}

fn ensure_godo_directory(godo_dir: &Path) -> Result<()> {
    // Create main godo directory
    fs::create_dir_all(godo_dir)?;

    // Create sandboxes subdirectory
    let sandboxes_dir = godo_dir.join("sandboxes");
    fs::create_dir_all(sandboxes_dir)?;

    Ok(())
}

impl Godo {
    fn new(godo_dir: PathBuf, repo_dir: PathBuf) -> Result<Self> {
        // Ensure godo directory exists
        ensure_godo_directory(&godo_dir)?;

        Ok(Self { godo_dir, repo_dir })
    }

    fn run(&self, options: &RunOptions) -> Result<()> {
        println!("Running sandbox '{}' in {:?}", options.name, self.godo_dir);
        println!("Repository directory: {:?}", self.repo_dir);

        // TODO: Implement run functionality
        // 1. Check we're in a git repo
        // 2. Create worktree
        // 3. Copy resources
        // 4. Run command or shell
        // 5. Commit results
        // 6. Cleanup (unless --persist)

        Ok(())
    }

    fn list(&self) -> Result<()> {
        println!("Listing sandboxes in {:?}", self.godo_dir);

        // TODO: Implement list functionality
        // 1. Read sandboxes directory
        // 2. Check which sandboxes exist
        // 3. Show their status (running, persisted)

        Ok(())
    }

    fn remove(&self, name: &str) -> Result<()> {
        println!("Removing sandbox '{}' from {:?}", name, self.godo_dir);

        // TODO: Implement remove functionality
        // 1. Check if sandbox exists
        // 2. Remove git worktree
        // 3. Delete sandbox directory

        Ok(())
    }

    fn prune(&self) -> Result<()> {
        println!("Pruning sandboxes in {:?}", self.godo_dir);

        // TODO: Implement prune functionality
        // 1. List all sandboxes
        // 2. Check which branches still exist
        // 3. Remove sandboxes whose branches are gone

        Ok(())
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

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Determine godo directory (priority: CLI flag > env var > default)
    let godo_dir = if let Some(dir) = &cli.dir {
        expand_tilde(dir)
    } else if let Ok(env_dir) = std::env::var("GODO_DIR") {
        expand_tilde(&env_dir)
    } else {
        expand_tilde(DEFAULT_GODO_DIR)
    };

    // Determine repository directory
    let repo_dir = if let Some(repo) = &cli.repo_dir {
        expand_tilde(repo)
    } else {
        // Find git root from current directory
        let current_dir = std::env::current_dir().context("Failed to get current directory")?;

        find_git_root(&current_dir).context("Not in a git repository")?
    };

    // Create Godo instance
    let godo = Godo::new(godo_dir, repo_dir).context("Failed to initialize godo")?;

    match cli.command {
        Commands::Run {
            persist,
            copy,
            name,
            command,
        } => {
            let options = RunOptions {
                persist,
                copy,
                name,
                command,
            };

            godo.run(&options)?;
        }
        Commands::List => {
            godo.list()?;
        }
        Commands::Rm { name } => {
            godo.remove(&name)?;
        }
        Commands::Prune => {
            godo.prune()?;
        }
    }

    Ok(())
}
