use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;

use crate::{resolve_dir_path, DirEntry, FileAttr, StorageBackend};

/// Local filesystem storage backend.
pub struct LocalStorage {
    base_path: PathBuf,
}

impl LocalStorage {
    pub fn new(base_path: PathBuf) -> Self {
        Self { base_path }
    }

    fn dir_root(&self, id: &str) -> PathBuf {
        resolve_dir_path(&self.base_path, id)
    }

    fn full_path(&self, id: &str, path: &Path) -> PathBuf {
        self.dir_root(id).join(path)
    }
}

#[async_trait]
impl StorageBackend for LocalStorage {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_storage_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = LocalStorage::new(tmp.path().to_path_buf());
        let id = "a1b2c3d4e5f6789012345678abcdef90";

        // init
        assert!(!storage.dir_exists(id).await.unwrap());
        storage.init_dir(id).await.unwrap();
        assert!(storage.dir_exists(id).await.unwrap());

        // write + read
        storage
            .write_file(id, Path::new("test.txt"), b"hello world")
            .await
            .unwrap();
        let data = storage.read_file(id, Path::new("test.txt")).await.unwrap();
        assert_eq!(data, b"hello world");

        // stat
        let attr = storage.stat(id, Path::new("test.txt")).await.unwrap();
        assert_eq!(attr.size, 11);
        assert!(!attr.is_dir);

        // mkdir + list
        storage.mkdir(id, Path::new("subdir")).await.unwrap();
        let entries = storage.list_dir(id, Path::new("")).await.unwrap();
        assert_eq!(entries.len(), 2);

        // rename
        storage
            .rename(id, Path::new("test.txt"), Path::new("renamed.txt"))
            .await
            .unwrap();
        assert!(storage.read_file(id, Path::new("test.txt")).await.is_err());
        let data = storage
            .read_file(id, Path::new("renamed.txt"))
            .await
            .unwrap();
        assert_eq!(data, b"hello world");

        // remove
        storage.remove(id, Path::new("renamed.txt")).await.unwrap();
        storage.remove(id, Path::new("subdir")).await.unwrap();

        // remove_dir
        storage.remove_dir(id).await.unwrap();
        assert!(!storage.dir_exists(id).await.unwrap());
    }
}
