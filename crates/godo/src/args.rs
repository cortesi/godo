use clap::{ArgGroup, Parser, Subcommand};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(group(
    ArgGroup::new("color_mode")
        .args(["color", "no_color"])
))]
/// Top-level CLI options for godo.
pub struct Cli {
    /// Override the godo directory location
    #[arg(long, global = true, value_name = "DIR")]
    pub dir: Option<String>,

    /// Override the repository directory (defaults to current git project)
    #[arg(long, global = true, value_name = "DIR")]
    pub repo_dir: Option<String>,

    /// Enable colored output
    #[arg(long, global = true)]
    pub color: bool,

    /// Disable colored output
    #[arg(long = "no-color", global = true)]
    pub no_color: bool,

    /// Suppress all output
    #[arg(long, global = true)]
    pub quiet: bool,

    /// Skip confirmation prompts
    #[arg(long, global = true)]
    pub no_prompt: bool,

    #[command(subcommand)]
    /// The primary command to execute.
    pub command: Commands,
}

#[derive(Subcommand)]
/// CLI subcommands supported by godo.
pub enum Commands {
    /// Run a command in an isolated workspace
    Run {
        /// Keep the sandbox after the command exits
        #[arg(long)]
        keep: bool,

        /// Automatically commit all changes with the specified message after command exits
        #[arg(long)]
        commit: Option<String>,

        /// Force shell evaluation with $SHELL -c
        #[arg(long = "sh")]
        sh: bool,

        /// Exclude directories that match glob (can be specified multiple times)
        #[arg(long = "exclude", value_name = "GLOB")]
        excludes: Vec<String>,

        /// Name of the sandbox
        name: String,

        /// Command to execute (if omitted, opens interactive shell)
        command: Vec<String>,
    },

    /// Show existing sandboxes
    #[command(alias = "ls")]
    List,

    /// Diff a sandbox against its recorded base commit
    Diff {
        /// Name of the sandbox to diff (auto-detected if running from within a sandbox)
        name: Option<String>,

        /// Override the base commit used for diffing
        #[arg(long, value_name = "COMMIT")]
        base: Option<String>,

        /// Override the pager command for diff output
        #[arg(long, value_name = "CMD", conflicts_with = "no_pager")]
        pager: Option<String>,

        /// Disable paging for diff output
        #[arg(long = "no-pager", conflicts_with = "pager")]
        no_pager: bool,
    },

    /// Delete a named sandbox
    #[command(alias = "rm")]
    Remove {
        /// Name of the sandbox to remove
        name: String,

        /// Force removal even if there are uncommitted changes
        #[arg(long)]
        force: bool,
    },

    /// Clean up a sandbox by removing worktree but keeping the branch
    Clean {
        /// Name of the sandbox to clean (if not specified, cleans all sandboxes)
        name: Option<String>,
    },
}

/// Parameters for the `godo run` command.
pub struct RunRequest {
    /// Keep the sandbox after command completion.
    pub keep: bool,
    /// Optional commit message for automatic commit.
    pub commit: Option<String>,
    /// Force shell execution.
    pub force_shell: bool,
    /// Directory exclusions to apply when cloning.
    pub excludes: Vec<String>,
    /// Name of the sandbox to operate on.
    pub sandbox_name: String,
    /// Command to execute inside the sandbox.
    pub command: Vec<String>,
}
