use anyhow::Result;
use clap::Parser;
use log::{error, info};
use std::sync::Arc;
use std::path::PathBuf;

mod proxy;
mod tunnel;
mod config;

use config::{Config, LOG_CONFIG};
use proxy::TunnelProxy;

#[derive(Parser, Debug)]
#[clap(author, version, about = "HTTP/HTTPS Tunnel Proxy for Blockchain Services")]
struct CliArgs {
    /// Path to the configuration file
    #[clap(short, long, parse(from_os_str))]
    config: Option<PathBuf>,

    /// Address to listen on (overrides config file)
    #[clap(short, long)]
    address: Option<String>,

    /// Connection timeout in seconds (overrides config file)
    #[clap(short, long)]
    timeout: Option<u64>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize log4rs
    let log_config = serde_yaml::from_str::<log4rs::config::RawConfig>(&LOG_CONFIG.to_string()).unwrap();
    log4rs::init_raw_config(log_config).unwrap();

    info!("Starting network-bridge-proxy tunnel service...");

    // Parse command line arguments
    let args = CliArgs::parse();
    
    // Load configuration from file or use default
    let config_path = args.config.unwrap_or_else(|| "bridge-proxy-config.yaml".into());
    let mut config = match Config::load_config(&config_path) {
        Ok(config) => {
            info!("Configuration loaded from {}", config_path.display());
            config
        }
        Err(e) => {
            info!("Failed to load config from {}: {}, using default configuration", config_path.display(), e);
            Config::default()
        }
    };

    // Override config with command line arguments if provided
    if let Some(address) = args.address {
        config.address = address;
    }
    if let Some(timeout) = args.timeout {
        config.timeout_seconds = timeout;
    }

    let config = Arc::new(config);

    info!("Proxy configuration: {:?}", config);
    
    // Create and start proxy service
    let proxy = TunnelProxy::new(config);
    
    info!("Network Bridge Proxy is listening on {}", proxy.config.address);

    if let Err(e) = proxy.start().await {
        error!("Proxy service error: {}", e);
        return Err(e.into());
    }

    Ok(())
}
