#!/usr/bin/env bash
set -euo pipefail

# ── afs (AgentFS) End-to-End Tests (NFS Storage) ──
# Runs the same test suite as e2e.sh but against an NFS-backed filesystem.
# Requires: NFS server running + mounted at /mnt/nfs-test (see setup-nfs.sh)

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

STORAGE_BASE="/mnt/nfs-test"
source "$SCRIPT_DIR/e2e-lib.sh"

nfs_cleanup() {
    # Kill NFS daemons BEFORE cleanup (cleanup calls exit)
    umount /mnt/nfs-test 2>/dev/null || true
    killall ganesha.nfsd 2>/dev/null || true
    killall rpcbind 2>/dev/null || true
    cleanup
}
trap 'nfs_cleanup' EXIT

start_services

# ── Test 1: NFS Filesystem Registration ────────────────

echo "── Test 1: NFS filesystem registration ──"

$AFS fs add testfs --type nfs --nfs-server 127.0.0.1 --nfs-path /export --mount-path "$STORAGE_BASE"
output=$($AFS fs list)
assert_contains "$output" "testfs" "fs appears in list"
assert_contains "$output" "nfs" "fs type is nfs"
echo ""

run_tests
