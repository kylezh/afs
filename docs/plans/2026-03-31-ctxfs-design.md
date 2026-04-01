# afs (AgentFS) Design Document

## Overview

afs (AgentFS) is a system for sharing file context between multiple AI agents. It provides a FUSE-based virtual filesystem backed by named filesystem instances (local or NFS), managed through a centralized controller service.

**Key concepts:**
- **Filesystem (fs)** — a named, registered storage instance (e.g., "local-dev", "nfs-team"). Configured with type-specific parameters. Managed dynamically via API.
- **Directory (dir)** — a shared directory created within a filesystem. Identified by a random hex ID, protected by access key with permissions. Mounted as a local FUSE filesystem by agents.

## Problem

Agents running on different hosts need to share working directories (code, data, context files). There is no lightweight, agent-friendly mechanism to create temporary shared directories with access control and mount them as local filesystems.

## Architecture

### Components

1. **Controller** (`afs-controller`) — Centralized metadata/auth service (1 instance)
   - Registers/unregisters filesystem instances
   - Creates/deletes dirs (metadata only, lazy creation on mount)
   - Generates access tokens with read/write permissions
   - Validates tokens on mount requests from FUSE servers
   - Persists state in SQLite

2. **FUSE Server** (`afs-fuse`) — Per-host daemon (1 per agent host)
   - Receives mount/unmount requests via gRPC
   - Validates access tokens with Controller
   - Creates actual storage directory on first mount (lazy creation)
   - Mounts dirs as FUSE filesystems
   - Enforces permissions (read-only mounts reject writes)
   - Proxies file operations through the storage layer

3. **CLI** (`afs-cli`) — Management tool
   - `afs fs` — filesystem management (add, remove, list)
   - `afs dir` — directory management (create, delete, mount, unmount, list)
   - `afs controller` / `afs fuse-server` — start daemons

4. **Storage** (`afs-storage`) — Storage abstraction
   - `StorageBackend` trait for pluggable storage types
   - `LocalStorage` — local filesystem under a configurable base path
   - `NfsStorage` — NFS-mounted path

### Diagram

```
Agent Host                              Controller Host
┌──────────────────────┐               ┌─────────────────────┐
│  CLI / Agent         │               │  afs-controller     │
│       │              │               │  ┌──────┐ ┌───────┐ │
│       │ gRPC         │               │  │ gRPC │ │SQLite │ │
│       ▼              │               │  └──┬───┘ └───────┘ │
│  afs-fuse ◄───────────────── gRPC ────────▶│               │
│       │              │               └─────────────────────┘
│  FUSE mount          │
│       ▼              │
│  /mnt/afs/<id>       │
│       │              │
│  StorageBackend      │
│  (local / NFS)       │
└──────────────────────┘
```

## Workflow

1. Admin registers a filesystem: `afs fs add local-dev --type local --base-path /data`
2. Agent 1 creates a dir: `afs dir create --fs local-dev` → returns `id` + `access_key`
3. Agent 1 mounts: `afs dir mount <id> --key <key> --mountpoint /path` → FUSE server validates token → creates directory on first mount → mounts FUSE
4. Agent 1 writes files to `/path`
5. Agent 1 shares `id` + `access_key` with Agent 2
6. Agent 2 mounts on its host: `afs dir mount <id> --key <key> --mountpoint /path` → reads files

## gRPC API

### Controller Service

```protobuf
syntax = "proto3";
package afs;

service ControllerService {
  // Filesystem management
  rpc RegisterFs(RegisterFsRequest) returns (RegisterFsResponse);
  rpc UnregisterFs(UnregisterFsRequest) returns (UnregisterFsResponse);
  rpc ListFs(ListFsRequest) returns (ListFsResponse);

  // Directory management
  rpc CreateDir(CreateDirRequest) returns (CreateDirResponse);
  rpc DeleteDir(DeleteDirRequest) returns (DeleteDirResponse);
  rpc ValidateToken(ValidateTokenRequest) returns (ValidateTokenResponse);
  rpc ListDirs(ListDirsRequest) returns (ListDirsResponse);
}

enum Permission {
  READ_ONLY = 0;
  READ_WRITE = 1;
}

// --- Filesystem messages ---

message RegisterFsRequest {
  string name = 1;                      // unique name, e.g., "local-dev"
  string fs_type = 2;                   // "local" or "nfs"
  map<string, string> config = 3;       // type-specific config
}

message RegisterFsResponse {
  string name = 1;
}

message UnregisterFsRequest {
  string name = 1;
}

message UnregisterFsResponse {}

message ListFsRequest {}

message ListFsResponse {
  repeated FsInfo filesystems = 1;
}

message FsInfo {
  string name = 1;
  string fs_type = 2;
  map<string, string> config = 3;
  string created_at = 4;
}

// --- Directory messages ---

message CreateDirRequest {
  string fs_name = 1;                   // which filesystem to create dir in
}

message CreateDirResponse {
  string id = 1;
  string access_key = 2;
  Permission permission = 3;
}

message DeleteDirRequest {
  string id = 1;
  string access_key = 2;
}

message DeleteDirResponse {}

message ValidateTokenRequest {
  string id = 1;
  string access_key = 2;
}

message ValidateTokenResponse {
  bool valid = 1;
  Permission permission = 2;
  string fs_type = 3;
  map<string, string> fs_config = 4;
}

message ListDirsRequest {
  string fs_name = 1;                   // optional: filter by filesystem
}

message ListDirsResponse {
  repeated DirInfo dirs = 1;
}

message DirInfo {
  string id = 1;
  string fs_name = 2;
  Permission permission = 3;
  string status = 4;
  string created_at = 5;
}
```

