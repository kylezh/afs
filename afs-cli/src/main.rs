mod commands;

use anyhow::Result;
use clap::Parser;
use commands::{Cli, Commands, DirCommands, FsCommands};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Fs { command } => match command {
            FsCommands::Add {
                name,
                r#type,
                base_path,
                nfs_server,
                nfs_path,
                mount_path,
            } => {
                commands::fs::add(
                    &cli.controller,
                    &name,
                    &r#type,
                    base_path.as_deref(),
                    nfs_server.as_deref(),
                    nfs_path.as_deref(),
                    mount_path.as_deref(),
                )
                .await?;
            }
            FsCommands::Remove { name } => {
                commands::fs::remove(&cli.controller, &name).await?;
            }
            FsCommands::List => {
                commands::fs::list(&cli.controller).await?;
            }
        },
        Commands::Dir { command } => match command {
            DirCommands::Create { fs } => {
                commands::dir::create(&cli.controller, &fs).await?;
            }
            DirCommands::Delete { id, key } => {
                commands::dir::delete(&cli.controller, &id, &key).await?;
            }
            DirCommands::Mount {
                id,
                key,
                mountpoint,
                readonly,
            } => {
                commands::dir::mount(
                    &cli.fuse_server,
                    &cli.controller,
                    &id,
                    &key,
                    &mountpoint,
                    readonly,
                )
                .await?;
            }
            DirCommands::Unmount { mountpoint } => {
                commands::dir::unmount(&cli.fuse_server, &mountpoint).await?;
            }
            DirCommands::Revoke { id, key } => {
                commands::dir::revoke(&cli.controller, &id, &key).await?;
            }
            DirCommands::List { fs } => {
                commands::dir::list(&cli.controller, fs.as_deref()).await?;
            }
        },
    }

    Ok(())
}
