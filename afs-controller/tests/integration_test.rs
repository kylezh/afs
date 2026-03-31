use std::sync::Arc;

use afs_controller::db::Database;
use afs_controller::proto::controller_service_server::ControllerServiceServer;
use afs_controller::proto::*;
use afs_controller::service::ControllerServiceImpl;

use tonic::transport::Server;

async fn start_controller() -> (String, tokio::task::JoinHandle<()>) {
    let db = Arc::new(Database::open(":memory:").unwrap());
    let service = ControllerServiceImpl::new(db);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let addr_str = addr.to_string();

    let handle = tokio::spawn(async move {
        Server::builder()
            .add_service(ControllerServiceServer::new(service))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    // Give server time to start
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    (addr_str, handle)
}

async fn connect(
    addr: &str,
) -> controller_service_client::ControllerServiceClient<tonic::transport::Channel> {
    let endpoint = format!("http://{}", addr);
    controller_service_client::ControllerServiceClient::connect(endpoint)
        .await
        .unwrap()
}

#[tokio::test]
async fn test_full_workflow() {
    let (addr, _handle) = start_controller().await;
    let mut client = connect(&addr).await;

    // 1. Register a filesystem
    let resp = client
        .register_fs(RegisterFsRequest {
            name: "local-test".to_string(),
            fs_type: "local".to_string(),
            config: [("base_path".to_string(), "/tmp/afs-test".to_string())]
                .into_iter()
                .collect(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.name, "local-test");

    // 2. List filesystems
    let resp = client
        .list_fs(ListFsRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.filesystems.len(), 1);
    assert_eq!(resp.filesystems[0].name, "local-test");
    assert_eq!(resp.filesystems[0].fs_type, "local");

    // 3. Create a directory
    let resp = client
        .create_dir(CreateDirRequest {
            fs_name: "local-test".to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.id.len(), 32);
    assert_eq!(resp.access_key.len(), 32);
    assert_eq!(resp.permission, Permission::ReadWrite as i32);
    let dir_id = resp.id;
    let access_key = resp.access_key;

    // 4. Validate token (valid)
    let resp = client
        .validate_token(ValidateTokenRequest {
            id: dir_id.clone(),
            access_key: access_key.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(resp.valid);
    assert_eq!(resp.permission, Permission::ReadWrite as i32);
    assert_eq!(resp.fs_type, "local");
    assert_eq!(resp.fs_config.get("base_path").unwrap(), "/tmp/afs-test");

    // 5. Validate token (invalid key)
    let resp = client
        .validate_token(ValidateTokenRequest {
            id: dir_id.clone(),
            access_key: "wrong-key".to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(!resp.valid);

    // 6. List directories
    let resp = client
        .list_dirs(ListDirsRequest {
            fs_name: "local-test".to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.dirs.len(), 1);
    assert_eq!(resp.dirs[0].id, dir_id);

    // 7. Create another directory
    let resp2 = client
        .create_dir(CreateDirRequest {
            fs_name: "local-test".to_string(),
        })
        .await
        .unwrap()
        .into_inner();

    // 8. List all directories
    let resp = client
        .list_dirs(ListDirsRequest {
            fs_name: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.dirs.len(), 2);

    // 9. Delete the second directory
    client
        .delete_dir(DeleteDirRequest {
            id: resp2.id.clone(),
            access_key: resp2.access_key.clone(),
        })
        .await
        .unwrap();

    // 10. Verify deleted dir is gone from list
    let resp = client
        .list_dirs(ListDirsRequest {
            fs_name: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.dirs.len(), 1);

    // 11. Validate deleted dir token fails
    let resp = client
        .validate_token(ValidateTokenRequest {
            id: resp2.id,
            access_key: resp2.access_key,
        })
        .await
        .unwrap()
        .into_inner();
    assert!(!resp.valid);

    // 12. Delete the first directory
    client
        .delete_dir(DeleteDirRequest {
            id: dir_id.clone(),
            access_key: access_key.clone(),
        })
        .await
        .unwrap();

    // 13. Unregister the filesystem (no active dirs now)
    client
        .unregister_fs(UnregisterFsRequest {
            name: "local-test".to_string(),
        })
        .await
        .unwrap();

    // 14. Verify empty
    let resp = client
        .list_fs(ListFsRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(resp.filesystems.is_empty());
}

#[tokio::test]
async fn test_cannot_unregister_fs_with_active_dirs() {
    let (addr, _handle) = start_controller().await;
    let mut client = connect(&addr).await;

    client
        .register_fs(RegisterFsRequest {
            name: "local-test".to_string(),
            fs_type: "local".to_string(),
            config: [("base_path".to_string(), "/tmp/afs-test".to_string())]
                .into_iter()
                .collect(),
        })
        .await
        .unwrap();

    client
        .create_dir(CreateDirRequest {
            fs_name: "local-test".to_string(),
        })
        .await
        .unwrap();

    let err = client
        .unregister_fs(UnregisterFsRequest {
            name: "local-test".to_string(),
        })
        .await
        .unwrap_err();

    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
}

#[tokio::test]
async fn test_create_dir_on_nonexistent_fs() {
    let (addr, _handle) = start_controller().await;
    let mut client = connect(&addr).await;

    let err = client
        .create_dir(CreateDirRequest {
            fs_name: "nonexistent".to_string(),
        })
        .await
        .unwrap_err();

    assert_eq!(err.code(), tonic::Code::NotFound);
}

#[tokio::test]
async fn test_multiple_filesystems() {
    let (addr, _handle) = start_controller().await;
    let mut client = connect(&addr).await;

    // Register two filesystems
    client
        .register_fs(RegisterFsRequest {
            name: "local-dev".to_string(),
            fs_type: "local".to_string(),
            config: [("base_path".to_string(), "/tmp/dev".to_string())]
                .into_iter()
                .collect(),
        })
        .await
        .unwrap();

    client
        .register_fs(RegisterFsRequest {
            name: "nfs-shared".to_string(),
            fs_type: "nfs".to_string(),
            config: [("mount_path".to_string(), "/mnt/nfs".to_string())]
                .into_iter()
                .collect(),
        })
        .await
        .unwrap();

    // Create dirs on each
    let dir1 = client
        .create_dir(CreateDirRequest {
            fs_name: "local-dev".to_string(),
        })
        .await
        .unwrap()
        .into_inner();

    let dir2 = client
        .create_dir(CreateDirRequest {
            fs_name: "nfs-shared".to_string(),
        })
        .await
        .unwrap()
        .into_inner();

    // List all dirs
    let resp = client
        .list_dirs(ListDirsRequest {
            fs_name: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.dirs.len(), 2);

    // List filtered by fs
    let resp = client
        .list_dirs(ListDirsRequest {
            fs_name: "local-dev".to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.dirs.len(), 1);
    assert_eq!(resp.dirs[0].fs_name, "local-dev");

    // Validate tokens return correct fs info
    let resp = client
        .validate_token(ValidateTokenRequest {
            id: dir1.id,
            access_key: dir1.access_key,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.fs_type, "local");

    let resp = client
        .validate_token(ValidateTokenRequest {
            id: dir2.id,
            access_key: dir2.access_key,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.fs_type, "nfs");
}
