use std::{
    fs,
    fs::OpenOptions,
    io,
    path::{Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use fs4::FileExt;
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};

use crate::GodoError;

/// Directory name used to store sandbox lease files.
pub const LEASE_DIR_NAME: &str = ".godo-leases";

/// Track active sessions per sandbox using lightweight lease files.
#[derive(Clone)]
pub struct SessionManager {
    /// Directory where lease files are stored.
    base_dir: PathBuf,
}

/// RAII lease for a sandbox session.
#[derive(Debug)]
pub struct SessionLease {
    /// Path to the lease file.
    lease_path: PathBuf,
    /// Path to the lock file.
    lock_path: PathBuf,
    /// Name of the sandbox.
    sandbox: String,
    /// Base directory for leases.
    base_dir: PathBuf,
}

/// A locked handle to the sandbox lease, allowing exclusive operations during setup.
pub struct LockedSandbox {
    /// The lock file handle.
    lock_file: fs::File,
    /// Directory where leases are stored.
    lease_dir: PathBuf,
    /// Path to the lock file.
    lock_path: PathBuf,
    /// Name of the sandbox.
    sandbox: String,
    /// Base directory for leases.
    base_dir: PathBuf,
}

impl Drop for LockedSandbox {
    #[allow(clippy::let_underscore_must_use)]
    fn drop(&mut self) {
        let _ = self.lock_file.unlock();
    }
}

/// Result of releasing a lease.
pub enum ReleaseOutcome {
    /// No other leases remain; caller is responsible for cleanup while holding the lock.
    Last(CleanupGuard),
    /// Other leases remain; skip cleanup.
    NotLast,
}

/// Holds the sandbox lock so new sessions cannot attach during cleanup.
pub struct CleanupGuard {
    /// The lock file handle.
    lock_file: fs::File,
    /// Directory where leases are stored.
    lease_dir: PathBuf,
}

impl Drop for CleanupGuard {
    #[allow(clippy::let_underscore_must_use)]
    fn drop(&mut self) {
        let _ = self.lock_file.unlock();
        // Best-effort cleanup of the lease directory when no sessions remain.
        let _ = fs::remove_dir(&self.lease_dir);
    }
}

impl SessionManager {
    /// Create a new session manager for the given project directory.
    pub fn new(project_dir: &Path) -> Self {
        Self {
            base_dir: project_dir.join(LEASE_DIR_NAME),
        }
    }

    /// Get the directory for a specific sandbox's leases.
    fn lease_dir(&self, sandbox: &str) -> PathBuf {
        self.base_dir.join(sandbox)
    }

    /// Get the path to the lock file for a specific sandbox.
    fn lock_path(&self, sandbox: &str) -> PathBuf {
        self.lease_dir(sandbox).join("lease.lock")
    }

    /// Count active leases for a sandbox (stale PIDs are pruned first).
    pub fn active_connections(&self, sandbox: &str) -> Result<usize, GodoError> {
        let lease_dir = self.lease_dir(sandbox);
        if !lease_dir.exists() {
            return Ok(0);
        }

        // Shared lock to avoid racing with writers.
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.lock_path(sandbox))
            .map_err(map_io)?;

        lock_file.lock_shared().map_err(map_io)?;
        prune_stale_leases(&lease_dir)?;
        let count = lease_files(&lease_dir)?.len();
        lock_file.unlock().map_err(map_io)?;
        Ok(count)
    }

    /// Acquire an exclusive lock on the sandbox configuration.
    /// This should be held during creation/setup to prevent races.
    pub fn lock(&self, sandbox: &str) -> Result<LockedSandbox, GodoError> {
        let lease_dir = self.lease_dir(sandbox);
        fs::create_dir_all(&lease_dir).map_err(map_io)?;

        let lock_path = self.lock_path(sandbox);
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(map_io)?;

        lock_file.lock_exclusive().map_err(map_io)?;

        Ok(LockedSandbox {
            lock_file,
            lease_dir,
            lock_path,
            sandbox: sandbox.to_string(),
            base_dir: self.base_dir.clone(),
        })
    }
}

impl LockedSandbox {
    /// Convert the lock into a registered session lease.
    pub fn acquire_lease(self) -> Result<SessionLease, GodoError> {
        prune_stale_leases(&self.lease_dir)?;

        let pid = process::id();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let lease_name = format!("lease-{pid}-{nonce}.pid");
        let lease_path = self.lease_dir.join(lease_name);

        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lease_path)
            .map_err(map_io)?;

        Ok(SessionLease {
            lease_path,
            lock_path: self.lock_path.clone(),
            sandbox: self.sandbox.clone(),
            base_dir: self.base_dir.clone(),
        })
    }
}

impl SessionLease {
    /// Release the lease and report whether this was the last active connection.
    pub fn release(self) -> Result<ReleaseOutcome, GodoError> {
        let lease_dir = self.base_dir.join(&self.sandbox);
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.lock_path)
            .map_err(map_io)?;

        lock_file.lock_exclusive().map_err(map_io)?;

        // Remove our lease first.
        #[allow(clippy::let_underscore_must_use)]
        {
            let _ = fs::remove_file(&self.lease_path);
        }

        prune_stale_leases(&lease_dir)?;
        let remaining = lease_files(&lease_dir)?.len();

        if remaining == 0 {
            Ok(ReleaseOutcome::Last(CleanupGuard {
                lock_file,
                lease_dir,
            }))
        } else {
            lock_file.unlock().map_err(map_io)?;
            Ok(ReleaseOutcome::NotLast)
        }
    }
}

impl Drop for SessionLease {
    #[allow(clippy::let_underscore_must_use)]
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lease_path);
    }
}

/// Prune lease files corresponding to dead processes.
fn prune_stale_leases(dir: &Path) -> Result<(), GodoError> {
    let mut sys =
        System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()));

    for lease in lease_files(dir)? {
        if let Some(pid) = parse_pid(&lease) {
            sys.refresh_process(pid);
            if sys.process(pid).is_some() {
                continue;
            }
        }
        #[allow(clippy::let_underscore_must_use)]
        {
            let _ = fs::remove_file(lease);
        }
    }

    Ok(())
}

/// List all lease files in the given directory.
fn lease_files(dir: &Path) -> Result<Vec<PathBuf>, GodoError> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).map_err(map_io)? {
        let entry = entry.map_err(map_io)?;
        let path = entry.path();
        if path.is_file()
            && path
                .file_name()
                .map(|n| n.to_string_lossy().starts_with("lease-"))
                .unwrap_or(false)
        {
            files.push(path);
        }
    }
    Ok(files)
}

/// Extract PID from a lease file name.
fn parse_pid(path: &Path) -> Option<Pid> {
    let name = path.file_name()?.to_string_lossy();
    let pid_part = name.split('-').nth(1)?;
    pid_part.parse::<u32>().ok().map(Pid::from_u32)
}

/// Map an IO error to a GodoError.
#[allow(clippy::needless_pass_by_value)]
fn map_io(err: io::Error) -> GodoError {
    GodoError::OperationError(format!("IO error: {err}"))
}
