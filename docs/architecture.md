# afs (AgentFS) Architecture

## Overview

afs is a system for sharing file context between AI agents. It mounts shared storage (local filesystem or NFS) as local directories on each agent host via FUSE, with a centralized controller service managing metadata and access control.

## Core Concepts

| Concept | Description |
|---------|-------------|
| **Filesystem (fs)** | A named storage instance (e.g., "local-dev", "nfs-team"), dynamically registered via API with type-specific configuration |
| **Directory (dir)** | A shared directory created within a filesystem, identified by a 32-char random hex ID, protected by an access key with permissions |
| **Controller** | Centralized metadata/auth service, single instance deployment |
| **FUSE Server** | One per agent host, handles local FUSE mounts and proxies file operations |

## System Architecture

```
                         ┌────────────────────────────────────┐
                         │          Controller Host           │
                         │                                    │
                         │   ┌─────────────────────────────┐  │
                         │   │      afs-controller         │  │
                         │   │                             │  │
                         │   │  ┌──────────┐  ┌──────────┐ │  │
                         │   │  │ gRPC API │  │  SQLite  │ │  │
                         │   │  │ :9100    │  │(metadata)│ │  │
                         │   │  └────┬─────┘  └──────────┘ │  │
                         │   └───────┼─────────────────────┘  │
                         └───────────┼────────────────────────┘
                                     │
                    gRPC (ValidateToken, RegisterFs, CreateDir, ...)
                                     │
              ┌──────────────────────┼──────────────────────┐
              │                      │                      │
┌─────────────┼──────────┐ ┌─────────┼─────────┐  ┌─────────┼────────┐
│  Agent Host 1          │ │  Agent Host 2     │  │  Agent Host N    │
│                        │ │                   │  │                  │
│  ┌──────────────────┐  │ │  ┌─────────────┐  │  │  ┌────────────┐  │
│  │   afs-fuse       │  │ │  │  afs-fuse   │  │  │  │  afs-fuse  │  │
│  │   gRPC :9101     │  │ │  │  gRPC :9101 │  │  │  │  gRPC :9101│  │
│  └────────┬─────────┘  │ │  └──────┬──────┘  │  │  └─────┬──────┘  │
│           │ FUSE mount │ │         │         │  │        │         │
│    /mnt/afs/<id>       │ │  /mnt/afs/<id>    │  │  /mnt/afs/<id>   │
│           │            │ │         │         │  │        │         │
│    StorageBackend      │ │  StorageBackend   │  │  StorageBackend  │
│    (local / NFS)       │ │  (local / NFS)    │  │  (local / NFS)   │
└────────────────────────┘ └───────────────────┘  └──────────────────┘
              │                      │                      │
              └──────────────────────┼──────────────────────┘
                                     │
                              Shared Storage
                           (local disk / NFS share)
```

## Components

### Controller (`afs-controller`)

Centralized metadata and auth service. Single instance globally.

**Responsibilities:**
- Register and unregister filesystem (fs) instances
- Create and delete directories (metadata only, no actual directory creation)
- Generate 32-char hex IDs and access keys for each dir
- Validate access keys on FUSE server mount requests and return fs configuration
- Persist all state to SQLite

**Does not:**
- Touch the actual storage backend
- Manage FUSE mounts
- Proxy file operations

**gRPC API (listens on :9100):**

| RPC | Description |
|-----|-------------|
| `RegisterFs` | Register a named fs instance |
| `UnregisterFs` | Unregister an fs (requires no active dirs) |
| `ListFs` | List all registered filesystems |
| `CreateDir` | Create a dir on a given fs (metadata only) |
| `DeleteDir` | Delete a dir (soft delete, requires access key). Also revokes all active sessions first. |
| `ValidateToken` | Validate a dir's access key, return fs type and config |
| `ListDirs` | List dirs, supports filtering by fs |
| `RegisterSession` | Register an active mount session (called by FUSE server after mount) |
| `DeregisterSession` | Deregister a session (called by FUSE server on unmount) |
| `RevokeDir` | Force-unmount all active sessions for a dir (requires access key) |
| `SessionStream` | Bidirectional stream: receives heartbeats from FUSE servers, pushes ForceUnmount commands |

**SQLite Schema:**

```sql
CREATE TABLE filesystems (
    name TEXT PRIMARY KEY,
    fs_type TEXT NOT NULL,         -- "local" or "nfs"
    config TEXT NOT NULL,          -- JSON: type-specific configuration
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE dirs (
    id TEXT PRIMARY KEY,           -- 32-char random hex
    access_key TEXT NOT NULL,      -- 32-char random hex
    permission TEXT NOT NULL DEFAULT 'READ_WRITE',
    fs_name TEXT NOT NULL REFERENCES filesystems(name),
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    status TEXT NOT NULL DEFAULT 'active'  -- active / deleted
);

CREATE TABLE sessions (
    session_id TEXT PRIMARY KEY,   -- 32-char random hex
    dir_id TEXT NOT NULL REFERENCES dirs(id),
    stream_id TEXT NOT NULL,       -- identifies the FUSE server's bidi stream
    mountpoint TEXT NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
```

