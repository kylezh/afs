use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Server;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use afs_fuse::config::FuseServerConfig;
use afs_fuse::proto::controller_service_client::ControllerServiceClient;
use afs_fuse::proto::fuse_service_server::FuseServiceServer;
use afs_fuse::proto::*;
use afs_fuse::service::FuseServiceImpl;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let config = match std::env::args().nth(1) {
        Some(path) => FuseServerConfig::from_file(&path)?,
        None => FuseServerConfig::default(),
    };

    let addr = config.server.listen.parse()?;
    info!("fuse-server listening on {}", addr);
    info!("controller at {}", config.server.controller_addr);

    let service = Arc::new(FuseServiceImpl::new(config.server.controller_addr.clone()));

    // Spawn session stream manager (heartbeat + command reader)
    let stream_service = service.clone();
    tokio::spawn(session_stream_loop(
        config.server.controller_addr,
        stream_service,
    ));

    Server::builder()
        .add_service(FuseServiceServer::from_arc(service))
        .serve(addr)
        .await?;

    Ok(())
}

/// Persistent loop: connect to Controller's SessionStream, send heartbeats, receive commands.
/// Reconnects with exponential backoff on disconnect.
async fn session_stream_loop(controller_addr: String, service: Arc<FuseServiceImpl>) {
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(30);

    loop {
        info!(
            "connecting session stream to controller at {}",
            controller_addr
        );
        match connect_session_stream(&controller_addr, &service).await {
            Ok(()) => {
                info!("session stream disconnected normally");
                backoff = Duration::from_secs(1);
            }
            Err(e) => {
                warn!("session stream error: {}", e);
            }
        }

        info!("reconnecting session stream in {:?}", backoff);
        tokio::time::sleep(backoff).await;
        backoff = std::cmp::min(backoff * 2, max_backoff);
    }
}

async fn connect_session_stream(
    controller_addr: &str,
    service: &Arc<FuseServiceImpl>,
) -> Result<()> {
    let endpoint = format!("http://{}", controller_addr);
    let mut client = ControllerServiceClient::connect(endpoint).await?;

    // Create channel for sending heartbeats
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    let outbound = ReceiverStream::new(rx);

    // Send initial heartbeat before starting the stream (so Controller learns our stream_id)
    let stream_id = service.stream_id().to_string();

    // Start the bidi stream
    let response = client.session_stream(outbound).await?;
    let mut inbound = response.into_inner();

    // Send first heartbeat immediately
    let initial_mounts: Vec<MountStatus> = service
        .mount_statuses()
        .into_iter()
        .map(|(dir_id, mountpoint)| MountStatus { dir_id, mountpoint })
        .collect();
    tx.send(Heartbeat {
        stream_id: stream_id.clone(),
        mounts: initial_mounts,
    })
    .await?;

    // Spawn heartbeat sender (periodic)
    let heartbeat_stream_id = stream_id.clone();
    let heartbeat_service = service.clone();
    let heartbeat_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;

            let mount_statuses: Vec<MountStatus> = heartbeat_service
                .mount_statuses()
                .into_iter()
                .map(|(dir_id, mountpoint)| MountStatus { dir_id, mountpoint })
                .collect();

            let heartbeat = Heartbeat {
                stream_id: heartbeat_stream_id.clone(),
                mounts: mount_statuses,
            };

            if tx.send(heartbeat).await.is_err() {
                break; // Channel closed
            }
        }
    });

    // Read commands from controller
    while let Some(cmd) = inbound.message().await? {
        if let Some(session_command::Command::ForceUnmount(force_unmount)) = cmd.command {
            info!("received force unmount for {}", force_unmount.mountpoint);
            if service.force_unmount(&force_unmount.mountpoint) {
                info!("force unmounted {}", force_unmount.mountpoint);
            } else {
                warn!(
                    "force unmount requested for unknown mountpoint: {}",
                    force_unmount.mountpoint
                );
            }
        }
    }

    heartbeat_handle.abort();
    Ok(())
}
