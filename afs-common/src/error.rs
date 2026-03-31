use std::fmt;

#[derive(Debug)]
pub enum AfsError {
    NotFound(String),
    AlreadyExists(String),
    PermissionDenied(String),
    InvalidArgument(String),
    Internal(String),
}

impl fmt::Display for AfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AfsError::NotFound(msg) => write!(f, "not found: {msg}"),
            AfsError::AlreadyExists(msg) => write!(f, "already exists: {msg}"),
            AfsError::PermissionDenied(msg) => write!(f, "permission denied: {msg}"),
            AfsError::InvalidArgument(msg) => write!(f, "invalid argument: {msg}"),
            AfsError::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for AfsError {}
