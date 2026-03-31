use std::sync::Arc;

use anyhow::Result;
use tonic::transport::Server;
use tracing_subscriber::EnvFilter;

use afs_controller::config::ControllerConfig;
use afs_controller::db::Database;
use afs_controller::proto::controller_service_server::ControllerServiceServer;
use afs_controller::service::ControllerServiceImpl;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let config = match std::env::args().nth(1) {
        Some(path) => ControllerConfig::from_file(&path)?,
        None => ControllerConfig::default(),
    };

    tracing::info!("opening database at {}", config.storage.db_path);
    let db = Arc::new(Database::open(&config.storage.db_path)?);

    let addr = config.server.listen.parse()?;
    tracing::info!("controller listening on {}", addr);

    let service = ControllerServiceImpl::new(db);

    Server::builder()
        .add_service(ControllerServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
