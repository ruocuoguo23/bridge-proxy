use anyhow::Result;
use clap::Parser;
use log::{error, info};
use std::sync::Arc;

mod proxy;
mod tunnel;
mod config;

use config::Config;
use proxy::TunnelProxy;

#[derive(Parser, Debug)]
#[clap(author, version, about = "HTTP/HTTPS Tunnel Proxy for Blockchain Services")]
struct CliArgs {
    /// Address to listen on
    #[clap(short, long, default_value = "127.0.0.1:8080")]
    address: String,
    
    /// Enable verbose logging
    #[clap(short, long)]
    verbose: bool,
    
    /// Connection timeout in seconds
    #[clap(short, long, default_value = "10")]
    timeout: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    env_logger::init();

    info!("Starting network-bridge-proxy tunnel service...");

    // Parse command line arguments
    let args = CliArgs::parse();
    
    // Create proxy configuration
    let config = Arc::new(Config {
        address: args.address.clone(),
        verbose: args.verbose,
        timeout_seconds: args.timeout,
        max_idle_connections: 100,
        idle_timeout_seconds: 90,
    });
    
    info!("Proxy configuration: {:?}", config);
    
    // Create and start proxy service
    let proxy = TunnelProxy::new(config);
    
    info!("Network Bridge Proxy is listening on {}", args.address);
    
    if let Err(e) = proxy.start().await {
        error!("Proxy service error: {}", e);
        return Err(e.into());
    }

    Ok(())
}
