use anyhow::Result;
use clap::Parser;
use log::{error, info};
use std::sync::Arc;
use std::path::PathBuf;

mod proxy;
mod tunnel;
mod config;
mod metrics;

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

    /// Metrics server address (overrides config file)
    #[clap(short, long)]
    metrics_address: Option<String>,
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
    if let Some(metrics_address) = args.metrics_address {
        config.metrics_address = Some(metrics_address);
    }

    let config = Arc::new(config);

    info!("Proxy configuration: {:?}", config);

    // Initialize metrics
    if let Err(e) = metrics::init_metrics("bridge_proxy") {
        error!("Failed to initialize metrics: {}", e);
        return Err(e.into());
    }
    info!("Metrics initialized successfully");

    // Start metrics server if configured
    let _metrics_handle = if let Some(ref metrics_addr) = config.metrics_address {
        match metrics_addr.parse() {
            Ok(addr) => {
                match metrics::start_metrics_server(addr).await {
                    Ok(handle) => {
                        info!("Metrics server started on {}", metrics_addr);
                        Some(handle)
                    }
                    Err(e) => {
                        error!("Failed to start metrics server: {}", e);
                        return Err(e);
                    }
                }
            }
            Err(e) => {
                error!("Invalid metrics address '{}': {}", metrics_addr, e);
                return Err(e.into());
            }
        }
    } else {
        info!("Metrics server disabled (no metrics address configured)");
        None
    };

    // Create and start proxy service
    let proxy = TunnelProxy::new(config);
    
    info!("Network Bridge Proxy is listening on {}", proxy.config.address);

    if let Err(e) = proxy.start().await {
        error!("Proxy service error: {}", e);
        return Err(e.into());
    }

    Ok(())
}
