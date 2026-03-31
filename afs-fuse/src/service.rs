use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tonic::{Request, Response, Status};

use afs_storage::local::LocalStorage;
use afs_storage::nfs::NfsStorage;
use afs_storage::StorageBackend;

use crate::filesystem::{AfsFilesystem, MountPermission};
use crate::proto::controller_service_client::ControllerServiceClient;
use crate::proto::fuse_service_server::FuseService;
use crate::proto::*;

struct ActiveMount {
    id: String,
    mountpoint: String,
    permission: MountPermission,
    _session: fuser::BackgroundSession,
}

pub struct FuseServiceImpl {
    controller_addr: String,
    mounts: Mutex<HashMap<String, ActiveMount>>,
}

impl FuseServiceImpl {
    pub fn new(controller_addr: String) -> Self {
        Self {
            controller_addr,
            mounts: Mutex::new(HashMap::new()),
        }
    }

    async fn validate_with_controller(
        &self,
        id: &str,
        access_key: &str,
        controller_addr: &str,
    ) -> Result<ValidateTokenResponse, Status> {
        let addr = if controller_addr.is_empty() {
            &self.controller_addr
        } else {
            controller_addr
        };

        let endpoint = format!("http://{}", addr);
        let mut client = ControllerServiceClient::connect(endpoint)
            .await
            .map_err(|e| Status::unavailable(format!("cannot reach controller: {}", e)))?;

        let resp = client
            .validate_token(ValidateTokenRequest {
                id: id.to_string(),
                access_key: access_key.to_string(),
            })
            .await?
            .into_inner();

        if !resp.valid {
            return Err(Status::permission_denied("invalid id or access key"));
        }

        Ok(resp)
    }

    fn create_backend(
        fs_type: &str,
        config: &HashMap<String, String>,
    ) -> Result<Arc<dyn StorageBackend>, Status> {
        match fs_type {
            "local" => {
                let base_path = config
                    .get("base_path")
                    .ok_or_else(|| Status::internal("missing base_path in fs config"))?;
                Ok(Arc::new(LocalStorage::new(PathBuf::from(base_path))))
            }
            "nfs" => {
                let mount_path = config
                    .get("mount_path")
                    .ok_or_else(|| Status::internal("missing mount_path in fs config"))?;
                Ok(Arc::new(NfsStorage::new(PathBuf::from(mount_path))))
            }
            _ => Err(Status::internal(format!("unsupported fs type: {}", fs_type))),
        }
    }
}

#[tonic::async_trait]
impl FuseService for FuseServiceImpl {
    async fn mount(
        &self,
        request: Request<MountRequest>,
    ) -> Result<Response<MountResponse>, Status> {
        let req = request.into_inner();
        if req.id.is_empty() || req.access_key.is_empty() || req.mountpoint.is_empty() {
            return Err(Status::invalid_argument(
                "id, access_key, and mountpoint are required",
            ));
        }

        // Check if already mounted at this mountpoint
        {
            let mounts = self.mounts.lock().unwrap();
            if mounts.contains_key(&req.mountpoint) {
                return Err(Status::already_exists(format!(
                    "already mounted at {}",
                    req.mountpoint
                )));
            }
        }

        // Validate token with controller
        let validation = self
            .validate_with_controller(&req.id, &req.access_key, &req.controller_addr)
            .await?;

        let permission = if req.readonly || validation.permission == Permission::ReadOnly as i32 {
            MountPermission::ReadOnly
        } else {
            MountPermission::ReadWrite
        };

        // Create storage backend
        let backend = Self::create_backend(&validation.fs_type, &validation.fs_config)?;

        // Init dir if needed (lazy creation)
        if !backend
            .dir_exists(&req.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
        {
            backend
                .init_dir(&req.id)
                .await
                .map_err(|e| Status::internal(format!("failed to init dir: {}", e)))?;
        }

        // Create mountpoint directory
        tokio::fs::create_dir_all(&req.mountpoint)
            .await
            .map_err(|e| Status::internal(format!("failed to create mountpoint: {}", e)))?;

        // Create and mount FUSE filesystem
        let handle = tokio::runtime::Handle::current();
        let fs = AfsFilesystem::new(backend, req.id.clone(), permission, handle);

        let config = AfsFilesystem::mount_config();

        let session = fuser::spawn_mount2(fs, &req.mountpoint, &config)
            .map_err(|e| Status::internal(format!("failed to mount FUSE: {}", e)))?;

        let mount = ActiveMount {
            id: req.id,
            mountpoint: req.mountpoint.clone(),
            permission,
            _session: session,
        };

        self.mounts
            .lock()
            .unwrap()
            .insert(req.mountpoint.clone(), mount);

        Ok(Response::new(MountResponse {
            mountpoint: req.mountpoint,
        }))
    }

    async fn unmount(
        &self,
        request: Request<UnmountRequest>,
    ) -> Result<Response<UnmountResponse>, Status> {
        let req = request.into_inner();
        if req.mountpoint.is_empty() {
            return Err(Status::invalid_argument("mountpoint is required"));
        }

        let mut mounts = self.mounts.lock().unwrap();
        if mounts.remove(&req.mountpoint).is_none() {
            return Err(Status::not_found(format!(
                "no mount at {}",
                req.mountpoint
            )));
        }
        // BackgroundSession is dropped, which unmounts the filesystem

        Ok(Response::new(UnmountResponse {}))
    }

    async fn list_mounts(
        &self,
        _request: Request<ListMountsRequest>,
    ) -> Result<Response<ListMountsResponse>, Status> {
        let mounts = self.mounts.lock().unwrap();
        let mount_infos = mounts
            .values()
            .map(|m| MountInfo {
                id: m.id.clone(),
                mountpoint: m.mountpoint.clone(),
                permission: match m.permission {
                    MountPermission::ReadOnly => Permission::ReadOnly as i32,
                    MountPermission::ReadWrite => Permission::ReadWrite as i32,
                },
            })
            .collect();

        Ok(Response::new(ListMountsResponse {
            mounts: mount_infos,
        }))
    }
}
