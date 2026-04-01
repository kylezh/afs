# CLI Reference

## Overview

afs provides three binaries:

| Binary | Description |
|--------|-------------|
| `afs` | Management CLI for filesystems and directories |
| `afs-controller` | Controller daemon (metadata + auth) |
| `afs-fuse` | FUSE server daemon (per-host mounts) |

## afs

```
afs [OPTIONS] <COMMAND>
```

### Global Options

| Option | Default | Description |
|--------|---------|-------------|
| `--controller <ADDR>` | `127.0.0.1:9100` | Controller gRPC address |
| `--fuse-server <ADDR>` | `127.0.0.1:9101` | FUSE server gRPC address |

### afs fs — Filesystem Management

#### afs fs add

Register a new storage backend.

```
afs fs add <NAME> --type <TYPE> [OPTIONS]
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `NAME` | Unique name for the filesystem (e.g., `my-storage`) |

**Options:**

| Option | Required | Description |
|--------|----------|-------------|
| `--type <TYPE>` | Yes | `local` or `nfs` |
| `--base-path <PATH>` | Yes (local) | Base directory for local storage |
| `--nfs-server <ADDR>` | No (nfs) | NFS server address |
| `--nfs-path <PATH>` | No (nfs) | NFS export path |
| `--mount-path <PATH>` | Yes (nfs) | Local mount point for the NFS share |

**Examples:**

```sh
# Local storage
afs fs add my-storage --type local --base-path /data/afs

# NFS storage
afs fs add shared-nfs --type nfs \
  --nfs-server 10.0.0.1 \
  --nfs-path /exports/afs \
  --mount-path /mnt/nfs/afs
```

#### afs fs remove

Unregister a storage backend.

```
afs fs remove <NAME>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `NAME` | Filesystem name to remove |

#### afs fs list

List all registered filesystems.

```
afs fs list
```

**Output columns:** `NAME`, `TYPE`, `CONFIG`, `CREATED`

### afs dir — Directory Management

#### afs dir create

Create a new shared directory. Returns the directory ID and access key.

```
afs dir create --fs <FS_NAME>
```

**Options:**

| Option | Required | Description |
|--------|----------|-------------|
| `--fs <FS_NAME>` | Yes | Filesystem to create the directory in |

**Output:**

```
Created directory:
  ID:         a1b2c3d4e5f6789012345678abcdef90
  Access Key: f0e1d2c3b4a596870123456789abcdef
  Permission: read-write
```

#### afs dir delete

Delete a shared directory. Requires the access key for authorization.

```
afs dir delete <ID> --key <ACCESS_KEY>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `ID` | Directory ID |

**Options:**

| Option | Required | Description |
|--------|----------|-------------|
| `--key <ACCESS_KEY>` | Yes | Access key for the directory |

#### afs dir mount

Mount a shared directory as a local FUSE filesystem. The actual storage directory is created on first mount (lazy creation).

```
afs dir mount <ID> --key <ACCESS_KEY> --mountpoint <PATH> [--readonly]
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `ID` | Directory ID |

**Options:**

| Option | Required | Description |
|--------|----------|-------------|
| `--key <ACCESS_KEY>` | Yes | Access key for the directory |
| `--mountpoint <PATH>` | Yes | Local path to mount on |
| `--readonly` | No | Mount as read-only (writes are rejected) |

**Examples:**

```sh
# Read-write mount
afs dir mount a1b2c3... --key d4e5f6... --mountpoint /mnt/afs/shared

# Read-only mount
afs dir mount a1b2c3... --key d4e5f6... --mountpoint /mnt/afs/readonly --readonly
```

#### afs dir unmount

Unmount a locally mounted directory.

```
afs dir unmount <MOUNTPOINT>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `MOUNTPOINT` | Path to unmount |

#### afs dir revoke

Force-unmount all active sessions (mounts) for a directory across all FUSE servers. Useful when an agent is offline or unresponsive.

```
afs dir revoke <ID> --key <ACCESS_KEY>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `ID` | Directory ID |

**Options:**

| Option | Required | Description |
|--------|----------|-------------|
| `--key <ACCESS_KEY>` | Yes | Access key for the directory |

**Output:**

```
Revoked 3 session(s) for directory: a1b2c3...
```

If some FUSE servers were unreachable:

```
Revoked 2 session(s) for directory: a1b2c3...
Warning: 1 session(s) could not be reached (cleaned up anyway)
```

#### afs dir list

List shared directories, optionally filtered by filesystem.

```
afs dir list [--fs <FS_NAME>]
```

**Options:**

| Option | Required | Description |
|--------|----------|-------------|
| `--fs <FS_NAME>` | No | Filter by filesystem name |

**Output columns:** `ID`, `FILESYSTEM`, `PERMISSION`, `STATUS`, `CREATED`

## afs-controller

Start the controller daemon.

```
afs-controller [CONFIG_FILE]
```

**Arguments:**

| Argument | Default | Description |
|----------|---------|-------------|
| `CONFIG_FILE` | (built-in defaults) | Path to TOML config file |

**Default configuration:**

| Setting | Default |
|---------|---------|
| `server.listen` | `0.0.0.0:9100` |
| `storage.db_path` | `/var/lib/afs/controller.db` |

**Config file format (`controller.toml`):**

```toml
[server]
listen = "0.0.0.0:9100"

[storage]
db_path = "/var/lib/afs/controller.db"
```

**Environment:**

| Variable | Description |
|----------|-------------|
| `RUST_LOG` | Log level filter (default: `info`). Example: `RUST_LOG=debug` |

## afs-fuse

Start the FUSE server daemon. Requires root for FUSE mount operations.

```
afs-fuse [CONFIG_FILE]
```

**Arguments:**

| Argument | Default | Description |
|----------|---------|-------------|
| `CONFIG_FILE` | (built-in defaults) | Path to TOML config file |

**Default configuration:**

| Setting | Default |
|---------|---------|
| `server.listen` | `0.0.0.0:9101` |
| `server.controller_addr` | `127.0.0.1:9100` |

**Config file format (`fuse-server.toml`):**

```toml
[server]
listen = "0.0.0.0:9101"
controller_addr = "controller-host:9100"
```

**Environment:**

| Variable | Description |
|----------|-------------|
| `RUST_LOG` | Log level filter (default: `info`). Example: `RUST_LOG=debug` |

The FUSE server maintains a persistent bidirectional session stream to the controller for heartbeats and force-unmount commands. It automatically reconnects with exponential backoff (1s to 30s) if the connection is lost.
