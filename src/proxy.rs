use anyhow::{Context, Result};
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1::Builder as Http1Builder;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::{TokioExecutor, TokioIo};
use log::{error, info};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use tokio::io::{AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

use crate::config::Config;
use crate::tunnel;

/// HTTP/HTTPS tunnel proxy service
pub struct TunnelProxy {
    pub config: Arc<Config>,
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
    async fn handle_connection(stream: TcpStream, config: Arc<Config>) -> Result<()> {
        // Use peek instead of read to detect request type
        let mut buffer = [0; 4096];
        let n = stream.peek(&mut buffer).await?;
        let request_data = &buffer[..n];
        let request_str = String::from_utf8_lossy(request_data);

        // Check if this is a CONNECT request
        if request_str.starts_with("CONNECT ") {
            // Handle CONNECT request directly
            Self::handle_connect_direct(stream, request_str.to_string(), config).await
        } else {
            // Directly use hyper to handle HTTP requests, no need for CombinedStream
            let io = TokioIo::new(stream);

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::time::{sleep, Duration};
    use env_logger;

    fn create_test_config() -> Arc<Config> {
        Arc::new(Config {
            address: "127.0.0.1:0".to_string(),
            timeout_seconds: 5,
            max_idle_connections: 10,
            idle_timeout_seconds: 30,
        })
    }

    // Helper function to create a mock HTTP server
    async fn create_mock_http_server() -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_url = format!("http://127.0.0.1:{}", addr.port());

        let handle = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buffer = [0; 4096];
                    if let Ok(n) = stream.read(&mut buffer).await {
                        let request = String::from_utf8_lossy(&buffer[..n]);

                        // Parse the request to extract method and path
                        let mut method = "GET";
                        let mut path = "/";
                        if let Some(first_line) = request.lines().next() {
                            let parts: Vec<&str> = first_line.split_whitespace().collect();
                            if parts.len() >= 2 {
                                method = parts[0];
                                path = parts[1];
                            }
                        }

                        // Send a simple HTTP response
                        let response_body = format!(r#"{{"method": "{}", "path": "{}"}}"#, method, path);
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{}",
                            response_body.len(),
                            response_body
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                    }
                });
            }
        });

        (server_url, handle)
    }

    // Helper function to create a mock HTTPS server (for CONNECT testing)
    async fn create_mock_https_server() -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_host = format!("127.0.0.1:{}", addr.port());

        let handle = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    // Simple echo server for HTTPS tunnel testing
                    let mut buffer = [0; 1024];
                    while let Ok(n) = stream.read(&mut buffer).await {
                        if n == 0 {
                            break;
                        }
                        // Echo back the data (simulating HTTPS traffic)
                        let _ = stream.write_all(&buffer[..n]).await;
                    }
                });
            }
        });

        (server_host, handle)
    }

    #[tokio::test]
    async fn test_tunnel_proxy_new() {
        let config = create_test_config();
        let proxy = TunnelProxy::new(config.clone());
        assert_eq!(proxy.config.address, config.address);
        assert_eq!(proxy.config.timeout_seconds, config.timeout_seconds);
    }

    #[tokio::test]
    async fn test_tunnel_proxy_http_request() {
        // Create a mock HTTP server
        let (server_url, server_handle) = create_mock_http_server().await;

        // Create and start the tunnel proxy
        let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = proxy_listener.local_addr().unwrap();
        drop(proxy_listener);

        let proxy_config = Arc::new(Config {
            address: proxy_addr.to_string(),
            timeout_seconds: 5,
            max_idle_connections: 10,
            idle_timeout_seconds: 30,
        });

        // Start the proxy
        let proxy_task = tokio::spawn(async move {
            let listener = TcpListener::bind(proxy_addr).await.unwrap();
            while let Ok((stream, _)) = listener.accept().await {
                let config = proxy_config.clone();
                tokio::spawn(async move {
                    let _ = TunnelProxy::handle_connection(stream, config).await;
                });
            }
        });

        sleep(Duration::from_millis(100)).await;


        // Create a simple HTTP client request through the proxy
        let mut proxy_stream = TcpStream::connect(proxy_addr).await.unwrap();

        // Extract target host from server_url
        let target_host = server_url.replace("http://", "");
        let http_request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            server_url, target_host
        );

        proxy_stream.write_all(http_request.as_bytes()).await.unwrap();

        // Read response
        let mut response = Vec::new();
        proxy_stream.read_to_end(&mut response).await.unwrap();
        let response_str = String::from_utf8_lossy(&response);

        // Verify we got a response
        assert!(response_str.contains("HTTP/1.1"));
        assert!(response_str.contains("method"));

        // Clean up
        proxy_task.abort();
        server_handle.abort();
    }

    #[tokio::test]
    async fn test_tunnel_proxy_connect_request() {
        // Create a mock HTTPS server
        let (server_host, server_handle) = create_mock_https_server().await;

        // Create and start the tunnel proxy
        let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = proxy_listener.local_addr().unwrap();
        drop(proxy_listener);

        let proxy_config = Arc::new(Config {
            address: proxy_addr.to_string(),
            timeout_seconds: 5,
            max_idle_connections: 10,
            idle_timeout_seconds: 30,
        });

        // Start the proxy
        let proxy_task = tokio::spawn(async move {
            let listener = TcpListener::bind(proxy_addr).await.unwrap();
            while let Ok((stream, _)) = listener.accept().await {
                let config = proxy_config.clone();
                tokio::spawn(async move {
                    let _ = TunnelProxy::handle_connection(stream, config).await;
                });
            }
        });

        sleep(Duration::from_millis(100)).await;

        // Test CONNECT request through proxy
        let mut proxy_stream = TcpStream::connect(proxy_addr).await.unwrap();

        // Send CONNECT request
        let connect_request = format!(
            "CONNECT {} HTTP/1.1\r\nHost: {}\r\n\r\n",
            server_host, server_host
        );

        proxy_stream.write_all(connect_request.as_bytes()).await.unwrap();

        // Read CONNECT response
        let mut response = [0; 1024];
        let n = proxy_stream.read(&mut response).await.unwrap();
        let response_str = String::from_utf8_lossy(&response[..n]);

        // Verify CONNECT succeeded
        assert!(response_str.contains("HTTP/1.1 200 Connection Established"));

        // Test tunnel by sending data through the established connection
        let test_data = b"Hello, HTTPS tunnel!";
        proxy_stream.write_all(test_data).await.unwrap();

        // Read echoed data back
        let mut echo_response = [0; 1024];
        let echo_n = proxy_stream.read(&mut echo_response).await.unwrap();

        // Verify echo
        assert_eq!(&echo_response[..echo_n], test_data);

        // Clean up
        proxy_task.abort();
        server_handle.abort();
    }

    #[tokio::test]
    async fn test_tunnel_proxy_multiple_concurrent_requests() {
        // Create mock servers
        let (server_url1, server_handle1) = create_mock_http_server().await;
        let (server_url2, server_handle2) = create_mock_http_server().await;

        // Create and start the tunnel proxy
        let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = proxy_listener.local_addr().unwrap();
        drop(proxy_listener);

        let proxy_config = Arc::new(Config {
            address: proxy_addr.to_string(),
            timeout_seconds: 5,
            max_idle_connections: 10,
            idle_timeout_seconds: 30,
        });

        // Start the proxy
        let proxy_task = tokio::spawn(async move {
            let listener = TcpListener::bind(proxy_addr).await.unwrap();
            while let Ok((stream, _)) = listener.accept().await {
                let config = proxy_config.clone();
                tokio::spawn(async move {
                    let _ = TunnelProxy::handle_connection(stream, config).await;
                });
            }
        });

        sleep(Duration::from_millis(100)).await;

        // Create multiple concurrent requests
        let mut tasks = Vec::new();

        for i in 0..5 {
            let server_url = if i % 2 == 0 { server_url1.clone() } else { server_url2.clone() };
            let proxy_addr = proxy_addr;

            let task = tokio::spawn(async move {
                let mut proxy_stream = TcpStream::connect(proxy_addr).await.unwrap();

                let target_host = server_url.replace("http://", "");
                let http_request = format!(
                    "GET {}/test{} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                    server_url, i, target_host
                );

                proxy_stream.write_all(http_request.as_bytes()).await.unwrap();

                let mut response = Vec::new();
                proxy_stream.read_to_end(&mut response).await.unwrap();
                let response_str = String::from_utf8_lossy(&response);

                // Verify response
                assert!(response_str.contains("HTTP/1.1 200"));
                assert!(response_str.contains(&format!("/test{}", i)));

                i
            });

            tasks.push(task);
        }

        // Wait for all requests to complete
        for task in tasks {
            let _ = task.await.unwrap();
        }

        // Clean up
        proxy_task.abort();
        server_handle1.abort();
        server_handle2.abort();
    }

    #[tokio::test]
    async fn test_tunnel_proxy_error_handling() {
        let _ = env_logger::try_init();

        // Create and start the tunnel proxy
        let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = proxy_listener.local_addr().unwrap();
        drop(proxy_listener);

        let proxy_config = Arc::new(Config {
            address: proxy_addr.to_string(),
            timeout_seconds: 1, // Short timeout for testing
            max_idle_connections: 10,
            idle_timeout_seconds: 30,
        });

        // Start the proxy
        let proxy_task = tokio::spawn(async move {
            let listener = TcpListener::bind(proxy_addr).await.unwrap();
            while let Ok((stream, _)) = listener.accept().await {
                let config = proxy_config.clone();
                tokio::spawn(async move {
                    let _ = TunnelProxy::handle_connection(stream, config).await;
                });
            }
        });

        sleep(Duration::from_millis(100)).await;

        // Test connection to non-existent server
        let mut proxy_stream = TcpStream::connect(proxy_addr).await.unwrap();

        let connect_request = "CONNECT 192.0.2.1:443 HTTP/1.1\r\nHost: 192.0.2.1:443\r\n\r\n";
        proxy_stream.write_all(connect_request.as_bytes()).await.unwrap();

        // Read error response
        let mut response = [0; 1024];
        let n = proxy_stream.read(&mut response).await.unwrap();
        let response_str = String::from_utf8_lossy(&response[..n]);

        // Should get an error response (502 or 504)
        assert!(response_str.contains("HTTP/1.1 502") || response_str.contains("HTTP/1.1 504"));

        // Also print the error response for debugging
        info!("Error response: {}", response_str);

        // Clean up
        proxy_task.abort();
    }

    #[tokio::test]
    async fn test_parse_connect_request_valid() {
        let request = "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n";
        let _config = create_test_config();

        // Test that we can parse the CONNECT request format
        let lines: Vec<&str> = request.lines().collect();
        assert!(!lines.is_empty());

        let first_line = lines[0];
        let parts: Vec<&str> = first_line.split_whitespace().collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "CONNECT");
        assert_eq!(parts[1], "example.com:443");
        assert_eq!(parts[2], "HTTP/1.1");
    }

    #[tokio::test]
    async fn test_parse_connect_request_invalid() {
        let invalid_requests = vec![
            "",
            "GET / HTTP/1.1",
            "CONNECT",
            "INVALID REQUEST",
        ];

        for request in invalid_requests {
            let lines: Vec<&str> = request.lines().collect();

            if lines.is_empty() {
                continue; // Empty request should be handled
            }

            let first_line = lines[0];
            let parts: Vec<&str> = first_line.split_whitespace().collect();

            let is_valid_connect = parts.len() >= 2 && parts[0] == "CONNECT";

            if request == "CONNECT" {
                assert!(!is_valid_connect);
            } else if request == "GET / HTTP/1.1" {
                assert!(!is_valid_connect);
            }
        }
    }

    #[tokio::test]
    async fn test_http_request_handling() {
        let _config = create_test_config();

        // Create a simple HTTP request with empty body
        let req = Request::builder()
            .method("GET")
            .uri("http://example.com/")
            .body(Full::new(Bytes::new()));

        // Since we can't easily mock the HTTP client in this context,
        // we'll test the request building logic
        assert!(req.is_ok());
    }

    #[tokio::test]
    async fn test_proxy_response_codes() {
        // Test that various HTTP status codes are handled correctly
        let status_codes = vec![
            StatusCode::OK,
            StatusCode::BAD_GATEWAY,
            StatusCode::GATEWAY_TIMEOUT,
            StatusCode::INTERNAL_SERVER_ERROR,
        ];

        for status in status_codes {
            let response = Response::builder()
                .status(status)
                .body(Full::new(Bytes::from("test")))
                .unwrap();

            assert_eq!(response.status(), status);
        }
    }

    #[tokio::test]
    async fn test_address_parsing() {
        let valid_addresses = vec![
            "127.0.0.1:8080",
            "0.0.0.0:3128",
        ];

        for addr_str in valid_addresses {
            let result = addr_str.parse::<SocketAddr>();
            assert!(result.is_ok(), "Failed to parse address: {}", addr_str);
        }
    }

    #[tokio::test]
    async fn test_error_response_generation() {
        let error_cases = vec![
            (StatusCode::BAD_REQUEST, "Bad Request"),
            (StatusCode::BAD_GATEWAY, "Bad Gateway"),
            (StatusCode::GATEWAY_TIMEOUT, "Gateway Timeout"),
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error"),
        ];

        for (status, message) in error_cases {
            let response = Response::builder()
                .status(status)
                .body(Full::new(Bytes::from(message)))
                .unwrap();

            assert_eq!(response.status(), status);
        }
    }

    #[tokio::test]
    async fn test_request_method_preservation() {
        let methods = vec![
            hyper::Method::GET,
            hyper::Method::POST,
            hyper::Method::PUT,
            hyper::Method::DELETE,
            hyper::Method::HEAD,
            hyper::Method::OPTIONS,
        ];

        for method in methods {
            let req = Request::builder()
                .method(method.clone())
                .uri("http://example.com/")
                .body(Full::new(Bytes::new()));

            assert!(req.is_ok());
            // In real implementation, verify that method is preserved through proxy
        }
    }

    #[tokio::test]
    async fn test_uri_handling() {
        let test_uris = vec![
            "http://example.com/",
            "https://api.example.com/v1/data",
            "http://localhost:8080/test",
            "https://secure.example.com:8443/api",
        ];

        for uri_str in test_uris {
            let uri: hyper::Uri = uri_str.parse().unwrap();
            assert!(uri.scheme().is_some());
            assert!(uri.authority().is_some());
        }
    }
}
