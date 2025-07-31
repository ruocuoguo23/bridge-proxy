use std::fmt;
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
#[derive(Clone)]
pub struct Config {
    /// Listening address
    pub address: String,
    /// Connection timeout (seconds)
    pub timeout_seconds: u64,
    /// Maximum number of idle connections
    pub max_idle_connections: usize,
    /// Idle connection timeout (seconds)
    pub idle_timeout_seconds: u64,
}

impl Config {
    /// Get connection timeout duration
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds)
    }
    
    /// Get idle connection timeout duration
    pub fn idle_timeout(&self) -> Duration {
        Duration::from_secs(self.idle_timeout_seconds)
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("address", &self.address)
            .field("timeout", &format!("{}s", self.timeout_seconds))
            .field("max_idle_connections", &self.max_idle_connections)
            .field("idle_timeout", &format!("{}s", self.idle_timeout_seconds))
            .finish()
    }
}
