use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

pub const LOG_CONFIG: &str = r#"
refresh_rate: 30 seconds
appenders:
  stdout:
    kind: console
  file:
    kind: rolling_file
    path: "logs/bridge_proxy.log"
    policy:
      kind: compound
      trigger:
        kind: time
        interval: 1 day # rotate log file every day
      roller:
        kind: fixed_window
        pattern: "logs/bridge_proxy.{}.log"
        base: 1
        count: 10 # ten days logs will be kept
root:
  level: info
  appenders:
    - stdout
    - file
"#;

/// Proxy server configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// Listening address
    #[serde(rename = "Address")]
    pub address: String,
    /// Connection timeout (seconds)
    #[serde(rename = "TimeoutSeconds")]
    pub timeout_seconds: u64,
    /// Maximum number of idle connections
    #[serde(rename = "MaxIdleConnections")]
    pub max_idle_connections: usize,
    /// Idle connection timeout (seconds)
    #[serde(rename = "IdleTimeoutSeconds")]
    pub idle_timeout_seconds: u64,
}

impl Config {
    /// Load configuration from YAML file
    pub fn load_config<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn Error>> {
        let mut file = File::open(path)?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        let config: Config = serde_yaml::from_str(&contents)?;
        Ok(config)
    }

    /// Get connection timeout duration
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds)
    }
    
    /// Get idle connection timeout duration
    pub fn idle_timeout(&self) -> Duration {
        Duration::from_secs(self.idle_timeout_seconds)
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            address: "0.0.0.0:8080".to_string(),
            timeout_seconds: 10,
            max_idle_connections: 100,
            idle_timeout_seconds: 90,
        }
    }
}
