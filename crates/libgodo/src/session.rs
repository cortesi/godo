use std::{
    fs,
    fs::OpenOptions,
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use fs4::FileExt;
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};

use crate::GodoError;

/// Track active sessions per sandbox using lightweight lease files.
#[derive(Clone)]
pub struct SessionManager {
    base_dir: PathBuf,
}

/// RAII lease for a sandbox session.
pub struct SessionLease {
    lease_path: PathBuf,
    lock_path: PathBuf,
    sandbox: String,
    base_dir: PathBuf,
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
    lock_file: fs::File,
    lease_dir: PathBuf,
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        let _ = self.lock_file.unlock();
        // Best-effort cleanup of the lease directory when no sessions remain.
        let _ = fs::remove_dir(&self.lease_dir);
    }
}

impl SessionManager {
    pub fn new(project_dir: PathBuf) -> Self {
        Self {
            base_dir: project_dir.join(".godo-leases"),
        }
    }

    fn lease_dir(&self, sandbox: &str) -> PathBuf {
        self.base_dir.join(sandbox)
    }

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
            .open(self.lock_path(sandbox))
            .map_err(map_io)?;

        lock_file.lock_shared().map_err(map_io)?;
        prune_stale_leases(&lease_dir)?;
        let count = lease_files(&lease_dir)?.len();
        lock_file.unlock().map_err(map_io)?;
        Ok(count)
    }

    /// Acquire a new lease for `sandbox`, returning a handle that can be released.
    pub fn acquire(&self, sandbox: &str) -> Result<SessionLease, GodoError> {
        let lease_dir = self.lease_dir(sandbox);
        fs::create_dir_all(&lease_dir).map_err(map_io)?;

        let lock_path = self.lock_path(sandbox);
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&lock_path)
            .map_err(map_io)?;

        lock_file.lock_exclusive().map_err(map_io)?;

        prune_stale_leases(&lease_dir)?;

        let pid = std::process::id();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let lease_name = format!("lease-{pid}-{nonce}.pid");
        let lease_path = lease_dir.join(lease_name);

        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lease_path)
            .map_err(map_io)?;

        lock_file.unlock().map_err(map_io)?;

        Ok(SessionLease {
            lease_path,
            lock_path,
            sandbox: sandbox.to_string(),
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
            .open(&self.lock_path)
            .map_err(map_io)?;

        lock_file.lock_exclusive().map_err(map_io)?;

        // Remove our lease first.
        let _ = fs::remove_file(&self.lease_path);

        prune_stale_leases(&lease_dir)?;
        let remaining = lease_files(&lease_dir)?.len();

        if remaining == 0 {
            Ok(ReleaseOutcome::Last(CleanupGuard { lock_file, lease_dir }))
        } else {
            lock_file.unlock().map_err(map_io)?;
            Ok(ReleaseOutcome::NotLast)
        }
    }
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lease_path);
    }
}

fn prune_stale_leases(dir: &Path) -> Result<(), GodoError> {
    let mut sys = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::new()),
    );

    for lease in lease_files(dir)? {
        if let Some(pid) = parse_pid(&lease) {
            sys.refresh_process(pid);
            if sys.process(pid).is_some() {
                continue;
            }
        }
        let _ = fs::remove_file(lease);
    }

    Ok(())
}

fn lease_files(dir: &Path) -> Result<Vec<PathBuf>, GodoError> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).map_err(map_io)? {
        let entry = entry.map_err(map_io)?;
        let path = entry.path();
        if path.is_file() && path.file_name().map(|n| n.to_string_lossy().starts_with("lease-")).unwrap_or(false) {
            files.push(path);
        }
    }
    Ok(files)
}

fn parse_pid(path: &Path) -> Option<Pid> {
    let name = path.file_name()?.to_string_lossy();
    let pid_part = name.split('-').nth(1)?;
    pid_part.parse::<u32>().ok().map(Pid::from_u32)
}

fn map_io(err: io::Error) -> GodoError {
    GodoError::OperationError(format!("IO error: {err}"))
}
