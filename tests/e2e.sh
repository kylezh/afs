#!/usr/bin/env bash
set -euo pipefail

# ── afs (AgentFS) End-to-End Tests ──
# Simulates multiple agents sharing directories via the CLI.
# Requires: FUSE support (run in Docker with --privileged)

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

AFS="${PROJECT_DIR}/target/debug/afs"
CONTROLLER="${PROJECT_DIR}/target/debug/afs-controller"
FUSE_SERVER="${PROJECT_DIR}/target/debug/afs-fuse"

STORAGE_BASE="$(mktemp -d)"
DB_PATH="$(mktemp)"
MOUNT_BASE="$(mktemp -d)"

PASS=0
FAIL=0
PIDS=()
MOUNTS=()

# ── Helpers ──────────────────────────────────────────

pass() { PASS=$((PASS + 1)); echo "  PASS: $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  FAIL: $1"; }

assert_eq() {
    local actual="$1" expected="$2" msg="$3"
    if [ "$actual" = "$expected" ]; then pass "$msg"
    else fail "$msg (expected '$expected', got '$actual')"; fi
}

assert_contains() {
    local haystack="$1" needle="$2" msg="$3"
    if echo "$haystack" | grep -qF "$needle"; then pass "$msg"
    else fail "$msg (expected to contain '$needle')"; fi
}

assert_file_content() {
    local path="$1" expected="$2" msg="$3"
    local actual
    if [ ! -f "$path" ]; then
        fail "$msg (file not found: $path)"
        return
    fi
    actual="$(cat "$path")"
    if [ "$actual" = "$expected" ]; then pass "$msg"
    else fail "$msg (expected '$expected', got '$actual')"; fi
}

assert_write_fails() {
    local path="$1" msg="$2"
    if echo "test" > "$path" 2>/dev/null; then
        fail "$msg (write succeeded, expected failure)"
        rm -f "$path" 2>/dev/null || true
    else
        pass "$msg"
    fi
}

wait_for_port() {
    local port=$1 name=$2
    for _ in $(seq 1 50); do
        if bash -c "echo > /dev/tcp/127.0.0.1/$port" 2>/dev/null; then
            echo "  $name ready on port $port"
            return 0
        fi
        sleep 0.2
    done
    echo "  FATAL: $name did not start on port $port"
    exit 1
}

track_mount() { MOUNTS+=("$1"); }

parse_dir_id()  { echo "$1" | grep 'ID:' | awk '{print $2}'; }
parse_dir_key() { echo "$1" | grep 'Access Key:' | awk '{print $3}'; }

