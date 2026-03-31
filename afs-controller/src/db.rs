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

/// Represents an active mount session.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub session_id: String,
    pub dir_id: String,
    pub stream_id: String,
    pub mountpoint: String,
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
            "PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS filesystems (
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
            );
            CREATE TABLE IF NOT EXISTS sessions (
                session_id TEXT PRIMARY KEY,
                dir_id TEXT NOT NULL REFERENCES dirs(id),
                stream_id TEXT NOT NULL,
                mountpoint TEXT NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
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

    // --- Session operations ---

    pub fn register_session(
        &self,
        dir_id: &str,
        stream_id: &str,
        mountpoint: &str,
    ) -> Result<SessionRecord> {
        let session_id = generate_hex_id();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sessions (session_id, dir_id, stream_id, mountpoint) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![session_id, dir_id, stream_id, mountpoint],
        )
        .context("failed to register session")?;

        let mut stmt = conn.prepare(
            "SELECT session_id, dir_id, stream_id, mountpoint, created_at FROM sessions WHERE session_id = ?1",
        )?;
        let record = stmt.query_row(rusqlite::params![session_id], |row| {
            Ok(SessionRecord {
                session_id: row.get(0)?,
                dir_id: row.get(1)?,
                stream_id: row.get(2)?,
                mountpoint: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        Ok(record)
    }

    pub fn deregister_session(&self, session_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM sessions WHERE session_id = ?1",
            rusqlite::params![session_id],
        )?;
        Ok(rows > 0)
    }

    pub fn get_sessions_for_dir(&self, dir_id: &str) -> Result<Vec<SessionRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT session_id, dir_id, stream_id, mountpoint, created_at FROM sessions WHERE dir_id = ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![dir_id], |row| {
            Ok(SessionRecord {
                session_id: row.get(0)?,
                dir_id: row.get(1)?,
                stream_id: row.get(2)?,
                mountpoint: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn get_sessions_by_stream(&self, stream_id: &str) -> Result<Vec<SessionRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT session_id, dir_id, stream_id, mountpoint, created_at FROM sessions WHERE stream_id = ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![stream_id], |row| {
            Ok(SessionRecord {
                session_id: row.get(0)?,
                dir_id: row.get(1)?,
                stream_id: row.get(2)?,
                mountpoint: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn delete_sessions_by_stream(&self, stream_id: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM sessions WHERE stream_id = ?1",
            rusqlite::params![stream_id],
        )?;
        Ok(rows)
    }

    /// Reconcile session state from a heartbeat: insert missing mounts, remove stale ones.
    pub fn reconcile_sessions_from_heartbeat(
        &self,
        stream_id: &str,
        mounts: &[(&str, &str)], // (dir_id, mountpoint)
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Get current sessions for this stream
        let mut stmt = conn.prepare(
            "SELECT session_id, dir_id, mountpoint FROM sessions WHERE stream_id = ?1",
        )?;
        let existing: Vec<(String, String, String)> = stmt
            .query_map(rusqlite::params![stream_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<std::result::Result<_, _>>()?;

        // Build set of (dir_id, mountpoint) from heartbeat
        let heartbeat_set: std::collections::HashSet<(&str, &str)> =
            mounts.iter().copied().collect();

        // Build set of (dir_id, mountpoint) from DB
        let db_set: std::collections::HashSet<(String, String)> = existing
            .iter()
            .map(|(_, d, m)| (d.clone(), m.clone()))
            .collect();

        // Insert missing (in heartbeat but not in DB)
        for (dir_id, mountpoint) in &heartbeat_set {
            if !db_set.contains(&(dir_id.to_string(), mountpoint.to_string())) {
                let session_id = generate_hex_id();
                conn.execute(
                    "INSERT INTO sessions (session_id, dir_id, stream_id, mountpoint) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![session_id, dir_id, stream_id, mountpoint],
                )?;
            }
        }

        // Remove stale (in DB but not in heartbeat)
        for (session_id, dir_id, mountpoint) in &existing {
            if !heartbeat_set.contains(&(dir_id.as_str(), mountpoint.as_str())) {
                conn.execute(
                    "DELETE FROM sessions WHERE session_id = ?1",
                    rusqlite::params![session_id],
                )?;
            }
        }

        Ok(())
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

    #[test]
    fn test_session_lifecycle() {
        let db = Database::open(":memory:").unwrap();
        db.register_fs("local-dev", "local", r#"{"base_path":"/data"}"#)
            .unwrap();
        let dir1 = db.create_dir("local-dev").unwrap();
        let dir2 = db.create_dir("local-dev").unwrap();

        // Register sessions
        let s1 = db
            .register_session(&dir1.id, "stream-a", "/mnt/agent1")
            .unwrap();
        assert_eq!(s1.session_id.len(), 32);
        assert_eq!(s1.dir_id, dir1.id);

        let s2 = db
            .register_session(&dir1.id, "stream-b", "/mnt/agent2")
            .unwrap();
        let _s3 = db
            .register_session(&dir2.id, "stream-a", "/mnt/agent1-dir2")
            .unwrap();

        // Query by dir
        let sessions = db.get_sessions_for_dir(&dir1.id).unwrap();
        assert_eq!(sessions.len(), 2);

        // Query by stream
        let sessions = db.get_sessions_by_stream("stream-a").unwrap();
        assert_eq!(sessions.len(), 2); // s1 and s3

        // Deregister one
        assert!(db.deregister_session(&s2.session_id).unwrap());
        assert!(!db.deregister_session("nonexistent").unwrap());
        let sessions = db.get_sessions_for_dir(&dir1.id).unwrap();
        assert_eq!(sessions.len(), 1);

        // Delete all sessions for a stream (crash cleanup)
        let deleted = db.delete_sessions_by_stream("stream-a").unwrap();
        assert_eq!(deleted, 2); // s1 and s3
        assert_eq!(db.get_sessions_for_dir(&dir1.id).unwrap().len(), 0);
        assert_eq!(db.get_sessions_for_dir(&dir2.id).unwrap().len(), 0);
    }

    #[test]
    fn test_session_heartbeat_reconciliation() {
        let db = Database::open(":memory:").unwrap();
        db.register_fs("local-dev", "local", r#"{"base_path":"/data"}"#)
            .unwrap();
        let dir1 = db.create_dir("local-dev").unwrap();
        let dir2 = db.create_dir("local-dev").unwrap();

        // Register initial session
        db.register_session(&dir1.id, "stream-a", "/mnt/a")
            .unwrap();

        // Heartbeat with dir1 still there + new dir2
        db.reconcile_sessions_from_heartbeat(
            "stream-a",
            &[(&dir1.id, "/mnt/a"), (&dir2.id, "/mnt/b")],
        )
        .unwrap();

        let sessions = db.get_sessions_by_stream("stream-a").unwrap();
        assert_eq!(sessions.len(), 2);

        // Heartbeat with only dir2 (dir1 removed)
        db.reconcile_sessions_from_heartbeat("stream-a", &[(&dir2.id, "/mnt/b")])
            .unwrap();

        let sessions = db.get_sessions_by_stream("stream-a").unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].dir_id, dir2.id);
    }

    #[test]
    fn test_reconcile_empty_heartbeat_clears_sessions() {
        let db = Database::open(":memory:").unwrap();
        db.register_fs("local-dev", "local", r#"{"base_path":"/data"}"#)
            .unwrap();
        let dir1 = db.create_dir("local-dev").unwrap();
        let dir2 = db.create_dir("local-dev").unwrap();

        db.register_session(&dir1.id, "stream-a", "/mnt/a").unwrap();
        db.register_session(&dir2.id, "stream-a", "/mnt/b").unwrap();
        assert_eq!(db.get_sessions_by_stream("stream-a").unwrap().len(), 2);

        // Empty heartbeat → all sessions for this stream should be removed
        db.reconcile_sessions_from_heartbeat("stream-a", &[]).unwrap();
        assert_eq!(db.get_sessions_by_stream("stream-a").unwrap().len(), 0);
    }

    #[test]
    fn test_delete_sessions_by_nonexistent_stream() {
        let db = Database::open(":memory:").unwrap();
        let deleted = db.delete_sessions_by_stream("nonexistent").unwrap();
        assert_eq!(deleted, 0);
    }

    #[test]
    fn test_get_sessions_for_nonexistent_dir() {
        let db = Database::open(":memory:").unwrap();
        let sessions = db.get_sessions_for_dir("nonexistent").unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_multiple_sessions_same_dir_different_streams() {
        let db = Database::open(":memory:").unwrap();
        db.register_fs("local-dev", "local", r#"{"base_path":"/data"}"#)
            .unwrap();
        let dir = db.create_dir("local-dev").unwrap();

        let s1 = db.register_session(&dir.id, "stream-a", "/mnt/a").unwrap();
        let s2 = db.register_session(&dir.id, "stream-b", "/mnt/b").unwrap();

        // Both sessions exist for the same dir
        let sessions = db.get_sessions_for_dir(&dir.id).unwrap();
        assert_eq!(sessions.len(), 2);

        // Each stream sees only its own session
        assert_eq!(db.get_sessions_by_stream("stream-a").unwrap().len(), 1);
        assert_eq!(db.get_sessions_by_stream("stream-b").unwrap().len(), 1);

        // Deregister one doesn't affect the other
        db.deregister_session(&s1.session_id).unwrap();
        assert_eq!(db.get_sessions_for_dir(&dir.id).unwrap().len(), 1);
        assert_eq!(db.get_sessions_for_dir(&dir.id).unwrap()[0].session_id, s2.session_id);
    }

    #[test]
    fn test_reconcile_idempotent() {
        let db = Database::open(":memory:").unwrap();
        db.register_fs("local-dev", "local", r#"{"base_path":"/data"}"#)
            .unwrap();
        let dir = db.create_dir("local-dev").unwrap();

        let mounts = [(&*dir.id, "/mnt/a")];

        // First reconcile creates the session
        db.reconcile_sessions_from_heartbeat("stream-a", &mounts).unwrap();
        assert_eq!(db.get_sessions_by_stream("stream-a").unwrap().len(), 1);

        // Second identical reconcile doesn't duplicate
        db.reconcile_sessions_from_heartbeat("stream-a", &mounts).unwrap();
        assert_eq!(db.get_sessions_by_stream("stream-a").unwrap().len(), 1);

        // Third identical reconcile still idempotent
        db.reconcile_sessions_from_heartbeat("stream-a", &mounts).unwrap();
        assert_eq!(db.get_sessions_by_stream("stream-a").unwrap().len(), 1);
    }
}
