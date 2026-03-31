pub mod config;
pub mod filesystem;
pub mod service;

pub mod proto {
    tonic::include_proto!("afs");
}