### FUSE Server (`afs-fuse`)

One per agent host. Mounts shared directories as local FUSE filesystems.

**Responsibilities:**
- Accept mount/unmount gRPC requests
- Validate access keys with the Controller
- Create the actual storage directory on first mount (lazy creation)
- Create FUSE sessions via the `fuser` crate
- Proxy FUSE file operations (read, write, readdir, etc.) to the StorageBackend
- Reject write operations on read-only mounts
- Track active FUSE sessions

**gRPC API (listens on :9101):**

| RPC | Description |
|-----|-------------|
| `Mount` | Mount a dir at a local path |
| `Unmount` | Unmount a dir |
| `ListMounts` | List currently active mounts |

**FUSE filesystem implementation:**

- Uses the `fuser` crate (Rust FUSE library)
- Inode mapping: maintains an `InodeTable` mapping between inode numbers and paths (inode 1 = root directory)
- Sync/async bridge: `fuser::Filesystem` trait is synchronous, `StorageBackend` is async — bridged via `tokio::runtime::Handle::block_on()`
- Each mounted dir runs as an independent FUSE session

### Sessions & Revocation

The session mechanism provides access control enforcement after initial mount. Without it, a mounted FUSE filesystem continues operating even after the directory's access key is revoked.

**Architecture:**

```
FUSE Server                                          Controller
    │                                                     │
    │─── RegisterSession(dir_id, mountpoint, stream_id) ─>│  (unary RPC)
    │<─── { session_id } ─────────────────────────────────│
    │                                                     │
    │════ SessionStream (bidi, persistent) ═══════════════│
    │  upstream:  Heartbeat { mounts: [...] }  (every 10s)│
    │  downstream: ForceUnmount { mountpoint }            │
    │                                                     │
CLI ──── RevokeDir(id, access_key) ──────────────────────>│  (unary RPC)
```

**Two communication patterns:**

1. **Unary RPCs** for discrete commands: `RegisterSession` (after mount), `DeregisterSession` (on unmount), `RevokeDir` (CLI-initiated)
2. **Bidirectional stream** for continuous state: heartbeat upstream (every 10s with full mount list) + `ForceUnmount` push downstream

**Session lifecycle:**

1. FUSE server mounts a dir → calls `RegisterSession` with `(dir_id, mountpoint, stream_id)` → gets `session_id`
2. FUSE server sends periodic heartbeats over the bidi stream with all active mounts
3. Agent CLI calls `RevokeDir(id, access_key)` → Controller validates key, finds sessions, pushes `ForceUnmount` over streams
4. FUSE server receives `ForceUnmount` → drops the `BackgroundSession` → FUSE filesystem is torn down immediately
5. On normal unmount → FUSE server calls `DeregisterSession`

**Self-healing via heartbeat reconciliation:**

- Mount in heartbeat but not in DB → auto-register (self-healing if a `RegisterSession` was lost)
- Mount in DB but not in heartbeat → auto-clean (stale session removal)
- Stream drops → Controller deletes all sessions for that `stream_id` (crash detection)

**Error handling:**

- If `RegisterSession` fails after mount, the mount still works — heartbeat will reconcile
- If `DeregisterSession` fails on unmount, the unmount still succeeds — heartbeat will reconcile
- On revoke, unreachable FUSE servers have their sessions cleaned up (best-effort)
- `DeleteDir` revokes all sessions before soft-deleting (best-effort)

### Storage (`afs-storage`)

Storage abstraction layer. Defines the `StorageBackend` trait and concrete implementations.

**`StorageBackend` trait:**

```rust
#[async_trait]
pub trait StorageBackend: Send + Sync {
    async fn init_dir(&self, id: &str) -> Result<()>;
    async fn remove_dir(&self, id: &str) -> Result<()>;
    async fn dir_exists(&self, id: &str) -> Result<bool>;
    async fn read_file(&self, id: &str, path: &Path) -> Result<Vec<u8>>;
    async fn write_file(&self, id: &str, path: &Path, data: &[u8]) -> Result<()>;
    async fn list_dir(&self, id: &str, path: &Path) -> Result<Vec<DirEntry>>;
    async fn stat(&self, id: &str, path: &Path) -> Result<FileAttr>;
    async fn mkdir(&self, id: &str, path: &Path) -> Result<()>;
    async fn remove(&self, id: &str, path: &Path) -> Result<()>;
    async fn rename(&self, id: &str, from: &Path, to: &Path) -> Result<()>;
}
```

