use std::sync::Mutex;

use anyhow::{Context, Result};
use rand::Rng;
use rusqlite::Connection;

/// Represents a registered filesystem.
#[derive(Debug, Clone)]
pub struct FsRecord {
    pub name: String,
    pub fs_type: String,
    pub config: String,
    pub created_at: String,
}

/// Represents a shared directory.
#[derive(Debug, Clone)]
pub struct DirRecord {
    pub id: String,
    pub access_key: String,
    pub permission: String,
    pub fs_name: String,
    pub created_at: String,
    pub status: String,
}

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &str) -> Result<Self> {
        let conn = if path == ":memory:" {
            Connection::open_in_memory()?
        } else {
            if let Some(parent) = std::path::Path::new(path).parent() {
                std::fs::create_dir_all(parent)?;
            }
            Connection::open(path)?
        };
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS filesystems (
                name TEXT PRIMARY KEY,
                fs_type TEXT NOT NULL,
                config TEXT NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS dirs (
                id TEXT PRIMARY KEY,
                access_key TEXT NOT NULL,
                permission TEXT NOT NULL DEFAULT 'READ_WRITE',
                fs_name TEXT NOT NULL REFERENCES filesystems(name),
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                status TEXT NOT NULL DEFAULT 'active'
            );",
        )
        .context("failed to init schema")?;
        Ok(())
    }

    // --- Filesystem operations ---

    pub fn register_fs(&self, name: &str, fs_type: &str, config: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO filesystems (name, fs_type, config) VALUES (?1, ?2, ?3)",
            rusqlite::params![name, fs_type, config],
        )
        .context("failed to register fs")?;
        Ok(())
    }

    pub fn unregister_fs(&self, name: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        // Check for active dirs on this fs
        let active_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM dirs WHERE fs_name = ?1 AND status = 'active'",
            rusqlite::params![name],
            |row| row.get(0),
        )?;
        if active_count > 0 {
            anyhow::bail!(
                "cannot unregister fs '{}': {} active dir(s) exist",
                name,
                active_count
            );
        }
        // Clean up soft-deleted dir records that reference this fs
        conn.execute(
            "DELETE FROM dirs WHERE fs_name = ?1 AND status = 'deleted'",
            rusqlite::params![name],
        )?;
        let rows = conn.execute(
            "DELETE FROM filesystems WHERE name = ?1",
            rusqlite::params![name],
        )?;
        Ok(rows > 0)
    }

    pub fn list_fs(&self) -> Result<Vec<FsRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT name, fs_type, config, created_at FROM filesystems")?;
        let rows = stmt.query_map([], |row| {
            Ok(FsRecord {
                name: row.get(0)?,
                fs_type: row.get(1)?,
                config: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn get_fs(&self, name: &str) -> Result<Option<FsRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT name, fs_type, config, created_at FROM filesystems WHERE name = ?1")?;
        let mut rows = stmt.query_map(rusqlite::params![name], |row| {
            Ok(FsRecord {
                name: row.get(0)?,
                fs_type: row.get(1)?,
                config: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    // --- Directory operations ---

    pub fn create_dir(&self, fs_name: &str) -> Result<DirRecord> {
        // Verify fs exists
        if self.get_fs(fs_name)?.is_none() {
            anyhow::bail!("filesystem '{}' not found", fs_name);
        }

        let id = generate_hex_id();
        let access_key = generate_hex_id();

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO dirs (id, access_key, permission, fs_name) VALUES (?1, ?2, 'READ_WRITE', ?3)",
            rusqlite::params![id, access_key, fs_name],
        )
        .context("failed to create dir")?;

        // Read back the created record
        let mut stmt = conn.prepare(
            "SELECT id, access_key, permission, fs_name, created_at, status FROM dirs WHERE id = ?1",
        )?;
        let record = stmt.query_row(rusqlite::params![id], |row| {
            Ok(DirRecord {
                id: row.get(0)?,
                access_key: row.get(1)?,
                permission: row.get(2)?,
                fs_name: row.get(3)?,
                created_at: row.get(4)?,
                status: row.get(5)?,
            })
        })?;
        Ok(record)
    }

    pub fn delete_dir(&self, id: &str, access_key: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE dirs SET status = 'deleted' WHERE id = ?1 AND access_key = ?2 AND status = 'active'",
            rusqlite::params![id, access_key],
        )?;
        Ok(rows > 0)
    }

    pub fn validate_token(&self, id: &str, access_key: &str) -> Result<Option<(DirRecord, FsRecord)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT d.id, d.access_key, d.permission, d.fs_name, d.created_at, d.status,
                    f.name, f.fs_type, f.config, f.created_at
             FROM dirs d
             JOIN filesystems f ON d.fs_name = f.name
             WHERE d.id = ?1 AND d.access_key = ?2 AND d.status = 'active'",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![id, access_key], |row| {
            Ok((
                DirRecord {
                    id: row.get(0)?,
                    access_key: row.get(1)?,
                    permission: row.get(2)?,
                    fs_name: row.get(3)?,
                    created_at: row.get(4)?,
                    status: row.get(5)?,
                },
                FsRecord {
                    name: row.get(6)?,
                    fs_type: row.get(7)?,
                    config: row.get(8)?,
                    created_at: row.get(9)?,
                },
            ))
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn list_dirs(&self, fs_name: Option<&str>) -> Result<Vec<DirRecord>> {
        let conn = self.conn.lock().unwrap();
        let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match fs_name {
            Some(name) if !name.is_empty() => (
                "SELECT id, access_key, permission, fs_name, created_at, status FROM dirs WHERE fs_name = ?1 AND status = 'active'",
                vec![Box::new(name.to_string())],
            ),
            _ => (
                "SELECT id, access_key, permission, fs_name, created_at, status FROM dirs WHERE status = 'active'",
                vec![],
            ),
        };
        let mut stmt = conn.prepare(sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            Ok(DirRecord {
                id: row.get(0)?,
                access_key: row.get(1)?,
                permission: row.get(2)?,
                fs_name: row.get(3)?,
                created_at: row.get(4)?,
                status: row.get(5)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }
}

/// Generate a 32-character random hex string.
fn generate_hex_id() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 16] = rng.gen();
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fs_lifecycle() {
        let db = Database::open(":memory:").unwrap();

        // Register
        db.register_fs("local-dev", "local", r#"{"base_path":"/data"}"#)
            .unwrap();
        db.register_fs("nfs-team", "nfs", r#"{"mount_path":"/nfs"}"#)
            .unwrap();

        // List
        let fss = db.list_fs().unwrap();
        assert_eq!(fss.len(), 2);

        // Get
        let fs = db.get_fs("local-dev").unwrap().unwrap();
        assert_eq!(fs.fs_type, "local");

        // Unregister
        assert!(db.unregister_fs("nfs-team").unwrap());
        assert_eq!(db.list_fs().unwrap().len(), 1);

        // Not found
        assert!(!db.unregister_fs("nonexistent").unwrap());
    }

    #[test]
    fn test_dir_lifecycle() {
        let db = Database::open(":memory:").unwrap();
        db.register_fs("local-dev", "local", r#"{"base_path":"/data"}"#)
            .unwrap();

        // Create
        let dir = db.create_dir("local-dev").unwrap();
        assert_eq!(dir.id.len(), 32);
        assert_eq!(dir.access_key.len(), 32);
        assert_eq!(dir.permission, "READ_WRITE");
        assert_eq!(dir.fs_name, "local-dev");
        assert_eq!(dir.status, "active");

        // Validate token
        let result = db.validate_token(&dir.id, &dir.access_key).unwrap();
        assert!(result.is_some());
        let (d, f) = result.unwrap();
        assert_eq!(d.id, dir.id);
        assert_eq!(f.name, "local-dev");

        // Invalid token
        let result = db.validate_token(&dir.id, "wrong-key").unwrap();
        assert!(result.is_none());

        // List
        let dirs = db.list_dirs(None).unwrap();
        assert_eq!(dirs.len(), 1);
        let dirs = db.list_dirs(Some("local-dev")).unwrap();
        assert_eq!(dirs.len(), 1);
        let dirs = db.list_dirs(Some("nonexistent")).unwrap();
        assert_eq!(dirs.len(), 0);

        // Delete
        assert!(db.delete_dir(&dir.id, &dir.access_key).unwrap());
        let dirs = db.list_dirs(None).unwrap();
        assert_eq!(dirs.len(), 0);

        // Validate deleted dir
        let result = db.validate_token(&dir.id, &dir.access_key).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_cannot_unregister_fs_with_active_dirs() {
        let db = Database::open(":memory:").unwrap();
        db.register_fs("local-dev", "local", r#"{"base_path":"/data"}"#)
            .unwrap();
        let _dir = db.create_dir("local-dev").unwrap();

        let err = db.unregister_fs("local-dev").unwrap_err();
        assert!(err.to_string().contains("active dir(s) exist"));
    }

    #[test]
    fn test_cannot_create_dir_on_nonexistent_fs() {
        let db = Database::open(":memory:").unwrap();
        let err = db.create_dir("nonexistent").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }
}
