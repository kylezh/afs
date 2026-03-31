use std::collections::HashMap;

use anyhow::{bail, Result};

pub mod proto {
    tonic::include_proto!("afs");
}

use proto::controller_service_client::ControllerServiceClient;
use proto::*;

pub async fn add(
    controller_addr: &str,
    name: &str,
    fs_type: &str,
    base_path: Option<&str>,
    nfs_server: Option<&str>,
    nfs_path: Option<&str>,
    mount_path: Option<&str>,
) -> Result<()> {
    let mut config = HashMap::new();
    match fs_type {
        "local" => {
            let path = base_path.ok_or_else(|| anyhow::anyhow!("--base-path is required for local filesystem"))?;
            config.insert("base_path".to_string(), path.to_string());
        }
        "nfs" => {
            if let Some(s) = nfs_server {
                config.insert("nfs_server".to_string(), s.to_string());
            }
            if let Some(p) = nfs_path {
                config.insert("nfs_path".to_string(), p.to_string());
            }
            let mp = mount_path.ok_or_else(|| anyhow::anyhow!("--mount-path is required for NFS filesystem"))?;
            config.insert("mount_path".to_string(), mp.to_string());
        }
        _ => bail!("unsupported filesystem type: {}. Use 'local' or 'nfs'", fs_type),
    }

    let mut client = connect_controller(controller_addr).await?;
    let resp = client
        .register_fs(RegisterFsRequest {
            name: name.to_string(),
            fs_type: fs_type.to_string(),
            config,
        })
        .await?
        .into_inner();

    println!("Registered filesystem: {}", resp.name);
    Ok(())
}

pub async fn remove(controller_addr: &str, name: &str) -> Result<()> {
    let mut client = connect_controller(controller_addr).await?;
    client
        .unregister_fs(UnregisterFsRequest {
            name: name.to_string(),
        })
        .await?;

    println!("Unregistered filesystem: {}", name);
    Ok(())
}

pub async fn list(controller_addr: &str) -> Result<()> {
    let mut client = connect_controller(controller_addr).await?;
    let resp = client.list_fs(ListFsRequest {}).await?.into_inner();

    if resp.filesystems.is_empty() {
        println!("No filesystems registered.");
        return Ok(());
    }

    println!("{:<20} {:<10} {:<30} {}", "NAME", "TYPE", "CONFIG", "CREATED");
    println!("{}", "-".repeat(80));
    for fs in &resp.filesystems {
        let config_str: String = fs
            .config
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join(", ");
        println!("{:<20} {:<10} {:<30} {}", fs.name, fs.fs_type, config_str, fs.created_at);
    }

    Ok(())
}

async fn connect_controller(
    addr: &str,
) -> Result<ControllerServiceClient<tonic::transport::Channel>> {
    let endpoint = format!("http://{}", addr);
    let client = ControllerServiceClient::connect(endpoint).await?;
    Ok(client)
}