**Implementations:**

| Implementation | Description |
|---------------|-------------|
| `LocalStorage` | Uses the local filesystem with `base_path` as root directory |
| `NfsStorage` | Same logic as LocalStorage, but `base_path` is an NFS mount point managed externally |

### CLI (`afs-cli`)

Management tool, compiled as the `afs` binary. Communicates with Controller and FUSE Server via gRPC.

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
afs dir revoke <id> --key <access_key>
afs dir list [--fs <fs-name>]

# Global options
--controller <addr>     # Controller address, default 127.0.0.1:9100
--fuse-server <addr>    # FUSE Server address, default 127.0.0.1:9101
```

## Directory Storage Layout

Dirs are stored using a 2-level hash directory layout (similar to Git's object storage) to prevent any single directory from accumulating too many entries. The **trailing** characters of the ID are used for directory levels:

```
<base_path>/<last 2 hex chars>/<second-to-last 2 hex chars>/<full id>/
```

Example: ID `a1b2c3d4e5f6789012345678abcdef90`

```
<base_path>/90/ef/a1b2c3d4e5f6789012345678abcdef90/
            ^^  ^^
            last 2   second-to-last 2
```

Implementation (`afs-storage/src/lib.rs`):

```rust
pub fn resolve_dir_path(base_path: &Path, id: &str) -> PathBuf {
    let clean: String = id.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let len = clean.len();
    let level1 = &clean[len - 2..len];
    let level2 = &clean[len - 4..len - 2];
    base_path.join(level1).join(level2).join(id)
}
```

Two levels of hex prefixes provide 256 x 256 = 65,536 buckets, sufficient for large-scale usage.

## Core Workflow

### Full Lifecycle of a Shared Directory

```
Agent 1                CLI              Controller          FUSE Server 1       Storage
  |                     |                    |                    |                 |
  |  1. Register fs     |                    |                    |                 |
  |--------------------→|-- RegisterFs -----→|                    |                 |
  |                     |←-- ok -------------|                    |                 |
  |                     |                    |                    |                 |
  |  2. Create dir      |                    |                    |                 |
  |--------------------→|-- CreateDir ------→|                    |                 |
  |                     |   (metadata only)  |                    |                 |
  |←-- id + access_key -|←-- id + key -------|                    |                 |
  |                     |                    |                    |                 |
  |  3. Mount           |                    |                    |                 |
  |--------------------→|------ Mount ---------------------------→|                 |
  |                     |                    |←-- ValidateToken --|                 |
  |                     |                    |-- permission + ---→|                 |
  |                     |                    |   fs config        |-- init_dir() --→|
  |                     |                    |                    |  (first mount)  |
  |                     |                    |                    |-- FUSE mount --→|
  |                     |                    |←- RegisterSession -|                 |
  |                     |                    |-- session_id -----→|                 |
  |←-- mounted ---------|←-- ok ----------------------------------|                 |
  |                     |                    |                    |                 |
  |  4. Read/write      |                    |                    |                 |
  |-- write to /mnt/afs/<id>/file.txt -------------------------------------------- →|
  |←-- read from /mnt/afs/<id>/file.txt ------------------------------------------- |
  |                     |                    |                    |                 |
  |  5. Share with      |                    |                    |                 |
  |     Agent 2         |                    |                    |                 |
  |-- send id + key --→ Agent 2              |                    |                 |
  |                     |                    |                    |                 |

Agent 2                CLI              Controller          FUSE Server 2       Storage
  |                     |                    |                    |                 |
  |  6. Mount           |                    |                    |                 |
  |  (different host)   |                    |                    |                 |
  |--------------------→|------ Mount ---------------------------→|                 |
  |                     |                    |←-- ValidateToken --|                 |
  |                     |                    |-- ok -------------→|-- FUSE mount --→|
  |                     |                    |←- RegisterSession -|  (dir exists)   |
  |                     |                    |-- session_id -----→|                 |
  |←-- mounted ---------|←-- ok ----------------------------------|                 |
  |                     |                    |                    |                 |
  |  7. Read shared     |                    |                    |                 |
  |     files           |                    |                    |                 |
  |←-- read from /mnt/afs/<id>/file.txt ------------------------------------------- |

Agent 1                CLI              Controller          FUSE Server 1/2     Storage
  |                     |                    |                    |                 |
  |  8. Revoke access   |                    |                    |                 |
  |--------------------→|-- RevokeDir ------→|                    |                 |
  |                     |   (validates key)  |-- ForceUnmount ---→|                 |
  |                     |                    |  (via bidi stream) |-- drop FUSE ----|
  |←-- revoked N -------|←-- sessions_revoked|                    |                 |
  |                     |                    |                    |                 |
  |  9. Unmount         |                    |                    |                 |
  |--------------------→|------ Unmount -------------------------→|                 |
  |                     |                    |←- DeregisterSession|                 |
  |←-- unmounted -------|←-- ok ----------------------------------|                 |
