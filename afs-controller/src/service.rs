use std::collections::HashMap;
use std::sync::Arc;

use tonic::{Request, Response, Status};

use crate::db::Database;
use crate::proto::controller_service_server::ControllerService;
use crate::proto::*;

pub struct ControllerServiceImpl {
    db: Arc<Database>,
}

impl ControllerServiceImpl {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[tonic::async_trait]
impl ControllerService for ControllerServiceImpl {
    async fn register_fs(
        &self,
        request: Request<RegisterFsRequest>,
    ) -> Result<Response<RegisterFsResponse>, Status> {
        let req = request.into_inner();
        if req.name.is_empty() {
            return Err(Status::invalid_argument("name is required"));
        }
        if req.fs_type.is_empty() {
            return Err(Status::invalid_argument("fs_type is required"));
        }

        let config_json =
            serde_json::to_string(&req.config).map_err(|e| Status::internal(e.to_string()))?;

        self.db
            .register_fs(&req.name, &req.fs_type, &config_json)
            .map_err(|e| Status::already_exists(e.to_string()))?;

        Ok(Response::new(RegisterFsResponse {
            name: req.name,
        }))
    }

    async fn unregister_fs(
        &self,
        request: Request<UnregisterFsRequest>,
    ) -> Result<Response<UnregisterFsResponse>, Status> {
        let req = request.into_inner();
        if req.name.is_empty() {
            return Err(Status::invalid_argument("name is required"));
        }

        match self.db.unregister_fs(&req.name) {
            Ok(true) => Ok(Response::new(UnregisterFsResponse {})),
            Ok(false) => Err(Status::not_found(format!("fs '{}' not found", req.name))),
            Err(e) => Err(Status::failed_precondition(e.to_string())),
        }
    }

    async fn list_fs(
        &self,
        _request: Request<ListFsRequest>,
    ) -> Result<Response<ListFsResponse>, Status> {
        let records = self
            .db
            .list_fs()
            .map_err(|e| Status::internal(e.to_string()))?;

        let filesystems = records
            .into_iter()
            .map(|r| {
                let config: HashMap<String, String> =
                    serde_json::from_str(&r.config).unwrap_or_default();
                FsInfo {
                    name: r.name,
                    fs_type: r.fs_type,
                    config,
                    created_at: r.created_at,
                }
            })
            .collect();

        Ok(Response::new(ListFsResponse { filesystems }))
    }

    async fn create_dir(
        &self,
        request: Request<CreateDirRequest>,
    ) -> Result<Response<CreateDirResponse>, Status> {
        let req = request.into_inner();
        if req.fs_name.is_empty() {
            return Err(Status::invalid_argument("fs_name is required"));
        }

        let record = self
            .db
            .create_dir(&req.fs_name)
            .map_err(|e| Status::not_found(e.to_string()))?;

        Ok(Response::new(CreateDirResponse {
            id: record.id,
            access_key: record.access_key,
            permission: permission_from_str(&record.permission) as i32,
        }))
    }

    async fn delete_dir(
        &self,
        request: Request<DeleteDirRequest>,
    ) -> Result<Response<DeleteDirResponse>, Status> {
        let req = request.into_inner();
        if req.id.is_empty() || req.access_key.is_empty() {
            return Err(Status::invalid_argument("id and access_key are required"));
        }

        match self.db.delete_dir(&req.id, &req.access_key) {
            Ok(true) => Ok(Response::new(DeleteDirResponse {})),
            Ok(false) => Err(Status::not_found("dir not found or invalid access key")),
            Err(e) => Err(Status::internal(e.to_string())),
        }
    }

    async fn validate_token(
        &self,
        request: Request<ValidateTokenRequest>,
    ) -> Result<Response<ValidateTokenResponse>, Status> {
        let req = request.into_inner();
        if req.id.is_empty() || req.access_key.is_empty() {
            return Err(Status::invalid_argument("id and access_key are required"));
        }

        match self.db.validate_token(&req.id, &req.access_key) {
            Ok(Some((dir, fs))) => {
                let fs_config: HashMap<String, String> =
                    serde_json::from_str(&fs.config).unwrap_or_default();
                Ok(Response::new(ValidateTokenResponse {
                    valid: true,
                    permission: permission_from_str(&dir.permission) as i32,
                    fs_type: fs.fs_type,
                    fs_config,
                }))
            }
            Ok(None) => Ok(Response::new(ValidateTokenResponse {
                valid: false,
                permission: Permission::ReadOnly as i32,
                fs_type: String::new(),
                fs_config: HashMap::new(),
            })),
            Err(e) => Err(Status::internal(e.to_string())),
        }
    }

    async fn list_dirs(
        &self,
        request: Request<ListDirsRequest>,
    ) -> Result<Response<ListDirsResponse>, Status> {
        let req = request.into_inner();
        let fs_filter = if req.fs_name.is_empty() {
            None
        } else {
            Some(req.fs_name.as_str())
        };

        let records = self
            .db
            .list_dirs(fs_filter)
            .map_err(|e| Status::internal(e.to_string()))?;

        let dirs = records
            .into_iter()
            .map(|r| DirInfo {
                id: r.id,
                fs_name: r.fs_name,
                permission: permission_from_str(&r.permission) as i32,
                status: r.status,
                created_at: r.created_at,
            })
            .collect();

        Ok(Response::new(ListDirsResponse { dirs }))
    }
}

fn permission_from_str(s: &str) -> Permission {
    match s {
        "READ_ONLY" => Permission::ReadOnly,
        _ => Permission::ReadWrite,
    }
}
