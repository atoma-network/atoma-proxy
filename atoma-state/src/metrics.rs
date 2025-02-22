use std::collections::HashMap;

use atoma_p2p::metrics::{
    ChatCompletionsMetrics, EmbeddingsMetrics, ImageGenerationMetrics, ModelMetrics, NodeMetrics,
};
use once_cell::sync::Lazy;
use prometheus::{GaugeVec, Opts, Registry};
use serde::Deserialize;
use tracing::instrument;

/// Metrics timeout, in seconds
const METRICS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// Model label
const MODEL_LABEL: &str = "model";

/// Node small id label
const NODE_SMALL_ID_LABEL: &str = "node_small_id";

type Result<T> = std::result::Result<T, MetricsServiceError>;

/// HTTP client for the node metrics queries
static HTTP_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .timeout(METRICS_TIMEOUT)
        .build()
        .expect("Failed to create HTTP client")
});

/// A service that manages and collects various metrics for different model types in the system.
///
/// This service handles metrics collection for three main categories:
/// - Chat Completions: Tracks GPU/CPU usage, timing metrics, and request counts
/// - Embeddings: Monitors latency and concurrent request counts
/// - Image Generation: Records latency and concurrent request counts
pub struct NodeMetricsCollector {
    /// The Prometheus registry for storing all metrics
    registry: Registry,

    /// GPU KV cache usage percentage for chat completions
    chat_completions_gpu_usage: GaugeVec,

    /// CPU KV cache usage percentage for chat completions
    chat_completions_cpu_usage: GaugeVec,

    /// Time to first token for chat completions
    chat_completions_ttft: GaugeVec,

    /// Time per output token for chat completions
    chat_completions_tpot: GaugeVec,

    /// Number of currently running chat completion requests
    chat_completions_num_running_requests: GaugeVec,

    /// Number of chat completion requests in waiting state
    chat_completions_num_waiting_requests: GaugeVec,

    /// Processing latency for embedding operations
    embeddings_latency: GaugeVec,

    /// Number of currently running embedding requests
    embeddings_num_running_requests: GaugeVec,

    /// Processing latency for image generation
    image_generation_latency: GaugeVec,

    /// Number of currently running image generation requests
    image_generation_num_running_requests: GaugeVec,

    /// The URL of the Prometheus metrics endpoint.
    metrics_url: String,
}

impl NodeMetricsCollector {
    /// Constructor
    #[allow(clippy::similar_names)]
    pub fn new(metrics_url: String) -> Self {
        let registry = Registry::new();

        let (
            chat_completions_gpu_usage,
            chat_completions_cpu_usage,
            chat_completions_ttft,
            chat_completions_tpot,
            chat_completions_num_running_requests,
            chat_completions_num_waiting_requests,
        ) = Self::chat_completions_registry(&registry);
        let (embeddings_latency, embeddings_num_running_requests) =
            Self::embeddings_registry(&registry);
        let (image_generation_latency, image_generation_num_running_requests) =
            Self::image_generation_registry(&registry);

        Self {
            registry,
            chat_completions_gpu_usage,
            chat_completions_cpu_usage,
            chat_completions_ttft,
            chat_completions_tpot,
            chat_completions_num_running_requests,
            chat_completions_num_waiting_requests,
            embeddings_latency,
            embeddings_num_running_requests,
            image_generation_latency,
            image_generation_num_running_requests,
            metrics_url,
        }
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn retrieve_best_available_node_for_chat_completions(
        &self,
        model: &str,
    ) -> Result<i64> {
        let query = format!(
            r#"topk(10,
                -1 * (
                    (
                        chat_time_to_first_token{{model="{model}"}} + 
                        chat_time_per_output_token{{model="{model}"}}
                    ) unless (
                        chat_num_waiting_requests{{model="{model}"}} / 
                        (chat_num_running_requests{{model="{model}"}} or vector(1)) >= 0.1
                    )
                )
            )"#,
        );

        let node_metrics: PromQueryResponse = HTTP_CLIENT
            .get(self.metrics_url.clone())
            .query(&[("query", query)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        // Reset the running requests metrics
        self.chat_completions_cpu_usage.reset();
        self.chat_completions_gpu_usage.reset();
        self.chat_completions_ttft.reset();
        self.chat_completions_tpot.reset();
        self.chat_completions_num_running_requests.reset();
        self.chat_completions_num_waiting_requests.reset();

        Ok(node_metrics)
    }

    /// Stores metrics collected from a node into the Prometheus registry.
    ///
    /// This method processes and stores different types of metrics (chat completions, embeddings,
    /// and image generation) for each model running on a node. The metrics are stored with
    /// appropriate labels for the model and node identifier.
    ///
    /// # Arguments
    ///
    /// * `node_metrics` - A reference to [`NodeMetrics`] containing metrics for all models on the node
    /// * `node_small_id` - A unique identifier for the node
    ///
    /// # Instrumentation
    ///
    /// This method is instrumented with debug-level tracing that includes:
    /// * `node_small_id` - The node's identifier
    /// * `model_metrics` - Debug representation of all model metrics being stored
    #[instrument(
        level = "debug",
        skip_all,
        fields(
            node_small_id = node_small_id,
            model_metrics = ?node_metrics.model_metrics,
        )
    )]
    pub fn store_metrics(&self, node_metrics: &NodeMetrics, node_small_id: i64) {
        for (model, model_metrics) in node_metrics.model_metrics.iter() {
            match model_metrics {
                ModelMetrics::ChatCompletions(chat_completions) => {
                    self.store_chat_completions_metrics(chat_completions, model, node_small_id);
                }
                ModelMetrics::Embeddings(embeddings) => {
                    self.store_embeddings_metrics(embeddings, model, node_small_id);
                }
                ModelMetrics::ImageGeneration(image_generation) => {
                    self.store_image_generation_metrics(image_generation, model, node_small_id);
                }
            }
        }
    }

