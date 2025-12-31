use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use crate::types::SandboxMetadata;

/// Store for reading and writing sandbox metadata files.
pub struct SandboxMetadataStore {
    /// Directory containing metadata files for sandboxes.
    pub(crate) base_dir: PathBuf,
}

impl SandboxMetadataStore {
    /// Directory name for sandbox metadata within a godo project directory.
    pub const DIR_NAME: &'static str = ".godo-meta";

    /// Create a metadata store rooted at the provided project directory.
    pub fn new(project_dir: &Path) -> Self {
        Self {
            base_dir: project_dir.join(Self::DIR_NAME),
        }
    }

    /// Read metadata for a sandbox, returning `None` when no metadata exists.
    pub fn read(&self, sandbox: &str) -> Result<Option<SandboxMetadata>> {
        let path = self.metadata_path(sandbox);
        if !path.exists() {
            return Ok(None);
        }

        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read metadata file {}", path.display()))?;
        let metadata = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse metadata file {}", path.display()))?;
        Ok(Some(metadata))
    }

    /// Persist metadata for a sandbox, creating the metadata directory if needed.
    pub fn write(&self, sandbox: &str, metadata: &SandboxMetadata) -> Result<()> {
        fs::create_dir_all(&self.base_dir).with_context(|| {
            format!(
                "Failed to create metadata directory {}",
                self.base_dir.display()
            )
        })?;

        let path = self.metadata_path(sandbox);
        let encoded = toml::to_string(metadata)
            .with_context(|| format!("Failed to encode metadata for {sandbox}"))?;
        fs::write(&path, encoded)
            .with_context(|| format!("Failed to write metadata file {}", path.display()))?;
        Ok(())
    }

    /// Remove metadata for a sandbox if present.
    pub fn remove(&self, sandbox: &str) -> Result<()> {
        let path = self.metadata_path(sandbox);
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove metadata file {}", path.display()))?;
        }

        if self.base_dir.exists() {
            let mut entries = fs::read_dir(&self.base_dir).with_context(|| {
                format!(
                    "Failed to read metadata directory {}",
                    self.base_dir.display()
                )
            })?;
            if entries.next().is_none() {
                fs::remove_dir(&self.base_dir).with_context(|| {
                    format!(
                        "Failed to remove metadata directory {}",
                        self.base_dir.display()
                    )
                })?;
            }
        }

        Ok(())
    }

    /// Build the metadata file path for a sandbox name.
    fn metadata_path(&self, sandbox: &str) -> PathBuf {
        self.base_dir.join(format!("{sandbox}.toml"))
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn metadata_round_trip() {
        let tmp = tempdir().unwrap();
        let store = SandboxMetadataStore::new(tmp.path());

        let metadata = SandboxMetadata {
            base_commit: "abc123".to_string(),
            base_ref: Some("main".to_string()),
            created_at: 1_700_000_000,
        };

        store.write("sandbox", &metadata).unwrap();
        let loaded = store.read("sandbox").unwrap().unwrap();
        assert_eq!(metadata, loaded);
    }

    #[test]
    fn missing_metadata_returns_none() {
        let tmp = tempdir().unwrap();
        let store = SandboxMetadataStore::new(tmp.path());

        assert!(store.read("missing").unwrap().is_none());
    }

    #[test]
    fn remove_metadata_cleans_empty_directory() {
        let tmp = tempdir().unwrap();
        let store = SandboxMetadataStore::new(tmp.path());

        let metadata = SandboxMetadata {
            base_commit: "abc123".to_string(),
            base_ref: None,
            created_at: 1_700_000_001,
        };

        store.write("sandbox", &metadata).unwrap();
        store.remove("sandbox").unwrap();

        assert!(!store.base_dir.exists());
    }
}
