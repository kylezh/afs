use anyhow::Result;
use tonic::transport::Server;
use tracing_subscriber::EnvFilter;

use afs_fuse::config::FuseServerConfig;
use afs_fuse::proto::fuse_service_server::FuseServiceServer;
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
    tracing::info!("fuse-server listening on {}", addr);
    tracing::info!("controller at {}", config.server.controller_addr);

    let service = FuseServiceImpl::new(config.server.controller_addr);

    Server::builder()
        .add_service(FuseServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
