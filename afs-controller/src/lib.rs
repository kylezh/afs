pub mod config;
pub mod db;
pub mod service;

pub mod proto {
    tonic::include_proto!("afs");
}
