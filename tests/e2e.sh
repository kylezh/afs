#!/usr/bin/env bash
set -euo pipefail

# ── afs (AgentFS) End-to-End Tests (Local Storage) ──
# Simulates multiple agents sharing directories via the CLI.
# Requires: FUSE support (run in Docker with --privileged)

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

STORAGE_BASE="$(mktemp -d)"
source "$SCRIPT_DIR/e2e-lib.sh"

trap 'cleanup; rm -rf "$STORAGE_BASE"' EXIT

start_services

# ── Test 1: Local Filesystem Registration ──────────────

echo "── Test 1: Filesystem registration ──"

$AFS fs add testfs --type local --base-path "$STORAGE_BASE"
output=$($AFS fs list)
assert_contains "$output" "testfs" "fs appears in list"
assert_contains "$output" "local" "fs type is local"
echo ""

run_tests