### FUSE Service

```protobuf
service FuseService {
  rpc Mount(MountRequest) returns (MountResponse);
  rpc Unmount(UnmountRequest) returns (UnmountResponse);
  rpc ListMounts(ListMountsRequest) returns (ListMountsResponse);
}

message MountRequest {
  string id = 1;
  string access_key = 2;
  string mountpoint = 3;
  string controller_addr = 4;
}

message MountResponse {
  string mountpoint = 1;
}

message UnmountRequest {
  string mountpoint = 1;
}

message UnmountResponse {}

message ListMountsRequest {}

message ListMountsResponse {
  repeated MountInfo mounts = 1;
}

message MountInfo {
  string id = 1;
  string mountpoint = 2;
  Permission permission = 3;
}
```

## Storage Backend

```rust
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Create the actual directory (called on first mount)
    async fn init_dir(&self, id: &str) -> Result<()>;
    /// Remove the directory (called on delete)
    async fn remove_dir(&self, id: &str) -> Result<()>;
    /// Check if directory exists
    async fn dir_exists(&self, id: &str) -> Result<bool>;

    // File operations
    async fn read_file(&self, id: &str, path: &Path) -> Result<Vec<u8>>;
    async fn write_file(&self, id: &str, path: &Path, data: &[u8]) -> Result<()>;
    async fn list_dir(&self, id: &str, path: &Path) -> Result<Vec<DirEntry>>;
    async fn stat(&self, id: &str, path: &Path) -> Result<FileAttr>;
    async fn mkdir(&self, id: &str, path: &Path) -> Result<()>;
    async fn remove(&self, id: &str, path: &Path) -> Result<()>;
    async fn rename(&self, id: &str, from: &Path, to: &Path) -> Result<()>;
}
```

### Dir ID Generation

Dir IDs are 32-character random hex strings (128-bit), generated using a cryptographically secure random number generator.

### Directory Placement

Dirs are placed using a 2-level hash directory layout (similar to Git's object storage) to avoid any single directory accumulating too many entries. The **trailing** characters of the ID are used for directory placement:

```
<base_path>/<last 2 hex chars>/<second-to-last 2 hex chars>/<full id>/
```

Example: ID `a1b2c3d4e5f6789012345678abcdef90` is stored at:
```
<base_path>/90/ef/a1b2c3d4e5f6789012345678abcdef90/
            ^^  ^^
            last 2   second-to-last 2
```

This is implemented in the `StorageBackend` — the `resolve_path` method computes the physical path from an ID:

```rust
fn resolve_dir_path(&self, id: &str) -> PathBuf {
    let len = id.len();
    self.base_path
        .join(&id[len-2..len])
        .join(&id[len-4..len-2])
        .join(id)
}
```

**LocalStorage**: uses `base_path` as root, places dirs via hash layout, direct filesystem operations.

**NfsStorage**: same hash layout but `base_path` is an NFS mount point. The NFS mount itself is managed externally.

Each `StorageBackend` instance is constructed from the filesystem's config (`base_path` for local, `mount_path` for NFS).

## CLI

```
# Filesystem management
afs fs add <name> --type local --base-path <path>
afs fs add <name> --type nfs --nfs-server <addr> --nfs-path <path> --mount-path <path>
afs fs remove <name>
afs fs list

# Directory management
afs dir create --fs <fs-name>
afs dir delete <id> --key <access_key>
afs dir mount <id> --key <key> --mountpoint <path> [--readonly]
afs dir unmount <mountpoint>
afs dir list [--fs <fs-name>]

# Daemon management
afs controller [--listen <addr>] [--db <path>] [--config <path>]
afs fuse-server [--listen <addr>] [--controller <addr>] [--config <path>]
```

## Configuration

```toml
# controller.toml
[server]
listen = "0.0.0.0:9100"

[storage]
db_path = "/var/lib/afs/controller.db"
```

```toml
# fuse-server.toml
[server]
listen = "0.0.0.0:9101"
controller_addr = "controller-host:9100"
```

## SQLite Schema

```sql
CREATE TABLE filesystems (
    name TEXT PRIMARY KEY,
    fs_type TEXT NOT NULL,
    config TEXT NOT NULL,           -- JSON: type-specific config
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE dirs (
    id TEXT PRIMARY KEY,
    access_key TEXT NOT NULL,
    permission TEXT NOT NULL DEFAULT 'READ_WRITE',
    fs_name TEXT NOT NULL REFERENCES filesystems(name),
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    status TEXT NOT NULL DEFAULT 'active'
);
```

## Key Crates

- `fuser` — FUSE filesystem implementation
- `tonic` / `prost` — gRPC server/client + protobuf
- `clap` — CLI argument parsing
- `rusqlite` — SQLite
- `tokio` — async runtime
- `serde` / `toml` — configuration
- `rand` — random hex ID and access key generation

## Project Structure

```
afs/
├── Cargo.toml              # workspace root
├── proto/
│   └── afs.proto            # protobuf definitions
├── afs-common/              # shared types, errors, config
├── afs-storage/             # StorageBackend trait + impls (local, nfs)
├── afs-controller/          # controller binary + library
├── afs-fuse/                # FUSE server binary + library
└── afs-cli/                 # CLI binary
```
