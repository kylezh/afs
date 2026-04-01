# afs (AgentFS)

FUSE-based shared filesystem for AI agents. Agents create shared directories via a central controller, then mount them locally via per-host FUSE servers.

## Architecture

```
Agent Host 1                    Controller                    Agent Host 2
┌──────────────┐          ┌──────────────────┐          ┌──────────────┐
│  afs-fuse    │── gRPC ──│  afs-controller  │── gRPC ──│  afs-fuse    │
│  FUSE mounts │          │  SQLite metadata │          │  FUSE mounts │
└──────┬───────┘          └──────────────────┘          └──────┬───────┘
       │                                                       │
  /mnt/afs/<id>              Shared Storage               /mnt/afs/<id>
       └──────────────── (local disk / NFS) ───────────────────┘
```

- **Controller** — centralized metadata and auth service (gRPC + SQLite). Manages filesystems, directories, sessions, and access revocation.
- **FUSE Server** — one per agent host. Mounts shared directories as local FUSE filesystems and proxies file operations to the storage backend.
- **CLI** — management tool (`afs`) for filesystem and directory operations.

See [docs/architecture.md](docs/architecture.md) for the full design.

## Prerequisites

| Platform | Requirements |
|----------|-------------|
| **Linux** | `libfuse3-dev`, `pkg-config`, `protobuf-compiler` |
| **macOS** | [macFUSE](https://osxfuse.github.io/), `protobuf` (via Homebrew) |

Rust toolchain (1.86+) is required. Install via [rustup](https://rustup.rs/).

## Build

```sh
# Default build (excludes afs-fuse — no FUSE library needed)
cargo build

# Full build (requires FUSE library installed)
cargo build --workspace
```

### Docker (full build + tests, no host dependencies)

```sh
docker build -f Dockerfile.test -t afs-test .
docker run --rm --privileged afs-test
```

## Installation

After building, the binaries are in `target/debug/` (or `target/release/` with `--release`):

```sh
cargo build --workspace --release

# Install the CLI
cp target/release/afs /usr/local/bin/

# Install the daemons
cp target/release/afs-controller /usr/local/bin/
cp target/release/afs-fuse /usr/local/bin/
```

## Usage

### 1. Start the Controller

```sh
# With defaults (listens on 0.0.0.0:9100, SQLite at /var/lib/afs/controller.db)
afs-controller

# With config file
afs-controller controller.toml
```

`controller.toml`:

```toml
[server]
listen = "0.0.0.0:9100"

[storage]
db_path = "/var/lib/afs/controller.db"
```

### 2. Start the FUSE Server (on each agent host)

```sh
# With defaults (listens on 0.0.0.0:9101, controller at 127.0.0.1:9100)
afs-fuse

# With config file
afs-fuse fuse-server.toml
```

`fuse-server.toml`:

```toml
[server]
listen = "0.0.0.0:9101"
controller_addr = "controller-host:9100"
```

### 3. Register a Storage Backend

```sh
# Local storage
afs fs add my-storage --type local --base-path /data/afs

# NFS storage
afs fs add shared-nfs --type nfs \
  --nfs-server 10.0.0.1 \
  --nfs-path /exports/afs \
  --mount-path /mnt/nfs/afs
```

### 4. Create and Share a Directory

```sh
# Create a directory (returns id + access_key)
afs dir create --fs my-storage
# Output: id=a1b2c3...  access_key=d4e5f6...

# Mount it locally
afs dir mount a1b2c3... --key d4e5f6... --mountpoint /mnt/afs/shared

# Share the id + key with another agent, who mounts on their host:
afs dir mount a1b2c3... --key d4e5f6... --mountpoint /mnt/afs/shared

# Read-only mount
afs dir mount a1b2c3... --key d4e5f6... --mountpoint /mnt/afs/readonly --readonly
```

### 5. Use the Shared Directory

Once mounted, the directory behaves like a normal local filesystem:

```sh
echo "hello from agent 1" > /mnt/afs/shared/message.txt

# On another agent host (same dir mounted):
cat /mnt/afs/shared/message.txt
# hello from agent 1
```

### 6. Revoke Access and Clean Up

```sh
# Force-unmount all sessions for a directory (e.g., if an agent is offline)
afs dir revoke a1b2c3... --key d4e5f6...

# Unmount locally
afs dir unmount /mnt/afs/shared

# Delete the directory
afs dir delete a1b2c3... --key d4e5f6...

# List directories
afs dir list
afs dir list --fs my-storage
```

### CLI Reference

```
afs fs add <name> --type <local|nfs> [options]    # Register a storage backend
afs fs remove <name>                               # Unregister a storage backend
afs fs list                                        # List storage backends

afs dir create --fs <fs-name>                      # Create a shared directory
afs dir delete <id> --key <access_key>             # Delete a directory
afs dir mount <id> --key <key> --mountpoint <path> [--readonly]
afs dir unmount <mountpoint>                       # Unmount a directory
afs dir revoke <id> --key <access_key>             # Force-unmount all sessions
afs dir list [--fs <fs-name>]                      # List directories

Global options:
  --controller <addr>     Controller address (default: 127.0.0.1:9100)
  --fuse-server <addr>    FUSE server address (default: 127.0.0.1:9101)
```

## Testing

```sh
# Unit + integration tests (default members, no FUSE needed)
cargo test

# Full workspace tests (requires FUSE library)
cargo test --workspace

# Full tests in Docker (unit + integration + E2E)
docker build -f Dockerfile.test -t afs-test .
docker run --rm --privileged afs-test
```

## Project Structure

```
afs/
├── proto/afs.proto          # gRPC/protobuf definitions
├── afs-controller/          # Controller service (gRPC + SQLite)
├── afs-fuse/                # FUSE server (gRPC + fuser)
├── afs-storage/             # StorageBackend trait + implementations
├── afs-common/              # Shared error types
├── afs-cli/                 # CLI binary (afs)
├── tests/e2e.sh             # E2E tests (shell, runs in Docker)
├── docs/architecture.md     # Architecture documentation
└── Dockerfile.test          # Docker test environment
```

## License

TBD
