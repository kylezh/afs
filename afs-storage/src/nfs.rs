use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;

use crate::{resolve_dir_path, DirEntry, FileAttr, StorageBackend};

/// NFS storage backend.
///
/// Operates identically to LocalStorage but the base path is an NFS mount point.
/// The NFS mount itself is managed externally (e.g., system-level fstab or mount command).
pub struct NfsStorage {
    mount_path: PathBuf,
}

impl NfsStorage {
    pub fn new(mount_path: PathBuf) -> Self {
        Self { mount_path }
    }

    fn dir_root(&self, id: &str) -> PathBuf {
        resolve_dir_path(&self.mount_path, id)
    }

    fn full_path(&self, id: &str, path: &Path) -> PathBuf {
        self.dir_root(id).join(path)
    }
}

#[async_trait]
impl StorageBackend for NfsStorage {
    async fn init_dir(&self, id: &str) -> Result<()> {
        let root = self.dir_root(id);
        tokio::fs::create_dir_all(&root)
            .await
            .with_context(|| format!("failed to create dir {}", root.display()))?;
        Ok(())
    }

    async fn remove_dir(&self, id: &str) -> Result<()> {
        let root = self.dir_root(id);
        if tokio::fs::try_exists(&root).await.unwrap_or(false) {
            tokio::fs::remove_dir_all(&root)
                .await
                .with_context(|| format!("failed to remove dir {}", root.display()))?;
        }
        Ok(())
    }

    async fn dir_exists(&self, id: &str) -> Result<bool> {
        let root = self.dir_root(id);
        Ok(tokio::fs::try_exists(&root).await.unwrap_or(false))
    }

    async fn read_file(&self, id: &str, path: &Path) -> Result<Vec<u8>> {
        let full = self.full_path(id, path);
        tokio::fs::read(&full)
            .await
            .with_context(|| format!("failed to read {}", full.display()))
    }

    async fn write_file(&self, id: &str, path: &Path, data: &[u8]) -> Result<()> {
        let full = self.full_path(id, path);
        if let Some(parent) = full.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&full, data)
            .await
            .with_context(|| format!("failed to write {}", full.display()))
    }

    async fn list_dir(&self, id: &str, path: &Path) -> Result<Vec<DirEntry>> {
        let full = self.full_path(id, path);
        let mut entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&full)
            .await
            .with_context(|| format!("failed to list dir {}", full.display()))?;
        while let Some(entry) = read_dir.next_entry().await? {
            let metadata = entry.metadata().await?;
            entries.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir: metadata.is_dir(),
                size: metadata.len(),
            });
        }
        Ok(entries)
    }

    async fn stat(&self, id: &str, path: &Path) -> Result<FileAttr> {
        let full = self.full_path(id, path);
        let metadata = tokio::fs::metadata(&full)
            .await
            .with_context(|| format!("failed to stat {}", full.display()))?;
        Ok(FileAttr {
            size: metadata.len(),
            is_dir: metadata.is_dir(),
            modified: metadata.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            accessed: metadata.accessed().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            created: metadata.created().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
        })
    }

    async fn mkdir(&self, id: &str, path: &Path) -> Result<()> {
        let full = self.full_path(id, path);
        tokio::fs::create_dir_all(&full)
            .await
            .with_context(|| format!("failed to mkdir {}", full.display()))
    }

    async fn remove(&self, id: &str, path: &Path) -> Result<()> {
        let full = self.full_path(id, path);
        let metadata = tokio::fs::metadata(&full).await?;
        if metadata.is_dir() {
            tokio::fs::remove_dir_all(&full).await?;
        } else {
            tokio::fs::remove_file(&full).await?;
        }
        Ok(())
    }

    async fn rename(&self, id: &str, from: &Path, to: &Path) -> Result<()> {
        let from_full = self.full_path(id, from);
        let to_full = self.full_path(id, to);
        if let Some(parent) = to_full.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::rename(&from_full, &to_full)
            .await
            .with_context(|| {
                format!(
                    "failed to rename {} -> {}",
                    from_full.display(),
                    to_full.display()
                )
            })
    }
}
