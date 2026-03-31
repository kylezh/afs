pub mod dir;
pub mod fs;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "afs", about = "AgentFS — shared filesystem for AI agents")]
pub struct Cli {
    /// Controller address
    #[arg(long, default_value = "127.0.0.1:9100", global = true)]
    pub controller: String,

    /// FUSE server address
    #[arg(long, default_value = "127.0.0.1:9101", global = true)]
    pub fuse_server: String,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Manage filesystem instances
    Fs {
        #[command(subcommand)]
        command: FsCommands,
    },
    /// Manage shared directories
    Dir {
        #[command(subcommand)]
        command: DirCommands,
    },
}

#[derive(Subcommand)]
pub enum FsCommands {
    /// Register a new filesystem
    Add {
        /// Unique name for the filesystem
        name: String,
        /// Filesystem type: local or nfs
        #[arg(long)]
        r#type: String,
        /// Base path for local filesystem
        #[arg(long)]
        base_path: Option<String>,
        /// NFS server address
        #[arg(long)]
        nfs_server: Option<String>,
        /// NFS export path
        #[arg(long)]
        nfs_path: Option<String>,
        /// Local mount path for NFS
        #[arg(long)]
        mount_path: Option<String>,
    },
    /// Unregister a filesystem
    Remove {
        /// Filesystem name
        name: String,
    },
    /// List registered filesystems
    List,
}

#[derive(Subcommand)]
pub enum DirCommands {
    /// Create a new shared directory
    Create {
        /// Filesystem to create the directory in
        #[arg(long)]
        fs: String,
    },
    /// Delete a shared directory
    Delete {
        /// Directory ID
        id: String,
        /// Access key
        #[arg(long)]
        key: String,
    },
    /// Mount a shared directory
    Mount {
        /// Directory ID
        id: String,
        /// Access key
        #[arg(long)]
        key: String,
        /// Local mount point
        #[arg(long)]
        mountpoint: String,
        /// Mount as read-only
        #[arg(long)]
        readonly: bool,
    },
    /// Unmount a shared directory
    Unmount {
        /// Mount point to unmount
        mountpoint: String,
    },
    /// List shared directories
    List {
        /// Filter by filesystem name
        #[arg(long)]
        fs: Option<String>,
    },
}
