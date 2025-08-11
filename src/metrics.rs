use std::sync::Mutex;

use lazy_static::lazy_static;
use prometheus::{CounterVec, Opts, default_registry, Encoder, TextEncoder};
use hyper::{Request, Response, Method, StatusCode};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use http_body_util::{Full};
use hyper::body::{Bytes, Incoming};
use std::convert::Infallible;
use std::net::SocketAddr;
use hyper::server::conn::http1::Builder;
use tokio::task::JoinHandle;
use tokio::net::TcpListener;
use anyhow::Result;

#[derive(Clone)]
pub struct Metrics {
    /// Total connections handled
    pub connections_total: CounterVec,
    /// Total errors, labeled by reason
    pub errors_total: CounterVec,
    /// Total bytes transferred in/out
    pub data_transferred_bytes_total: CounterVec,
}

impl Metrics {
    pub fn new(namespace: &str) -> Self {
        let connections_total = CounterVec::new(
            Opts::new("connections_total", "Total connections handled")
                .namespace(namespace),
            &["direction"], // "inbound" or "outbound"
        )
        .unwrap();

        let errors_total = CounterVec::new(
            Opts::new("errors_total", "Total errors by reason")
                .namespace(namespace),
            &["reason"], // "dns_resolution_failed", "timeout", "connection_refused", etc.
        )
        .unwrap();

        let data_transferred_bytes_total = CounterVec::new(
            Opts::new("data_transferred_bytes_total", "Total bytes transferred")
                .namespace(namespace),
            &["direction"], // "in" or "out"
        )
        .unwrap();

        Metrics {
            connections_total,
            errors_total,
            data_transferred_bytes_total,
        }
    }

    pub fn register(self) -> Result<Self, prometheus::Error> {
        let registry = default_registry();
        registry.register(Box::new(self.connections_total.clone()))?;
        registry.register(Box::new(self.errors_total.clone()))?;
        registry.register(Box::new(self.data_transferred_bytes_total.clone()))?;

        Ok(self)
    }

    pub fn inc_connections_total(&self, direction: &str) {
        self.connections_total
            .with_label_values(&[direction])
            .inc();
    }

    pub fn inc_errors_total(&self, reason: &str) {
        self.errors_total
            .with_label_values(&[reason])
            .inc();
    }

    pub fn add_data_transferred(&self, direction: &str, bytes: u64) {
        self.data_transferred_bytes_total
            .with_label_values(&[direction])
            .inc_by(bytes as f64);
    }
}

lazy_static! {
    pub static ref METRICS: Mutex<Option<Metrics>> = Mutex::new(None);
}

pub fn init_metrics(namespace: &str) -> Result<(), prometheus::Error> {
    let metrics = Metrics::new(namespace).register()?;
    let mut metrics_lock = METRICS.lock().unwrap();
    *metrics_lock = Some(metrics);
    Ok(())
}

pub fn inc_connections_total(direction: &str) {
    let metrics_lock = METRICS.lock().unwrap();
    if let Some(metrics) = &*metrics_lock {
        metrics.inc_connections_total(direction);
    }
}

pub fn inc_errors_total(reason: &str) {
    let metrics_lock = METRICS.lock().unwrap();
    if let Some(metrics) = &*metrics_lock {
        metrics.inc_errors_total(reason);
    }
}

pub fn add_data_transferred(direction: &str, bytes: u64) {
    let metrics_lock = METRICS.lock().unwrap();
    if let Some(metrics) = &*metrics_lock {
        metrics.add_data_transferred(direction, bytes);
    }
}

async fn metrics_handler(_req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer).unwrap();

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", encoder.format_type())
        .body(Full::new(Bytes::from(buffer))).unwrap())
}

async fn metrics_service(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    match (req.method(), req.uri().path()) {
        (&Method::GET, "/metrics") => metrics_handler(req).await,
        _ => Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from("Not Found")))
            .unwrap()),
    }
}

pub async fn start_metrics_server(addr: SocketAddr) -> Result<JoinHandle<()>> {
    let listener = TcpListener::bind(addr).await?;

    log::info!("Metrics server listening on http://{}/metrics", addr);

    let handle = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let io = TokioIo::new(stream);

                    tokio::task::spawn(async move {
                        let service = service_fn(metrics_service);

                        if let Err(err) = Builder::new()
                            .serve_connection(io, service)
                            .await
                        {
                            log::error!("Error serving connection: {:?}", err);
                        }
                    });
                }
                Err(e) => {
                    log::error!("Error accepting connection: {:?}", e);
                }
            }
        }
    });

    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics() {
        // Initialize metrics
        init_metrics("bridge_proxy").unwrap();

        // Test connections counter
        inc_connections_total("inbound");
        inc_connections_total("outbound");

        // Test errors counter
        inc_errors_total("dns_resolution_failed");
        inc_errors_total("timeout");
        inc_errors_total("connection_refused");

        // Test data transfer counter
        add_data_transferred("in", 1024);
        add_data_transferred("out", 2048);

        // Check if metrics are registered correctly
        let metric_families = prometheus::gather();

        let connections_metric = metric_families
            .iter()
            .find(|m| m.get_name() == "bridge_proxy_connections_total")
            .expect("connections_total metric should exist");
        assert_eq!(connections_metric.get_metric().len(), 2);

        let errors_metric = metric_families
            .iter()
            .find(|m| m.get_name() == "bridge_proxy_errors_total")
            .expect("errors_total metric should exist");
        assert_eq!(errors_metric.get_metric().len(), 3);

        let data_metric = metric_families
            .iter()
            .find(|m| m.get_name() == "bridge_proxy_data_transferred_bytes_total")
            .expect("data_transferred_bytes_total metric should exist");
        assert_eq!(data_metric.get_metric().len(), 2);
    }
}
