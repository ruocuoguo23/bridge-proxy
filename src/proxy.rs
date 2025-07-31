use anyhow::{Context, Result};
use hyper::{Request, Response, StatusCode};
use hyper::body::{Bytes, Incoming};
use http_body_util::{Full};
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper::service::service_fn;
use log::{error, info};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use hyper::server::conn::http1::Builder as Http1Builder;

use crate::config::Config;
use crate::tunnel;

/// HTTP/HTTPS tunnel proxy service
pub struct TunnelProxy {
    config: Arc<Config>,
}

impl TunnelProxy {
    /// Create a new tunnel proxy
    pub fn new(config: Arc<Config>) -> Self {
        Self { config }
    }

    /// Start the proxy service
    pub async fn start(&self) -> Result<()> {
        let config = self.config.clone();

        // Bind address and start server
        let addr = self.config.address.parse::<SocketAddr>()
            .with_context(|| format!("Failed to parse address: {}", self.config.address))?;

        let listener = TcpListener::bind(addr).await?;
        info!("Tunnel proxy listening on {}", addr);

        loop {
            let (stream, _) = match listener.accept().await {
                Ok(accept) => accept,
                Err(e) => {
                    error!("Failed to accept connection: {}", e);
                    continue;
                }
            };

            let config = config.clone();

            // Spawn a task to handle each connection
            tokio::spawn(async move {
                if let Err(e) = Self::handle_connection(stream, config).await {
                    error!("Connection handling error: {}", e);
                }
            });
        }
    }

    /// Handle a single TCP connection
    async fn handle_connection(mut stream: TcpStream, config: Arc<Config>) -> Result<()> {
        // Read the first line to determine if it's a CONNECT request
        let mut buffer = [0; 4096];
        let n = stream.read(&mut buffer).await?;
        let request_data = &buffer[..n];
        let request_str = String::from_utf8_lossy(request_data);

        // Check if this is a CONNECT request
        if request_str.starts_with("CONNECT ") {
            // Handle CONNECT request directly
            Self::handle_connect_direct(stream, request_str.to_string(), config).await
        } else {
            // Handle as regular HTTP request using hyper
            // Put the data back by creating a new stream with the buffered data
            let mut full_stream = std::io::Cursor::new(request_data.to_vec());
            full_stream.set_position(0);

            // Create HTTP client for regular requests
            let http_client = Client::builder(TokioExecutor::new())
                .pool_idle_timeout(config.idle_timeout())
                .pool_max_idle_per_host(config.max_idle_connections)
                .build_http();

            let service = service_fn(move |req| {
                let http_client = http_client.clone();
                let config = config.clone();
                async move {
                    Self::handle_http(req, http_client, config).await
                }
            });

            // Create a combined stream with buffered data + original stream
            let combined_stream = CombinedStream::new(request_data.to_vec(), stream);
            let io = TokioIo::new(combined_stream);

            let conn = Http1Builder::new()
                .serve_connection(io, service)
                .await;

            if let Err(e) = conn {
                error!("HTTP connection error: {}", e);
            }
            Ok(())
        }
    }

    /// Handle CONNECT request directly at TCP level
    async fn handle_connect_direct(
        mut client_stream: TcpStream,
        request: String,
        config: Arc<Config>
    ) -> Result<()> {
        // Parse the CONNECT request to extract host and port
        let lines: Vec<&str> = request.lines().collect();
        if lines.is_empty() {
            return Err(anyhow::anyhow!("Invalid CONNECT request"));
        }

        let first_line = lines[0];
        let parts: Vec<&str> = first_line.split_whitespace().collect();
        if parts.len() < 2 || parts[0] != "CONNECT" {
            return Err(anyhow::anyhow!("Invalid CONNECT request format"));
        }

        let host = parts[1];
        info!("HTTPS CONNECT request for {}", host);

        // Try to resolve host to socket address
        let addr = host.to_socket_addrs()
            .with_context(|| format!("Failed to resolve host: {}", host))?
            .next()
            .ok_or_else(|| anyhow::anyhow!("Failed to resolve host: {}", host))?;

        // Connect to target server
        let target_stream = match timeout(config.timeout(), TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => {
                error!("Failed to connect to {}: {}", host, e);
                let response = "HTTP/1.1 502 Bad Gateway\r\n\r\n";
                let _ = client_stream.write_all(response.as_bytes()).await;
                return Err(e.into());
            }
            Err(_) => {
                error!("Connection to {} timed out", host);
                let response = "HTTP/1.1 504 Gateway Timeout\r\n\r\n";
                let _ = client_stream.write_all(response.as_bytes()).await;
                return Err(anyhow::anyhow!("Connection timeout"));
            }
        };

        info!("Connected to {}", host);

        // Send 200 Connection Established response
        let response = "HTTP/1.1 200 Connection Established\r\n\r\n";
        client_stream.write_all(response.as_bytes()).await?;

        // Create tunnel between client and target
        tunnel::create_tunnel(client_stream, target_stream, config.timeout()).await
    }

