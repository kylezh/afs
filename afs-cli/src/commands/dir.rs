use anyhow::Result;

pub mod proto {
    tonic::include_proto!("afs");
}

use proto::controller_service_client::ControllerServiceClient;
use proto::fuse_service_client::FuseServiceClient;
use proto::*;

pub async fn create(controller_addr: &str, fs_name: &str) -> Result<()> {
    let mut client = connect_controller(controller_addr).await?;
    let resp = client
        .create_dir(CreateDirRequest {
            fs_name: fs_name.to_string(),
        })
        .await?
        .into_inner();

    println!("Created directory:");
    println!("  ID:         {}", resp.id);
    println!("  Access Key: {}", resp.access_key);
    println!("  Permission: {}", permission_str(resp.permission));
    Ok(())
}

pub async fn delete(controller_addr: &str, id: &str, access_key: &str) -> Result<()> {
    let mut client = connect_controller(controller_addr).await?;
    client
        .delete_dir(DeleteDirRequest {
            id: id.to_string(),
            access_key: access_key.to_string(),
        })
        .await?;

    println!("Deleted directory: {}", id);
    Ok(())
}

pub async fn mount(
    fuse_addr: &str,
    controller_addr: &str,
    id: &str,
    access_key: &str,
    mountpoint: &str,
    _readonly: bool,
) -> Result<()> {
    let mut client = connect_fuse(fuse_addr).await?;
    let resp = client
        .mount(MountRequest {
            id: id.to_string(),
            access_key: access_key.to_string(),
            mountpoint: mountpoint.to_string(),
            controller_addr: controller_addr.to_string(),
        })
        .await?
        .into_inner();

    println!("Mounted at: {}", resp.mountpoint);
    Ok(())
}

pub async fn unmount(fuse_addr: &str, mountpoint: &str) -> Result<()> {
    let mut client = connect_fuse(fuse_addr).await?;
    client
        .unmount(UnmountRequest {
            mountpoint: mountpoint.to_string(),
        })
        .await?;

    println!("Unmounted: {}", mountpoint);
    Ok(())
}

pub async fn list(controller_addr: &str, fs_name: Option<&str>) -> Result<()> {
    let mut client = connect_controller(controller_addr).await?;
    let resp = client
        .list_dirs(ListDirsRequest {
            fs_name: fs_name.unwrap_or("").to_string(),
        })
        .await?
        .into_inner();

    if resp.dirs.is_empty() {
        println!("No directories found.");
        return Ok(());
    }

    println!(
        "{:<36} {:<20} {:<12} {:<10} {}",
        "ID", "FILESYSTEM", "PERMISSION", "STATUS", "CREATED"
    );
    println!("{}", "-".repeat(100));
    for dir in &resp.dirs {
        println!(
            "{:<36} {:<20} {:<12} {:<10} {}",
            dir.id,
            dir.fs_name,
            permission_str(dir.permission),
            dir.status,
            dir.created_at,
        );
    }

    Ok(())
}

fn permission_str(p: i32) -> &'static str {
    if p == Permission::ReadOnly as i32 {
        "read-only"
    } else {
        "read-write"
    }
}

async fn connect_controller(
    addr: &str,
) -> Result<ControllerServiceClient<tonic::transport::Channel>> {
    let endpoint = format!("http://{}", addr);
    let client = ControllerServiceClient::connect(endpoint).await?;
    Ok(client)
}

async fn connect_fuse(addr: &str) -> Result<FuseServiceClient<tonic::transport::Channel>> {
    let endpoint = format!("http://{}", addr);
    let client = FuseServiceClient::connect(endpoint).await?;
    Ok(client)
}
