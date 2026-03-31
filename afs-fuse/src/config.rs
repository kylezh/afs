use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct FuseServerConfig {
    pub server: ServerConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub listen: String,
    pub controller_addr: String,
}

impl Default for FuseServerConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                listen: "0.0.0.0:9101".to_string(),
                controller_addr: "127.0.0.1:9100".to_string(),
            },
        }
    }
}

impl FuseServerConfig {
    pub fn from_file(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }
}
