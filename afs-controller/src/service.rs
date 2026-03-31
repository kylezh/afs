use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex as TokioMutex};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{info, warn};

use crate::db::Database;
use crate::proto::controller_service_server::ControllerService;
use crate::proto::*;

type StreamSender = mpsc::Sender<Result<SessionCommand, Status>>;

pub struct ControllerServiceImpl {
    db: Arc<Database>,
    streams: Arc<TokioMutex<HashMap<String, StreamSender>>>,
}

impl ControllerServiceImpl {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            streams: Arc::new(TokioMutex::new(HashMap::new())),
        }
    }

    /// Revoke all sessions for a directory. Returns (sessions_revoked, errors).
    async fn revoke_sessions_for_dir(&self, dir_id: &str) -> (i32, i32) {
        let sessions = match self.db.get_sessions_for_dir(dir_id) {
            Ok(s) => s,
            Err(e) => {
                warn!("failed to get sessions for dir {}: {}", dir_id, e);
                return (0, 0);
            }
        };

        let mut revoked = 0i32;
        let mut errors = 0i32;

        let streams = self.streams.lock().await;
        for session in &sessions {
            if let Some(tx) = streams.get(&session.stream_id) {
                let cmd = SessionCommand {
                    command: Some(session_command::Command::ForceUnmount(ForceUnmount {
                        mountpoint: session.mountpoint.clone(),
                    })),
                };
                if tx.send(Ok(cmd)).await.is_ok() {
                    revoked += 1;
                } else {
                    warn!(
                        "stream {} disconnected while revoking session {}",
                        session.stream_id, session.session_id
                    );
                    errors += 1;
                }
            } else {
                warn!(
                    "stream {} not found for session {}",
                    session.stream_id, session.session_id
                );
                errors += 1;
            }

            // Delete session record regardless
            if let Err(e) = self.db.deregister_session(&session.session_id) {
                warn!("failed to deregister session {}: {}", session.session_id, e);
            }
        }

        (revoked, errors)
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

        // Revoke all sessions first (best-effort)
        let (revoked, errors) = self.revoke_sessions_for_dir(&req.id).await;
        if revoked > 0 || errors > 0 {
            info!(
                "delete_dir {}: revoked {} session(s), {} error(s)",
                req.id, revoked, errors
            );
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

    // --- Session management ---

    async fn register_session(
        &self,
        request: Request<RegisterSessionRequest>,
    ) -> Result<Response<RegisterSessionResponse>, Status> {
        let req = request.into_inner();
        if req.dir_id.is_empty() || req.mountpoint.is_empty() || req.stream_id.is_empty() {
            return Err(Status::invalid_argument(
                "dir_id, mountpoint, and stream_id are required",
            ));
        }

        let record = self
            .db
            .register_session(&req.dir_id, &req.stream_id, &req.mountpoint)
            .map_err(|e| Status::internal(e.to_string()))?;

        info!(
            "registered session {} for dir {} on stream {}",
            record.session_id, req.dir_id, req.stream_id
        );

        Ok(Response::new(RegisterSessionResponse {
            session_id: record.session_id,
        }))
    }

    async fn deregister_session(
        &self,
        request: Request<DeregisterSessionRequest>,
    ) -> Result<Response<DeregisterSessionResponse>, Status> {
        let req = request.into_inner();
        if req.session_id.is_empty() {
            return Err(Status::invalid_argument("session_id is required"));
        }

        match self.db.deregister_session(&req.session_id) {
            Ok(true) => {
                info!("deregistered session {}", req.session_id);
                Ok(Response::new(DeregisterSessionResponse {}))
            }
            Ok(false) => Err(Status::not_found(format!(
                "session {} not found",
                req.session_id
            ))),
            Err(e) => Err(Status::internal(e.to_string())),
        }
    }

    async fn revoke_dir(
        &self,
        request: Request<RevokeDirRequest>,
    ) -> Result<Response<RevokeDirResponse>, Status> {
        let req = request.into_inner();
        if req.id.is_empty() || req.access_key.is_empty() {
            return Err(Status::invalid_argument("id and access_key are required"));
        }

        // Validate access key
        match self.db.validate_token(&req.id, &req.access_key) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Err(Status::permission_denied("invalid id or access key"));
            }
            Err(e) => {
                return Err(Status::internal(e.to_string()));
            }
        }

        let (sessions_revoked, errors) = self.revoke_sessions_for_dir(&req.id).await;
        info!(
            "revoke_dir {}: {} session(s) revoked, {} error(s)",
            req.id, sessions_revoked, errors
        );

        Ok(Response::new(RevokeDirResponse {
            sessions_revoked,
            errors,
        }))
    }

    type SessionStreamStream =
        Pin<Box<dyn tokio_stream::Stream<Item = Result<SessionCommand, Status>> + Send>>;

    async fn session_stream(
        &self,
        request: Request<Streaming<Heartbeat>>,
    ) -> Result<Response<Self::SessionStreamStream>, Status> {
        let mut inbound = request.into_inner();
        let db = self.db.clone();
        let streams = self.streams.clone();

        // We'll learn the stream_id from the first heartbeat
        let (tx, rx) = mpsc::channel(32);

        tokio::spawn(async move {
            let mut stream_id: Option<String> = None;

            while let Ok(Some(heartbeat)) = inbound.message().await {
                let sid = heartbeat.stream_id.clone();
                if sid.is_empty() {
                    warn!("received heartbeat with empty stream_id, skipping");
                    continue;
                }

                // Register stream on first heartbeat
                if stream_id.is_none() {
                    stream_id = Some(sid.clone());
                    streams.lock().await.insert(sid.clone(), tx.clone());
                    info!("stream {} connected", sid);
                }

                // Reconcile sessions from heartbeat
                let mounts: Vec<(&str, &str)> = heartbeat
                    .mounts
                    .iter()
                    .map(|m| (m.dir_id.as_str(), m.mountpoint.as_str()))
                    .collect();

                if let Err(e) = db.reconcile_sessions_from_heartbeat(&sid, &mounts) {
                    warn!("failed to reconcile heartbeat for stream {}: {}", sid, e);
                }
            }

            // Stream dropped — clean up
            if let Some(sid) = stream_id {
                info!("stream {} disconnected, cleaning up sessions", sid);
                streams.lock().await.remove(&sid);
                if let Err(e) = db.delete_sessions_by_stream(&sid) {
                    warn!("failed to clean up sessions for stream {}: {}", sid, e);
                }
            }
        });

        let output_stream = ReceiverStream::new(rx);
        Ok(Response::new(
            Box::pin(output_stream) as Self::SessionStreamStream
        ))
    }
}

fn permission_from_str(s: &str) -> Permission {
    match s {
        "READ_ONLY" => Permission::ReadOnly,
        _ => Permission::ReadWrite,
    }
}
