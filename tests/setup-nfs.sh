#!/usr/bin/env bash
set -euo pipefail

# ── NFS Server Setup for E2E Tests ──
# Starts nfs-ganesha (userspace NFS server) inside the Docker container
# and mounts the export at /mnt/nfs-test.
# Requires: nfs-ganesha, nfs-ganesha-vfs, nfs-common, rpcbind, dbus

NFS_EXPORT="/srv/nfs-export"
NFS_MOUNT="/mnt/nfs-test"

echo "══ NFS Server Setup ══"
echo ""

# Create export directory on tmpfs (overlayfs doesn't support name_to_handle_at
# which ganesha's VFS FSAL requires)
mkdir -p "$NFS_EXPORT"
mount -t tmpfs tmpfs "$NFS_EXPORT"
chmod 777 "$NFS_EXPORT"

# Write ganesha config — export at NFSv4 pseudo root
mkdir -p /etc/ganesha
cat > /etc/ganesha/ganesha.conf << 'EOF'
NFS_CORE_PARAM {
    NFS_Port = 2049;
    MNT_Port = 20048;
    NLM_Port = 32803;
}

EXPORT {
    Export_Id = 1;
    Path = /srv/nfs-export;
    Pseudo = /;
    Protocols = 3,4;
    Transports = TCP;
    Access_Type = RW;
    Squash = No_Root_Squash;
    FSAL {
        Name = VFS;
    }
}

NFS_KRB5 {
    Active_krb5 = false;
}

LOG {
    Default_Log_Level = WARN;
}
EOF

# Start services in order (order matters for ganesha)
echo "Starting dbus..."
mkdir -p /var/run/dbus /var/run/ganesha
dbus-daemon --system --fork 2>/dev/null || true
sleep 1

echo "Starting rpcbind..."
rpcbind 2>/dev/null || true
sleep 1

echo "Starting nfs-ganesha..."
ganesha.nfsd -f /etc/ganesha/ganesha.conf -L /var/log/ganesha/ganesha.log -N NIV_WARN
sleep 2

# Wait for NFS server to be ready (poll rpcinfo for nfs service)
echo "Waiting for NFS server..."
for i in $(seq 1 30); do
    if rpcinfo -p localhost 2>/dev/null | grep -q nfs; then
        echo "  NFS service registered"
        break
    fi
    if [ "$i" -eq 30 ]; then
        echo "  FATAL: NFS server did not start within 30 seconds"
        cat /var/log/ganesha/ganesha.log 2>/dev/null || true
        exit 1
    fi
    sleep 1
done

# Additional wait for exports to be ready
sleep 2

# Mount the NFS export with retry
mkdir -p "$NFS_MOUNT"
echo "Mounting NFS export at $NFS_MOUNT..."
for i in $(seq 1 10); do
    if mount -t nfs -o vers=4,nolock localhost:/ "$NFS_MOUNT" 2>/dev/null; then
        echo "  NFS mounted successfully"
        break
    fi
    if [ "$i" -eq 10 ]; then
        echo "  FATAL: NFS mount failed after 10 attempts"
        echo "  Ganesha log:"
        tail -20 /var/log/ganesha/ganesha.log 2>/dev/null || true
        exit 1
    fi
    sleep 1
done

# Verify the mount works
echo "test" > "$NFS_MOUNT/.nfs-verify"
if [ "$(cat "$NFS_MOUNT/.nfs-verify")" = "test" ]; then
    rm "$NFS_MOUNT/.nfs-verify"
    echo "  NFS mount verified"
else
    echo "  FATAL: NFS mount verification failed"
    exit 1
fi

echo ""
echo "NFS server ready at $NFS_MOUNT"
echo ""
