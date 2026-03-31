use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ControllerConfig {
    pub server: ServerConfig,
    pub storage: StorageConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub listen: String,
}

#[derive(Debug, Deserialize)]
pub struct StorageConfig {
    pub db_path: String,
}

impl Default for ControllerConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                listen: "0.0.0.0:9100".to_string(),
            },
            storage: StorageConfig {
                db_path: "/var/lib/afs/controller.db".to_string(),
            },
        }
    }
}

impl ControllerConfig {
    pub fn from_file(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }
}