    /// Stores chat completion metrics for a specific model and node in the Prometheus registry.
    ///
    /// This method records various performance metrics related to chat completion operations:
    /// - GPU and CPU KV cache usage percentages
    /// - Time to first token (TTFT)
    /// - Time per output token (TPOT)
    /// - Number of running and waiting requests
    ///
    /// # Arguments
    ///
    /// * `chat_completions` - Reference to chat completion metrics containing performance data
    /// * `model` - The model identifier string
    /// * `node_small_id` - Unique identifier for the node
    ///
    /// # Instrumentation
    ///
    /// This method is instrumented with debug-level tracing that includes:
    /// * `model` - The model identifier string
    /// * `node_small_id` - Unique identifier for the node
    #[instrument(
        level = "debug",
        skip_all,
        fields(
            model = model,
            node_small_id = node_small_id,
        )
    )]
    pub fn store_chat_completions_metrics(
        &self,
        chat_completions: &ChatCompletionsMetrics,
        model: &str,
        node_small_id: i64,
    ) {
        self.chat_completions_gpu_usage
            .with_label_values(&[model, node_small_id.to_string().as_str()])
            .set(chat_completions.gpu_kv_cache_usage_perc);
        self.chat_completions_cpu_usage
            .with_label_values(&[model, node_small_id.to_string().as_str()])
            .set(chat_completions.cpu_kv_cache_usage_perc);
        self.chat_completions_ttft
            .with_label_values(&[model, node_small_id.to_string().as_str()])
            .set(chat_completions.time_to_first_token);
        self.chat_completions_tpot
            .with_label_values(&[model, node_small_id.to_string().as_str()])
            .set(chat_completions.time_per_output_token);
        self.chat_completions_num_running_requests
            .with_label_values(&[model, node_small_id.to_string().as_str()])
            .set(chat_completions.num_running_requests as f64);
        self.chat_completions_num_waiting_requests
            .with_label_values(&[model, node_small_id.to_string().as_str()])
            .set(chat_completions.num_waiting_requests as f64);
    }

    /// Stores embedding metrics for a specific model and node in the Prometheus registry.
    ///
    /// This method records performance metrics related to embedding operations:
    /// - Processing latency for embedding requests
    /// - Number of currently running embedding requests
    ///
    /// # Arguments
    ///
    /// * `embeddings` - Reference to embedding metrics containing performance data
    /// * `model` - The model identifier string
    /// * `node_small_id` - Unique identifier for the node
    ///
    /// # Instrumentation
    ///
    /// This method is instrumented with debug-level tracing that includes:
    /// * `model` - The model identifier string
    /// * `node_small_id` - Unique identifier for the node
    #[instrument(
        level = "debug",
        skip_all,
        fields(
            model = model,
            node_small_id = node_small_id,
        )
    )]
    pub fn store_embeddings_metrics(
        &self,
        embeddings: &EmbeddingsMetrics,
        model: &str,
        node_small_id: i64,
    ) {
        self.embeddings_latency
            .with_label_values(&[model, node_small_id.to_string().as_str()])
            .set(embeddings.embeddings_latency);
        self.embeddings_num_running_requests
            .with_label_values(&[model, node_small_id.to_string().as_str()])
            .set(embeddings.num_running_requests as f64);
    }

    /// Stores image generation metrics for a specific model and node in the Prometheus registry.
    ///
    /// This method records performance metrics related to image generation operations:
    /// - Processing latency for image generation requests
    /// - Number of currently running image generation requests
    ///
    /// # Arguments
    ///
    /// * `image_generation` - Reference to image generation metrics containing performance data
    /// * `model` - The model identifier string
    /// * `node_small_id` - Unique identifier for the node
    ///
    /// # Instrumentation
    ///
    /// This method is instrumented with debug-level tracing that includes:
    /// * `model` - The model identifier string
    /// * `node_small_id` - Unique identifier for the node
    #[instrument(
        level = "debug",
        skip_all,
        fields(
            model = model,
            node_small_id = node_small_id,
        )
    )]
    pub fn store_image_generation_metrics(
        &self,
        image_generation: &ImageGenerationMetrics,
        model: &str,
        node_small_id: i64,
    ) {
        self.image_generation_latency
            .with_label_values(&[model, node_small_id.to_string().as_str()])
            .set(image_generation.image_generation_latency);
        self.image_generation_num_running_requests
            .with_label_values(&[model, node_small_id.to_string().as_str()])
            .set(image_generation.num_running_requests as f64);
    }

    /// Creates and registers the chat completions metrics in the Prometheus registry.
    ///
    /// This method sets up the metrics for chat completions, including:
    /// - GPU KV cache usage percentage
    /// - CPU KV cache usage percentage
    /// - Time to first token
    /// - Time per output token
    /// - Number of running requests
    /// - Number of waiting requests
    ///
    /// # Returns
    ///
    /// A tuple containing the following metrics:
    /// - GPU KV cache usage percentage
    /// - CPU KV cache usage percentage
    /// - Time to first token
    /// - Time per output token
    /// - Number of running requests
    /// - Number of waiting requests
    fn chat_completions_registry(
        registry: &Registry,
    ) -> (GaugeVec, GaugeVec, GaugeVec, GaugeVec, GaugeVec, GaugeVec) {
        let gpu_usage_opts = Opts::new(
            "chat_gpu_kv_cache_usage_perc",
            "GPU KV cache usage percentage for chat completions",
        );
        let gpu_usage = GaugeVec::new(gpu_usage_opts, &[MODEL_LABEL, NODE_SMALL_ID_LABEL])
            .expect("Failed to create gauge");
        let cpu_usage_opts = Opts::new(
            "chat_cpu_kv_cache_usage_perc",
            "CPU KV cache usage percentage for chat completions",
        );
        let cpu_usage = GaugeVec::new(cpu_usage_opts, &[MODEL_LABEL, NODE_SMALL_ID_LABEL])
            .expect("Failed to create gauge");
        let ttft_opts = Opts::new(
            "chat_time_to_first_token",
            "Time to first token for chat completions",
        );
        let ttft = GaugeVec::new(ttft_opts, &[MODEL_LABEL, NODE_SMALL_ID_LABEL])
            .expect("Failed to create gauge");
        let tpot_opts = Opts::new(
            "chat_time_per_output_token",
            "Time per output token for chat completions",
        );
        let tpot = GaugeVec::new(tpot_opts, &[MODEL_LABEL, NODE_SMALL_ID_LABEL])
            .expect("Failed to create gauge");
        let num_running_requests_opts = Opts::new(
            "chat_num_running_requests",
            "Number of running requests for chat completions",
        );
        let num_running_requests = GaugeVec::new(
            num_running_requests_opts,
            &[MODEL_LABEL, NODE_SMALL_ID_LABEL],
        )
        .expect("Failed to create gauge");
        let num_waiting_requests_opts = Opts::new(
            "chat_num_waiting_requests",
            "Number of waiting requests for chat completions",
        );
        let num_waiting_requests = GaugeVec::new(
            num_waiting_requests_opts,
            &[MODEL_LABEL, NODE_SMALL_ID_LABEL],
        )
        .expect("Failed to create gauge");

        registry.register(Box::new(gpu_usage.clone()));
        registry.register(Box::new(cpu_usage.clone()));
        registry.register(Box::new(ttft.clone()));
        registry.register(Box::new(tpot.clone()));
        registry.register(Box::new(num_running_requests.clone()));
        registry.register(Box::new(num_waiting_requests.clone()));

        (
            gpu_usage,
            cpu_usage,
            ttft,
            tpot,
            num_running_requests,
            num_waiting_requests,
        )
    }

    /// Creates and registers the embeddings metrics in the Prometheus registry.
    ///
    /// This method sets up the metrics for embeddings, including:
    /// - Processing latency for embedding requests
    /// - Number of currently running embedding requests
    ///
    /// # Returns
    ///
    /// A tuple containing the following metrics:
    /// - Processing latency for embedding requests
    /// - Number of currently running embedding requests
    fn embeddings_registry(registry: &Registry) -> (GaugeVec, GaugeVec) {
        let embeddings_latency_opts = Opts::new("embeddings_latency", "Latency for embeddings");
        let embeddings_latency =
            GaugeVec::new(embeddings_latency_opts, &[MODEL_LABEL, NODE_SMALL_ID_LABEL])
                .expect("Failed to create gauge");
        let num_running_requests_opts = Opts::new(
            "embeddings_num_running_requests",
            "Number of running requests for embeddings",
        );
        let num_running_requests = GaugeVec::new(
            num_running_requests_opts,
            &[MODEL_LABEL, NODE_SMALL_ID_LABEL],
        )
        .expect("Failed to create gauge");

        registry.register(Box::new(embeddings_latency.clone()));
        registry.register(Box::new(num_running_requests.clone()));

        (embeddings_latency, num_running_requests)
    }

    /// Creates and registers the image generation metrics in the Prometheus registry.
    ///
    /// This method sets up the metrics for image generation, including:
    /// - Processing latency for image generation requests
    /// - Number of currently running image generation requests
    ///
    /// # Returns
    ///
    /// A tuple containing the following metrics:
    /// - Processing latency for image generation requests
    /// - Number of currently running image generation requests
    fn image_generation_registry(registry: &Registry) -> (GaugeVec, GaugeVec) {
        let image_generation_latency_opts =
            Opts::new("image_generation_latency", "Latency for image generation");
        let image_generation_latency = GaugeVec::new(
            image_generation_latency_opts,
            &[MODEL_LABEL, NODE_SMALL_ID_LABEL],
        )
        .expect("Failed to create gauge");
        let num_running_requests_opts = Opts::new(
            "image_generation_num_running_requests",
            "Number of running requests for image generation",
        );
        let num_running_requests = GaugeVec::new(
            num_running_requests_opts,
            &[MODEL_LABEL, NODE_SMALL_ID_LABEL],
        )
        .expect("Failed to create gauge");

        registry.register(Box::new(image_generation_latency.clone()));
        registry.register(Box::new(num_running_requests.clone()));

        (image_generation_latency, num_running_requests)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MetricsServiceError {
    #[error("IO error: {0}")]
    Io(std::io::Error),
    #[error("Prometheus error: {0}")]
    Prometheus(#[from] prometheus::Error),
    #[error("Reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
}

/// Prometheus query response format following the API description of 
/// https://prometheus.io/docs/prometheus/latest/querying/api/
#[derive(Deserialize)]
struct PromQueryResponse {
    /// The status of the query
    status: String,
    /// The data of the query
    data: PromData,
}

/// The data of the query
#[derive(Deserialize)]
struct PromData {
    /// The type of the result
    #[serde(rename = "resultType")]
    result_type: String,
    /// The result of the query
    result: Vec<PromResult>,
}

/// The result of the query
#[derive(Deserialize)]
struct PromResult {
    /// The metric of the result with the label name as the key and the label value as the value
    metric: HashMap<String, String>,
    /// The value of the result as a tuple of [timestamp, value_as_string]
    value: (f64, String),
}