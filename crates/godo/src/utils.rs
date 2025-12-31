use anyhow::Result;
use std::{env, path::{Path, PathBuf}};

/// Expand a leading `~` in a filesystem path using the `HOME` environment variable.
pub fn expand_tilde(path: &str) -> PathBuf {
    if path.starts_with("~")
        && let Ok(home) = env::var("HOME")
    {
        return PathBuf::from(path.replacen("~", &home, 1));
    }
    PathBuf::from(path)
}

/// If running from within a godo sandbox, returns the sandbox name.
/// Returns None if not in a sandbox.
pub fn current_sandbox_name(godo_dir: &Path) -> Result<Option<String>> {
    let current_dir = env::current_dir()?;
    let canonical_godo = godo_dir
        .canonicalize()
        .unwrap_or_else(|_| godo_dir.to_path_buf());

    let canonical_cwd = match current_dir.canonicalize() {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };

    let relative = match canonical_cwd.strip_prefix(&canonical_godo) {
        Ok(rel) => rel,
        Err(_) => return Ok(None), // Not in godo dir
    };

    let components: Vec<_> = relative.components().collect();
    if components.len() < 2 {
        return Ok(None); // In godo dir but not in a sandbox
    }

    // components[0] = project, components[1] = sandbox
    let sandbox_name = components[1].as_os_str().to_string_lossy().to_string();

    // Filter out meta-directories
    if sandbox_name.starts_with(".godo-") {
        return Ok(None);
    }

    Ok(Some(sandbox_name))
}
