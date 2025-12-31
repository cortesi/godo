use std::{io, path::PathBuf, result::Result as StdResult};
use thiserror::Error;

/// Custom Result type for Godo operations.
pub type Result<T> = StdResult<T, GodoError>;

/// Godo-specific error types
#[derive(Error, Debug)]
pub enum GodoError {
    /// A command executed inside the sandbox exited with a non-zero status.
    #[error("Command exited with status code: {code}")]
    CommandExit {
        /// The process exit status code.
        code: i32,
    },

    /// The requested sandbox operation failed due to an invalid state.
    #[error("Sandbox error: {message}")]
    SandboxError {
        /// Name of the sandbox associated with the failure.
        name: String,
        /// Human-readable error description.
        message: String,
    },

    /// The operation was cancelled by the user.
    #[error("Aborted by user")]
    UserAborted,

    /// A contextual precondition failed (e.g. not inside a Git repo).
    #[error("Context error: {0}")]
    ContextError(String),

    /// A high-level operation failed.
    #[error("Operation failed: {0}")]
    OperationError(String),

    /// A git command failed.
    #[error("Git error: {0}")]
    GitError(String),

    /// Base commit resolution failed for a sandbox.
    #[error("Base commit error for sandbox '{name}': {message}")]
    BaseError {
        /// Name of the sandbox associated with the failure.
        name: String,
        /// Human-readable error description.
        message: String,
    },
    /// The repository has uncommitted changes and the selected policy forbids proceeding.
    #[error("Uncommitted changes present in repository: {repo_dir}")]
    UncommittedChanges {
        /// Root of the repository with uncommitted changes.
        repo_dir: PathBuf,
    },

    /// An underlying I/O operation failed.
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),
}

impl GodoError {
    /// Return the recommended process exit code for this error.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::CommandExit { code } => *code,
            Self::UserAborted => 130,
            Self::SandboxError { .. } => 2,
            Self::UncommittedChanges { .. } => 2,
            Self::BaseError { .. } => 3,
            Self::GitError(_) => 4,
            _ => 1,
        }
    }
}
