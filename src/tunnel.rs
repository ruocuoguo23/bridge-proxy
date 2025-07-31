use anyhow::{Result, anyhow};
use log::{error, info};
use std::time::Duration;
use tokio::io::{self, AsyncRead, AsyncWrite};
use tokio::time::timeout;

/// Create a bidirectional tunnel between two asynchronous streams
pub async fn create_tunnel<C, T>(
    mut client_stream: C,
    mut target_stream: T,
    conn_timeout: Duration,
) -> Result<()>
where
    C: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    // Use Tokio's built-in bidirectional copy function
    match timeout(conn_timeout, io::copy_bidirectional(&mut client_stream, &mut target_stream)).await {
        Ok(Ok((from_client, from_target))) => {
            info!(
                "Tunnel closed: {} bytes from client to target, {} bytes from target to client",
                from_client, from_target
            );
            Ok(())
        }
        Ok(Err(e)) => {
            error!("Tunnel error: {}", e);
            Err(anyhow!("Tunnel error: {}", e))
        }
        Err(_) => {
            error!("Tunnel timed out");
            Err(anyhow!("Tunnel timed out"))
        }
    }
}