```

### Key Design Decisions

**Lazy Creation:** `CreateDir` only writes metadata in the Controller — no actual directory is created. The physical directory is created on first `Mount` by the FUSE Server via `StorageBackend.init_dir()`. This keeps the Controller as a pure metadata service with no need to access storage backends.

**Token Validation Flow:** On every Mount request, the FUSE Server sends a `ValidateToken` RPC to the Controller. The Controller returns the dir's permission and the fs type and configuration. The FUSE Server then instantiates the appropriate StorageBackend from this information.

**Permission Enforcement:** Enforced at the FUSE filesystem layer. Read-only mounted dirs reject all write operations (write, mkdir, unlink, rmdir, rename) in FUSE callbacks, returning `EACCES`.

## Configuration

### Controller (`controller.toml`)

```toml
[server]
listen = "0.0.0.0:9100"

[storage]
db_path = "/var/lib/afs/controller.db"
```

### FUSE Server (`fuse-server.toml`)

```toml
[server]
listen = "0.0.0.0:9101"
controller_addr = "controller-host:9100"
```

Both daemons can be started with command-line arguments or a config file path. Defaults are used when no config file is provided.

## Project Structure

```
afs/
├── Cargo.toml                  # workspace root
├── proto/
│   └── afs.proto               # gRPC protobuf definitions
├── afs-common/                 # shared types and errors
│   └── src/
│       ├── lib.rs
│       └── error.rs            # AfsError enum
├── afs-storage/                # storage abstraction layer
│   └── src/
│       ├── lib.rs              # StorageBackend trait + resolve_dir_path
│       ├── local.rs            # LocalStorage implementation
│       └── nfs.rs              # NfsStorage implementation
├── afs-controller/             # Controller service
│   ├── build.rs                # tonic-build protobuf compilation
│   ├── src/
│   │   ├── lib.rs              # proto module re-export
│   │   ├── main.rs             # entrypoint: starts gRPC server
│   │   ├── config.rs           # TOML config loading
│   │   ├── db.rs               # SQLite data access layer
│   │   └── service.rs          # ControllerService gRPC implementation
│   └── tests/
│       └── integration_test.rs # gRPC integration tests
├── afs-fuse/                   # FUSE Server service
│   ├── build.rs                # tonic-build protobuf compilation
│   └── src/
│       ├── lib.rs              # proto module re-export
│       ├── main.rs             # entrypoint: starts gRPC server
│       ├── config.rs           # TOML config loading
│       ├── filesystem.rs       # AfsFilesystem (fuser::Filesystem impl)
│       └── service.rs          # FuseService gRPC impl + mount management
├── afs-cli/                    # CLI tool
│   ├── build.rs                # tonic-build protobuf compilation
│   └── src/
│       ├── main.rs             # entrypoint: clap arg parsing + dispatch
│       └── commands/
│           ├── mod.rs          # CLI struct definitions (Cli, Commands, FsCommands, DirCommands)
│           ├── fs.rs           # afs fs add/remove/list
│           └── dir.rs          # afs dir create/delete/mount/unmount/revoke/list
├── tests/
│   └── e2e.sh                  # shell-based E2E tests (run in Docker with FUSE)
├── docs/
│   ├── architecture.md         # this file
│   └── plans/                  # design documents
└── Dockerfile.test             # Linux test environment (fuse3 + protobuf-compiler)
```

## Dependencies

| Crate | Purpose |
|-------|---------|
| `fuser` 0.17 | FUSE filesystem implementation |
| `tonic` / `prost` | gRPC server/client + protobuf code generation |
| `rusqlite` (bundled) | Embedded SQLite database |
| `clap` | CLI argument parsing |
| `tokio` | Async runtime |
| `serde` / `toml` | Configuration serialization |
| `rand` / `hex` | Random ID and access key generation |
| `tracing` | Structured logging |

## Building

```sh
# Default build (excludes afs-fuse; no macFUSE needed on macOS)
cargo build

# Full build (requires FUSE library: macFUSE on macOS, libfuse3-dev on Linux)
cargo build --workspace

# Full build and test in Docker (via OrbStack or Docker Desktop)
docker build -f Dockerfile.test -t afs-test .
docker run --rm --privileged afs-test
```

`afs-fuse` is excluded from `default-members` because it depends on a system-level FUSE library. On macOS, install macFUSE. On Linux, install `libfuse3-dev`.
