use anyhow::{Context, Result};
use hyper::{Method, Request, Response, StatusCode};
use hyper::body::{Bytes, Incoming};
use http_body_util::{Full};
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper::service::service_fn;
use log::{debug, error, info};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use http::header::HeaderValue;
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

        // Create HTTP client
        let http_client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(config.idle_timeout())
            .pool_max_idle_per_host(config.max_idle_connections)
            .build_http();

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

            let http_client = http_client.clone();
            let config = config.clone();

            // Spawn a task to handle each connection
            tokio::spawn(async move {
                let service = service_fn(move |req| {
                    let http_client = http_client.clone();
                    let config = config.clone();
                    async move {
                        Self::handle_request(req, http_client, config).await
                    }
                });

                // Wrap TcpStream with TokioIo to make it compatible with hyper's I/O traits
                let io = TokioIo::new(stream);
                let conn = Http1Builder::new()
                    .serve_connection(io, service)
                    .await;

                if let Err(e) = conn {
                    error!("Connection error: {}", e);
                }
            });
        }
    }

    /// Handle client requests
    async fn handle_request(
        req: Request<Incoming>,
        http_client: Client<HttpConnector, Full<Bytes>>,
        config: Arc<Config>
    ) -> Result<Response<Full<Bytes>>> {
        if config.verbose {
            debug!("Received request: {} {}", req.method(), req.uri());
        }

        // Handle CONNECT method (HTTPS tunnel)
        if req.method() == Method::CONNECT {
            return Self::handle_connect(req, config).await;
        }

        // Handle regular HTTP requests
        Self::handle_http(req, http_client, config).await
    }

    /// Handle HTTPS CONNECT requests
    async fn handle_connect(req: Request<Incoming>, config: Arc<Config>) -> Result<Response<Full<Bytes>>> {
        // Extract target host and port from URI
        let host = req.uri().authority()
            .ok_or_else(|| anyhow::anyhow!("URI missing authority"))?
            .to_string();

        if config.verbose {
            info!("HTTPS CONNECT request for {}", host);
        }

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
                let response = Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .body(Full::new(Bytes::new()))?;
                return Ok(response);
            }
            Err(_) => {
                error!("Connection to {} timed out", host);
                let response = Response::builder()
                    .status(StatusCode::GATEWAY_TIMEOUT)
                    .body(Full::new(Bytes::new()))?;
                return Ok(response);
            }
        };

        if config.verbose {
            debug!("Connected to target: {}", host);
        }

        // Create a special response to let hyper know we're taking over the connection
        let mut response = Response::builder()
            .status(StatusCode::OK)
            .body(Full::new(Bytes::new()))?; // Use Full<Bytes> instead of Empty

        // Set upgrade flag to indicate we want to take over the connection
        response.headers_mut().insert(
            hyper::header::CONNECTION,
            HeaderValue::from_static("Upgrade"),
        );

        let on_upgrade = hyper::upgrade::on(req);

        // Spawn a task to handle the upgrade once it completes
        tokio::spawn(async move {
            match on_upgrade.await {
                Ok(upgraded) => {
                    // 将 upgraded 转换为实现 AsyncRead/AsyncWrite 的类型
                    let upgraded_io = TokioIo::new(upgraded);
                    
                    if let Err(e) = tunnel::create_tunnel(upgraded_io, target_stream, config.timeout(), config.verbose).await {
                        error!("Tunnel error for {}: {}", host, e);
                    }
                }
                Err(e) => {
                    error!("Upgrade failed for {}: {}", host, e);
                }
            }
        });

        Ok(response)
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

        if config.verbose {
            debug!("Handling HTTP request: {} {}", method, uri);
        }

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
                if config.verbose {
                    debug!("Received response: {} for {}", response.status(), uri);
                }
                
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