    /// Handle regular HTTP requests
    async fn handle_http(
        req: Request<Incoming>,
        http_client: Client<HttpConnector, Full<Bytes>>,
        config: Arc<Config>
    ) -> Result<Response<Full<Bytes>>> {
        // Ensure request has a complete URL
        let uri = req.uri().clone();
        let method = req.method().clone();

        info!("Handling HTTP request: {} {}", method, uri);

        // Copy headers before consuming the request
        let mut headers = Vec::new();
        for (name, value) in req.headers().iter() {
            let name_str = name.as_str().to_lowercase();
            if !name_str.starts_with("proxy-") {
                headers.push((name.clone(), value.clone()));
            }
        }
        
        // Read the request body
        let (_parts, body) = req.into_parts();
        let body_bytes = match http_body_util::BodyExt::collect(body).await {
            Ok(collected) => collected.to_bytes(),
            Err(e) => {
                error!("Failed to read request body: {}", e);
                let response = Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Full::new(Bytes::from("Failed to read request body")))?;
                return Ok(response);
            }
        };

        // Create target request
        let mut target_req = Request::builder()
            .method(method)
            .uri(uri.clone());

        // Add headers to the target request
        let headers_mut = target_req.headers_mut().unwrap();
        for (name, value) in headers {
            headers_mut.insert(name, value);
        }

        let target_req = target_req.body(Full::new(body_bytes))?;

        // Send request to target server
        match timeout(config.timeout(), http_client.request(target_req)).await {
            Ok(Ok(response)) => {
                info!("Received response: {} for {}", response.status(), uri);

                // Convert the response body
                let (parts, body) = response.into_parts();
                let body_bytes = match http_body_util::BodyExt::collect(body).await {
                    Ok(collected) => collected.to_bytes(),
                    Err(e) => {
                        error!("Failed to read response body: {}", e);
                        return Ok(Response::builder()
                            .status(StatusCode::INTERNAL_SERVER_ERROR)
                            .body(Full::new(Bytes::from("Failed to read response body")))?);
                    }
                };
                
                let mut builder = Response::builder()
                    .status(parts.status);
                
                // Copy headers
                for (name, value) in parts.headers {
                    if let Some(name) = name {
                        builder = builder.header(name, value);
                    }
                }
                
                Ok(builder.body(Full::new(body_bytes))?)
            },
            Ok(Err(e)) => {
                error!("HTTP request error for {}: {}", uri, e);
                Ok(Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .body(Full::new(Bytes::from(format!("Error: {}", e))))?
                )
            }
            Err(_) => {
                error!("HTTP request timed out for {}", uri);
                Ok(Response::builder()
                    .status(StatusCode::GATEWAY_TIMEOUT)
                    .body(Full::new(Bytes::from("Request timed out")))?
                )
            }
        }
    }
}

/// A stream that combines buffered data with a TCP stream
struct CombinedStream {
    buffer: std::io::Cursor<Vec<u8>>,
    stream: TcpStream,
    buffer_exhausted: bool,
}

impl CombinedStream {
    fn new(buffer_data: Vec<u8>, stream: TcpStream) -> Self {
        Self {
            buffer: std::io::Cursor::new(buffer_data),
            stream,
            buffer_exhausted: false,
        }
    }
}

impl tokio::io::AsyncRead for CombinedStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if !self.buffer_exhausted {
            let mut temp_buf = vec![0; buf.remaining()];
            match std::io::Read::read(&mut self.buffer, &mut temp_buf) {
                Ok(0) => {
                    self.buffer_exhausted = true;
                }
                Ok(n) => {
                    buf.put_slice(&temp_buf[..n]);
                    return std::task::Poll::Ready(Ok(()));
                }
                Err(e) => return std::task::Poll::Ready(Err(e)),
            }
        }

        // Buffer exhausted, read from the actual stream
        std::pin::Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for CombinedStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        std::pin::Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::pin::Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::pin::Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