cleanup() {
    set +e
    echo ""
    echo "── Cleanup ──"
    for (( i=${#MOUNTS[@]}-1; i>=0; i-- )); do
        fusermount3 -u "${MOUNTS[$i]}" 2>/dev/null
    done
    for pid in "${PIDS[@]}"; do
        kill "$pid" 2>/dev/null
        wait "$pid" 2>/dev/null
    done
    sleep 0.5
    rm -rf "$STORAGE_BASE" "$MOUNT_BASE" "$DB_PATH" /tmp/afs-controller.toml /tmp/afs-fuse.toml 2>/dev/null
    echo ""
    echo "══════════════════════════"
    echo "  Results: $PASS passed, $FAIL failed"
    echo "══════════════════════════"
    [ "$FAIL" -eq 0 ] && exit 0 || exit 1
}
trap cleanup EXIT

# ── Start Services ───────────────────────────────────

echo "══ afs E2E Tests ══"
echo ""

cat > /tmp/afs-controller.toml << EOF
[server]
listen = "0.0.0.0:9100"
[storage]
db_path = "$DB_PATH"
EOF

cat > /tmp/afs-fuse.toml << EOF
[server]
listen = "0.0.0.0:9101"
controller_addr = "127.0.0.1:9100"
EOF

echo "Starting controller..."
$CONTROLLER /tmp/afs-controller.toml &
PIDS+=($!)

echo "Starting fuse-server..."
$FUSE_SERVER /tmp/afs-fuse.toml &
PIDS+=($!)

wait_for_port 9100 "controller"
wait_for_port 9101 "fuse-server"
echo ""

# ── Test 1: Filesystem Registration ─────────────────

echo "── Test 1: Filesystem registration ──"

$AFS fs add testfs --type local --base-path "$STORAGE_BASE"
output=$($AFS fs list)
assert_contains "$output" "testfs" "fs appears in list"
assert_contains "$output" "local" "fs type is local"
echo ""

# ── Test 2: Agent 1 Creates Dir, Mounts, Writes ─────

echo "── Test 2: Agent 1 creates dir and writes ──"

create_output=$($AFS dir create --fs testfs)
DIR1_ID=$(parse_dir_id "$create_output")
DIR1_KEY=$(parse_dir_key "$create_output")
assert_contains "$create_output" "read-write" "dir created with read-write"

MOUNT_AGENT1="$MOUNT_BASE/agent1"
mkdir -p "$MOUNT_AGENT1"
$AFS dir mount "$DIR1_ID" --key "$DIR1_KEY" --mountpoint "$MOUNT_AGENT1"
track_mount "$MOUNT_AGENT1"
sleep 1

echo "hello from agent 1" > "$MOUNT_AGENT1/message.txt"
assert_file_content "$MOUNT_AGENT1/message.txt" "hello from agent 1" "agent1 can write and read"
echo ""

# ── Test 3: Agent 2 Mounts Same Dir, Reads & Writes ─

echo "── Test 3: Agent 2 reads agent 1's data ──"

MOUNT_AGENT2="$MOUNT_BASE/agent2"
mkdir -p "$MOUNT_AGENT2"
$AFS dir mount "$DIR1_ID" --key "$DIR1_KEY" --mountpoint "$MOUNT_AGENT2"
track_mount "$MOUNT_AGENT2"
sleep 1

assert_file_content "$MOUNT_AGENT2/message.txt" "hello from agent 1" "agent2 reads agent1 file"

echo "reply from agent 2" > "$MOUNT_AGENT2/reply.txt"
sleep 1
assert_file_content "$MOUNT_AGENT1/reply.txt" "reply from agent 2" "agent1 sees agent2 reply"
echo ""

# ── Test 4: Read-Only Mount ─────────────────────────

echo "── Test 4: Read-only mount ──"

MOUNT_RO="$MOUNT_BASE/readonly"
mkdir -p "$MOUNT_RO"
$AFS dir mount "$DIR1_ID" --key "$DIR1_KEY" --mountpoint "$MOUNT_RO" --readonly
track_mount "$MOUNT_RO"
sleep 1

assert_file_content "$MOUNT_RO/message.txt" "hello from agent 1" "readonly mount can read"

assert_write_fails "$MOUNT_RO/blocked.txt" "readonly mount rejects write"

if mkdir "$MOUNT_RO/blocked_dir" 2>/dev/null; then
    fail "readonly mount rejects mkdir (mkdir succeeded)"
    rmdir "$MOUNT_RO/blocked_dir" 2>/dev/null || true
else
    pass "readonly mount rejects mkdir"
fi

if rm "$MOUNT_RO/message.txt" 2>/dev/null; then
    fail "readonly mount rejects unlink (rm succeeded)"
else
    pass "readonly mount rejects unlink"
fi
echo ""

# ── Test 5: Invalid Access Key Rejected ──────────────

echo "── Test 5: Invalid access key ──"

MOUNT_BAD="$MOUNT_BASE/badkey"
mkdir -p "$MOUNT_BAD"
if $AFS dir mount "$DIR1_ID" --key "0000000000000000000000000000dead" --mountpoint "$MOUNT_BAD" 2>/dev/null; then
    fail "invalid key rejected (mount succeeded)"
else
    pass "invalid key rejected"
fi
echo ""

# ── Test 6: Multiple Dirs, Isolation ─────────────────

echo "── Test 6: Multiple dirs, isolation ──"

create2_output=$($AFS dir create --fs testfs)
DIR2_ID=$(parse_dir_id "$create2_output")
DIR2_KEY=$(parse_dir_key "$create2_output")

MOUNT_DIR2="$MOUNT_BASE/dir2"
mkdir -p "$MOUNT_DIR2"
$AFS dir mount "$DIR2_ID" --key "$DIR2_KEY" --mountpoint "$MOUNT_DIR2"
track_mount "$MOUNT_DIR2"
sleep 1

echo "dir2 data" > "$MOUNT_DIR2/data.txt"
assert_file_content "$MOUNT_DIR2/data.txt" "dir2 data" "second dir works"

if [ -f "$MOUNT_DIR2/message.txt" ]; then
    fail "dirs are isolated (dir1 file leaked to dir2)"
else
    pass "dirs are isolated"
fi

list_output=$($AFS dir list --fs testfs)
assert_contains "$list_output" "$DIR1_ID" "dir list shows dir1"
assert_contains "$list_output" "$DIR2_ID" "dir list shows dir2"
echo ""

# ── Test 7: Revoke with Active Mounts ──────────────

echo "── Test 7: Revoke with active mounts ──"

# Create a new dir specifically for revocation testing
create_rev_output=$($AFS dir create --fs testfs)
REV_ID=$(parse_dir_id "$create_rev_output")
REV_KEY=$(parse_dir_key "$create_rev_output")

MOUNT_REV1="$MOUNT_BASE/rev1"
MOUNT_REV2="$MOUNT_BASE/rev2"
mkdir -p "$MOUNT_REV1" "$MOUNT_REV2"
$AFS dir mount "$REV_ID" --key "$REV_KEY" --mountpoint "$MOUNT_REV1"
track_mount "$MOUNT_REV1"
$AFS dir mount "$REV_ID" --key "$REV_KEY" --mountpoint "$MOUNT_REV2"
track_mount "$MOUNT_REV2"
sleep 1

echo "revoke data" > "$MOUNT_REV1/test.txt"
assert_file_content "$MOUNT_REV2/test.txt" "revoke data" "both mounts see data before revoke"

# Revoke — should force unmount both mounts
revoke_output=$($AFS dir revoke "$REV_ID" --key "$REV_KEY")
assert_contains "$revoke_output" "Revoked" "revoke command succeeds"
sleep 1

# After revoke, mounting at the same points should fail (already unmounted by revoke)
# Verify the revoked dir is still active (revoke ≠ delete)
list_rev=$($AFS dir list --fs testfs)
assert_contains "$list_rev" "$REV_ID" "dir still active after revoke"

# Clean up revoke dir
$AFS dir delete "$REV_ID" --key "$REV_KEY" && pass "delete revoked dir" || fail "delete revoked dir"
# Remove from tracked mounts since revoke already unmounted
MOUNTS=("${MOUNTS[@]/$MOUNT_REV1}")
MOUNTS=("${MOUNTS[@]/$MOUNT_REV2}")
echo ""

# ── Test 8: Revoke with No Sessions ────────────────

echo "── Test 8: Revoke with no sessions ──"

create_empty_output=$($AFS dir create --fs testfs)
EMPTY_ID=$(parse_dir_id "$create_empty_output")
EMPTY_KEY=$(parse_dir_key "$create_empty_output")

# Revoke a dir with zero active mounts — should succeed with 0 sessions
revoke_empty=$($AFS dir revoke "$EMPTY_ID" --key "$EMPTY_KEY")
assert_contains "$revoke_empty" "Revoked 0" "revoke with no sessions"

$AFS dir delete "$EMPTY_ID" --key "$EMPTY_KEY" && pass "delete empty dir" || fail "delete empty dir"
echo ""

# ── Test 9: Revoke with Bad Key ─────────────────────

echo "── Test 9: Revoke with bad key ──"

create_badkey_output=$($AFS dir create --fs testfs)
BADKEY_ID=$(parse_dir_id "$create_badkey_output")
BADKEY_KEY=$(parse_dir_key "$create_badkey_output")

if $AFS dir revoke "$BADKEY_ID" --key "0000000000000000000000000000dead" 2>/dev/null; then
    fail "revoke with bad key rejected (succeeded)"
else
    pass "revoke with bad key rejected"
fi

$AFS dir delete "$BADKEY_ID" --key "$BADKEY_KEY" && pass "cleanup bad key dir" || fail "cleanup bad key dir"
echo ""

# ── Test 10: Delete Also Revokes ─────────────────────

echo "── Test 10: Delete also revokes ──"

create_del_output=$($AFS dir create --fs testfs)
DEL_ID=$(parse_dir_id "$create_del_output")
DEL_KEY=$(parse_dir_key "$create_del_output")

MOUNT_DEL="$MOUNT_BASE/deltest"
mkdir -p "$MOUNT_DEL"
$AFS dir mount "$DEL_ID" --key "$DEL_KEY" --mountpoint "$MOUNT_DEL"
track_mount "$MOUNT_DEL"
sleep 1

echo "delete test" > "$MOUNT_DEL/data.txt"
assert_file_content "$MOUNT_DEL/data.txt" "delete test" "mount works before delete"

# Delete should revoke the active mount first, then soft-delete
$AFS dir delete "$DEL_ID" --key "$DEL_KEY" && pass "delete with active mount" || fail "delete with active mount"
sleep 1

# Dir should no longer appear in list
list_del=$($AFS dir list --fs testfs)
if echo "$list_del" | grep -qF "$DEL_ID"; then
    fail "deleted dir removed from list"
else
    pass "deleted dir removed from list"
fi
# Remove from tracked mounts since delete already revoked
MOUNTS=("${MOUNTS[@]/$MOUNT_DEL}")
echo ""

# ── Test 11: Full Cleanup Lifecycle ─────────────────

echo "── Test 11: Cleanup lifecycle ──"

$AFS dir unmount "$MOUNT_AGENT1" && pass "unmount agent1" || fail "unmount agent1"
$AFS dir unmount "$MOUNT_AGENT2" && pass "unmount agent2" || fail "unmount agent2"
$AFS dir unmount "$MOUNT_RO" && pass "unmount readonly" || fail "unmount readonly"
$AFS dir unmount "$MOUNT_DIR2" && pass "unmount dir2" || fail "unmount dir2"

# Clear tracked mounts since we already unmounted
MOUNTS=()

$AFS dir delete "$DIR1_ID" --key "$DIR1_KEY" && pass "delete dir1" || fail "delete dir1"
$AFS dir delete "$DIR2_ID" --key "$DIR2_KEY" && pass "delete dir2" || fail "delete dir2"

list_after=$($AFS dir list --fs testfs)
assert_contains "$list_after" "No directories found" "dirs deleted from list"

$AFS fs remove testfs && pass "fs unregistered" || fail "fs unregistered"

list_fs_after=$($AFS fs list)
assert_contains "$list_fs_after" "No filesystems registered" "fs removed from list"
echo ""
