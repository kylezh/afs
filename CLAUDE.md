# afs (AgentFS)

FUSE-based shared filesystem for AI agents. Agents create shared directories via a central controller, then mount them locally via per-host FUSE servers.

## Project Structure

Rust workspace with 5 crates:

- `afs-controller` — centralized metadata/auth service (gRPC + SQLite)
- `afs-fuse` — per-host FUSE server daemon (gRPC + fuser)
- `afs-storage` — `StorageBackend` trait and implementations (local, NFS)
- `afs-common` — shared error types
- `afs-cli` — CLI binary (`afs`)

Protobuf definitions are in `proto/afs.proto`.

## Build & Test

```sh
cargo build            # builds default members (excludes afs-fuse on macOS without macFUSE)
cargo build --workspace  # builds all crates (requires FUSE: macFUSE on macOS, libfuse3-dev on Linux)
cargo test             # runs all tests for default members
```

Full workspace build and test in Docker (Linux with native FUSE):

```sh
docker build -f Dockerfile.test -t afs-test .
docker run --rm --privileged afs-test
```
