use opentelemetry::{
    global,
    metrics::{Counter, Histogram, Meter},
    KeyValue,
};
use reqwest::Client;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use tracing::{debug, error, instrument, warn};

static GLOBAL_METER: LazyLock<Meter> = LazyLock::new(|| global::meter("atoma-proxy"));

/// Histogram metric that tracks the latency of node connections from the proxy to the node.
///
/// This metric tracks the latency of node connections from the proxy to the node.
///
/// # Metric Details
/// - Name: `atoma_node_connection_latency`
/// - Type: Histogram
/// - Labels: `node_ip_address`
/// - Unit: seconds (s)
///
pub static NODE_CONNECTION_LATENCY: LazyLock<Histogram<f64>> = LazyLock::new(|| {
    GLOBAL_METER
        .f64_histogram("atoma_node_connection_latency")
        .with_description("The latency of node connections from the proxy to the node")
        .with_unit("s")
        .build()
});

/// Counter metric that tracks the number of failed node connections from the proxy to the node.
///
/// This metric tracks the number of failed node connections from the proxy to the node.
///
/// # Metric Details
/// - Name: `atoma_failed_node_connections`
/// - Type: Counter
/// - Labels: `node_ip_address`
/// - Unit: count
///
pub static FAILED_NODE_CONNECTIONS: LazyLock<Counter<u64>> = LazyLock::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_failed_node_connections")
        .with_description("The number of failed node connections from the proxy to the node")
        .with_unit("s")
        .build()
});

#[derive(Default)]
pub struct NetworkMetrics {
    client: Client,
}

impl NetworkMetrics {
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
    #[instrument(level = "trace", name = "update_metrics", skip(self))]
    pub async fn update_metrics(&mut self, node_addresses: Vec<String>) {
        for address in node_addresses {
            let url = format!("http://{address}/health");
            let start = Instant::now();
            match self
                .client
                .get(&url)
                .timeout(Duration::from_secs(5))
                .send()
                .await
            {
                Ok(response) => {
                    let latency = start.elapsed();
                    if response.status().is_success() {
                        NODE_CONNECTION_LATENCY.record(
                            latency.as_secs_f64(),
                            &[KeyValue::new("node_ip_address", address.clone())],
                        );
                        debug!(
                            target = "network_metrics",
                            node_address = %address,
                            latency_ms = latency.as_millis(),
                            "Successfully pinged node"
                        );
                    } else {
                        FAILED_NODE_CONNECTIONS
                            .add(1, &[KeyValue::new("node_ip_address", address.clone())]);
                        warn!(
                            target = "network_metrics",
                            node_address = %address,
                            status = %response.status(),
                            "Node health check returned non-success status"
                        );
                    }
                }
                Err(e) => {
                    FAILED_NODE_CONNECTIONS
                        .add(1, &[KeyValue::new("node_ip_address", address.clone())]);
                    error!(
                        target = "network_metrics",
                        node_address = %address,
                        error = %e,
                        "Failed to ping node"
                    );
                }
            }
        }
    }
}
