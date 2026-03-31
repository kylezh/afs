pub mod local;
pub mod nfs;

use std::path::{Path, PathBuf};

use anyhow::Result;
use async_trait::async_trait;

/// Metadata about a directory entry.
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// File attributes.
#[derive(Debug, Clone)]
pub struct FileAttr {
    pub size: u64,
    pub is_dir: bool,
    pub modified: std::time::SystemTime,
    pub accessed: std::time::SystemTime,
    pub created: std::time::SystemTime,
}

/// Storage backend trait — all file operations for a given filesystem instance.
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Create the actual directory for a dir (called on first mount).
    async fn init_dir(&self, id: &str) -> Result<()>;

    /// Remove the directory (called on delete).
    async fn remove_dir(&self, id: &str) -> Result<()>;

    /// Check if the directory exists.
    async fn dir_exists(&self, id: &str) -> Result<bool>;

    /// Read a file's contents.
    async fn read_file(&self, id: &str, path: &Path) -> Result<Vec<u8>>;

    /// Write data to a file (create or overwrite).
    async fn write_file(&self, id: &str, path: &Path, data: &[u8]) -> Result<()>;

    /// List entries in a subdirectory.
    async fn list_dir(&self, id: &str, path: &Path) -> Result<Vec<DirEntry>>;

    /// Get file attributes.
    async fn stat(&self, id: &str, path: &Path) -> Result<FileAttr>;

    /// Create a subdirectory.
    async fn mkdir(&self, id: &str, path: &Path) -> Result<()>;

    /// Remove a file or directory.
    async fn remove(&self, id: &str, path: &Path) -> Result<()>;

    /// Rename a file or directory.
    async fn rename(&self, id: &str, from: &Path, to: &Path) -> Result<()>;
}

/// Resolve the physical path for a dir ID using 2-level hash layout.
///
/// Layout: `<base_path>/<last 2 hex>/<second-to-last 2 hex>/<id>/`
pub fn resolve_dir_path(base_path: &Path, id: &str) -> PathBuf {
    let clean: String = id.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let len = clean.len();
    let level1 = &clean[len - 2..len];
    let level2 = &clean[len - 4..len - 2];
    base_path.join(level1).join(level2).join(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_dir_path() {
        let base = Path::new("/data");
        let id = "a1b2c3d4e5f6789012345678abcdef90";
        let path = resolve_dir_path(base, id);
        assert_eq!(path, PathBuf::from("/data/90/ef/a1b2c3d4e5f6789012345678abcdef90"));
    }
}
