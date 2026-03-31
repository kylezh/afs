use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rand::Rng;
use tonic::{Request, Response, Status};
use tracing::warn;

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
    session_id: String,
    _session: fuser::BackgroundSession,
}

pub struct FuseServiceImpl {
    controller_addr: String,
    stream_id: String,
    mounts: Arc<Mutex<HashMap<String, ActiveMount>>>,
}

impl FuseServiceImpl {
    pub fn new(controller_addr: String) -> Self {
        let stream_id = generate_stream_id();
        Self {
            controller_addr,
            stream_id,
            mounts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// Get snapshot of current mounts as (dir_id, mountpoint) pairs for heartbeat.
    pub fn mount_statuses(&self) -> Vec<(String, String)> {
        let mounts = self.mounts.lock().unwrap();
        mounts
            .values()
            .map(|m| (m.id.clone(), m.mountpoint.clone()))
            .collect()
    }

    /// Force unmount a mountpoint (called by session stream command reader).
    pub fn force_unmount(&self, mountpoint: &str) -> bool {
        let mut mounts = self.mounts.lock().unwrap();
        mounts.remove(mountpoint).is_some()
        // BackgroundSession dropped → FUSE unmounted
    }

    async fn register_session_with_controller(
        &self,
        dir_id: &str,
        mountpoint: &str,
    ) -> Option<String> {
        let endpoint = format!("http://{}", &self.controller_addr);
        match ControllerServiceClient::connect(endpoint).await {
            Ok(mut client) => {
                match client
                    .register_session(RegisterSessionRequest {
                        dir_id: dir_id.to_string(),
                        mountpoint: mountpoint.to_string(),
                        stream_id: self.stream_id.clone(),
                    })
                    .await
                {
                    Ok(resp) => Some(resp.into_inner().session_id),
                    Err(e) => {
                        warn!("failed to register session: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                warn!("failed to connect to controller for session registration: {}", e);
                None
            }
        }
    }

    async fn deregister_session_with_controller(&self, session_id: &str) {
        if session_id.is_empty() {
            return;
        }
        let endpoint = format!("http://{}", &self.controller_addr);
        match ControllerServiceClient::connect(endpoint).await {
            Ok(mut client) => {
                if let Err(e) = client
                    .deregister_session(DeregisterSessionRequest {
                        session_id: session_id.to_string(),
                    })
                    .await
                {
                    warn!("failed to deregister session {}: {}", session_id, e);
                }
            }
            Err(e) => {
                warn!("failed to connect to controller for session deregistration: {}", e);
            }
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

        // Register session with controller (best-effort)
        let session_id = self
            .register_session_with_controller(&req.id, &req.mountpoint)
            .await
            .unwrap_or_default();

        let mount = ActiveMount {
            id: req.id,
            mountpoint: req.mountpoint.clone(),
            permission,
            session_id,
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

        let removed = {
            let mut mounts = self.mounts.lock().unwrap();
            mounts.remove(&req.mountpoint)
        };

        match removed {
            Some(mount) => {
                // Deregister session with controller (best-effort)
                self.deregister_session_with_controller(&mount.session_id)
                    .await;
                // BackgroundSession is dropped, which unmounts the filesystem
                Ok(Response::new(UnmountResponse {}))
            }
            None => Err(Status::not_found(format!(
                "no mount at {}",
                req.mountpoint
            ))),
        }
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

/// Generate a 32-character random hex string for stream identification.
fn generate_stream_id() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 16] = rng.gen();
    hex::encode(bytes)
}
