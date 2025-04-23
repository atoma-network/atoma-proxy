use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
};

use atoma_p2p::broadcast_metrics::{
    ChatCompletionsMetrics, EmbeddingsMetrics, ImageGenerationMetrics, ModelMetrics, NodeMetrics,
};
use flume::Receiver as FlumeReceiver;
use prometheus::{GaugeVec, Opts, Registry};
use serde::Deserialize;
use tokio::sync::{oneshot, watch, RwLock};
use tracing::instrument;

use crate::{config::MetricsCollectionConfig, errors::MetricsServiceError, types::Modalities};

/// Duration until next top k best available nodes selection
const DURATION_UNTIL_NEXT_TOP_K_BEST_AVAILABLE_NODES_SELECTION: std::time::Duration =
    std::time::Duration::from_secs(3 * 60); // 3 minutes

/// Metrics timeout, in seconds
const METRICS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// Model label
const MODEL_LABEL: &str = "model";

/// Node small id label
const NODE_SMALL_ID_LABEL: &str = "node_small_id";

/// Default top k best available nodes
pub(crate) const DEFAULT_TOP_K_BEST_AVAILABLE_NODES: usize = 10;

type Result<T> = std::result::Result<T, MetricsServiceError>;

/// HTTP client for the node metrics queries
static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(METRICS_TIMEOUT)
        .build()
        .expect("Failed to create HTTP client")
});

/// Spawns a task that periodically collects metrics for different modalities and their models.
///
/// This function creates a background task that runs indefinitely until a shutdown signal is received.
/// At regular intervals (defined by `DURATION_UNTIL_NEXT_TOP_K_BEST_AVAILABLE_NODES_SELECTION`), it
/// collects metrics for each modality-model pair and sends the best available nodes through a channel.
///
/// # Arguments
///
/// * `metrics_collection` - The metrics collection configuration
/// * `rx_request_best_available_nodes` - Channel receiver for requesting the collected best available nodes
/// * `shutdown_signal` - Watch channel receiver for graceful shutdown coordination
///
/// # Returns
///
/// Returns a [`JoinHandle`] that resolves to a `Result<()>`. The task can be joined to await its completion.
///
/// # Example
///
/// ```rust,ignore
/// use tokio::sync::{mpsc, watch};
///
/// let (tx, _rx) = mpsc::unbounded_channel();
/// let (shutdown_tx, shutdown_rx) = watch::channel(false);
///
/// let handle = trigger_new_metrics_collection_task(
///     "http://metrics.example.com".to_string(),
///     vec![(Modalities::ChatCompletions, "gpt-4".to_string())],
///     Some(3),
///     tx,
///     shutdown_rx,
/// );
/// ```
#[instrument(
    level = "debug",
    skip_all,
    fields(
        metrics_url = %metrics_collection_config.metrics_url,
        models = ?metrics_collection_config.models,
        top_k = ?metrics_collection_config.top_k
    )
)]
pub async fn trigger_new_metrics_collection_task(
    node_metrics_collector: NodeMetricsCollector,
    metrics_collection_config: MetricsCollectionConfig,
    rx_collected_metrics: FlumeReceiver<(i64, NodeMetrics)>,
    request_best_available_models_receiver: FlumeReceiver<(String, oneshot::Sender<Vec<i64>>)>,
    mut shutdown_signal: watch::Receiver<bool>,
) -> Result<()> {
    let best_available_nodes = Arc::new(RwLock::new(HashMap::with_capacity(
        metrics_collection_config.models.len(),
    )));
    loop {
        tokio::select! {
            () = tokio::time::sleep(DURATION_UNTIL_NEXT_TOP_K_BEST_AVAILABLE_NODES_SELECTION) => {
                if let Err(e) = collect_best_available_nodes(
                    best_available_nodes.clone(),
                    &metrics_collection_config,
                )
                .await
                {
                    tracing::error!(
                        target = "atoma-state-manager",
                        event = "collect_and_send_best_available_nodes_error",
                        error = %e,
                        "Error collecting and sending best available nodes"
                    );
                }
                // NOTE: We reset the prometheus metrics, as we retrieve the best available nodes
                // batch for the current time period.
                node_metrics_collector.reset_metrics();
            }
            rx_collected_metrics = rx_collected_metrics.recv_async() => {
                match rx_collected_metrics {
                    Ok((node_small_id, node_metrics)) => {
                        node_metrics_collector.store_metrics(&node_metrics, node_small_id);
                    }
                    Err(e) => {
                        tracing::error!(
                            target = "atoma-state-manager",
                            event = "rx_collected_metrics_error",
                            error = %e,
                        );
                    }
                }
            }
            request_best_available_models = request_best_available_models_receiver.recv_async() => {
                if let Err(e) = send_best_available_nodes(
                    request_best_available_models.map_err(MetricsServiceError::FlumeRecvError),
                    best_available_nodes.clone(),
                )
                .await
                {
                    tracing::error!(
                        target = "atoma-state-manager",
                        event = "send_best_available_nodes_error",
                        error = %e,
                    );
                }
            }
            shutdown_signal_changed = shutdown_signal.changed() => {
                match shutdown_signal_changed {
                    Ok(()) => {
                        if *shutdown_signal.borrow() {
                            tracing::trace!(
                                target = "atoma-state-manager",
                                event = "shutdown_signal",
                                "Shutdown signal received, shutting down"
                            );
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            target = "atoma-state-manager",
                            event = "shutdown_signal_error",
                            error = %e,
                            "Shutdown signal channel closed"
                        );
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Sends the best available nodes for a requested model through a oneshot channel.
///
/// This function retrieves the list of best available nodes for a specific model from the shared state
/// and sends it through a oneshot channel to the requester. The function is used as part of the metrics
/// collection system to provide information about the most optimal nodes for handling specific model requests.
///
/// # Arguments
///
/// * `request_best_available_models` - A Result containing a tuple of:
///   * A String representing the model identifier
///   * A oneshot::Sender for sending the list of best available node IDs
/// * `best_available_nodes` - An Arc<RwLock<HashMap>> containing the cached best available nodes for each model
///
/// # Returns
///
/// Returns a `Result<()>` which is:
/// * `Ok(())` if the nodes were successfully retrieved and sent
/// * `Err(MetricsServiceError)` if:
///   * The model was not found in the cached best available nodes
///   * The channel send operation failed
///   * The input Result was an error
///
/// # Example
///
/// ```rust,ignore
/// let (tx, rx) = oneshot::channel();
/// let best_nodes = Arc::new(RwLock::new(HashMap::new()));
///
/// // Send request for best nodes for "gpt-4" model
/// let request = Ok(("gpt-4".to_string(), tx));
/// send_best_available_nodes(request, best_nodes).await?;
///
/// // Receive the response
/// let nodes = rx.await?;
/// ```
#[instrument(level = "debug", skip_all, fields(send_best_available_nodes = true,))]
async fn send_best_available_nodes(
    request_best_available_models: Result<(String, oneshot::Sender<Vec<i64>>)>,
    best_available_nodes: Arc<RwLock<HashMap<String, Vec<i64>>>>,
) -> Result<()> {
    let (model, sender) = request_best_available_models?;
    let model_best_available_nodes = {
        let read_lock = best_available_nodes.read().await;
        read_lock
            .get(&model)
            .ok_or(MetricsServiceError::ModelNotFound(model))?
            .clone()
    };
    sender
        .send(model_best_available_nodes)
        .map_err(|_| MetricsServiceError::ChannelSendError)?;
    Ok(())
}

/// Collects and sends the best available nodes for the given models
///
/// # Arguments
///
/// * `best_available_nodes` - The best available nodes for the given models
/// * `config` - The metrics collection configuration
/// * `models` - The models to collect the best available nodes for
///
/// # Returns
///
/// Returns a `Result<()>`. The task can be joined to await its completion.
///
/// # Example
///
/// ```rust,ignore
/// let best_available_nodes = Arc::new(RwLock::new(HashMap::with_capacity(10)));
/// let config = MetricsCollectionConfig::default();
/// let models = vec![(Modalities::ChatCompletions, "gpt-4".to_string())];
/// let handle = collect_and_send_best_available_nodes(
///     best_available_nodes,
///     &config,
///     &models,
/// );
/// ```
#[instrument(
    level = "debug",
    skip_all,
    fields(
        metrics_url = %metrics_collection_config.metrics_url,
        models = ?metrics_collection_config.models,
        top_k = ?metrics_collection_config.top_k
    )
)]
async fn collect_best_available_nodes(
    best_available_nodes: Arc<RwLock<HashMap<String, Vec<i64>>>>,
    metrics_collection_config: &MetricsCollectionConfig,
) -> Result<()> {
    let futures = metrics_collection_config
        .models
        .iter()
        .map(|(modality, modality_model)| {
            let metrics_url = metrics_collection_config.metrics_url.clone();
            let best_available_nodes = best_available_nodes.clone();
            async move {
                let model_best_available_nodes = match modality {
                    Modalities::ChatCompletions => {
                        NodeMetricsCollector::retrieve_best_available_nodes_for_chat_completions(
                            &metrics_url,
                            modality_model,
                            metrics_collection_config.top_k,
                        )
                        .await?
                    }
                    Modalities::Embeddings => {
                        NodeMetricsCollector::retrieve_best_available_nodes_for_embeddings(
                            &metrics_url,
                            modality_model,
                            metrics_collection_config.top_k,
                        )
                        .await?
                    }
                    Modalities::ImagesGenerations => {
                        NodeMetricsCollector::retrieve_best_available_nodes_for_image_generation(
                            &metrics_url,
                            modality_model,
                            metrics_collection_config.top_k,
                        )
                        .await?
                    }
                };

                best_available_nodes
                    .write()
                    .await
                    .entry(modality_model.to_string())
                    .or_insert(model_best_available_nodes);

                Ok::<_, MetricsServiceError>((modality_model.to_string(), best_available_nodes))
            }
        });

    futures::future::try_join_all(futures).await?;

    Ok(())
}

/// A service that manages and collects various metrics for different model types in the system.
///
/// This service handles metrics collection for three main categories:
/// - Chat Completions: Tracks GPU/CPU usage, timing metrics, and request counts
/// - Embeddings: Monitors latency and concurrent request counts
/// - Image Generation: Records latency and concurrent request counts
pub struct NodeMetricsCollector {
    /// The Prometheus registry for storing all metrics
    #[allow(dead_code)]
    registry: Registry,

    /// GPU KV cache usage percentage for chat completions
    chat_completions_gpu_kv_cache_usage: GaugeVec,

    /// CPU KV cache usage percentage for chat completions
    chat_completions_cpu_kv_cache_usage: GaugeVec,

    /// Time to first token for chat completions
    chat_completions_ttft: GaugeVec,

    /// Time per output token for chat completions
    chat_completions_tpot: GaugeVec,

    /// Number of currently running chat completion requests
    chat_completions_num_running_requests: GaugeVec,

    /// Number of chat completion requests in waiting state
    chat_completions_num_waiting_requests: GaugeVec,

    /// Queue duration for embeddings
    embeddings_queue_duration: GaugeVec,

    /// Inference duration for embeddings
    embeddings_inference_duration: GaugeVec,

    /// Input length for embeddings
    embeddings_input_length: GaugeVec,

    /// Batch size for embeddings
    embeddings_batch_size: GaugeVec,

    /// Batch tokens for embeddings
    embeddings_batch_tokens: GaugeVec,

    /// Latency for image generation
    image_generation_latency: GaugeVec,

    /// Number of currently running image generation requests
    image_generation_num_running_requests: GaugeVec,
}

impl NodeMetricsCollector {
    /// Constructor
    ///
    /// # Errors
    ///
    /// Returns an error if the metrics cannot be registered.
    #[allow(clippy::similar_names)]
    pub fn new() -> Result<Self> {
        let registry = Registry::new();

        let (
            chat_completions_gpu_kv_cache_usage,
            chat_completions_cpu_kv_cache_usage,
            chat_completions_ttft,
            chat_completions_tpot,
            chat_completions_num_running_requests,
            chat_completions_num_waiting_requests,
        ) = Self::chat_completions_registry(&registry)?;
        let (
            embeddings_queue_duration,
            embeddings_inference_duration,
            embeddings_input_length,
            embeddings_batch_size,
            embeddings_batch_tokens,
        ) = Self::embeddings_registry(&registry)?;
        let (image_generation_latency, image_generation_num_running_requests) =
            Self::image_generation_registry(&registry)?;

        Ok(Self {
            registry,
            chat_completions_gpu_kv_cache_usage,
            chat_completions_cpu_kv_cache_usage,
            chat_completions_ttft,
            chat_completions_tpot,
            chat_completions_num_running_requests,
            chat_completions_num_waiting_requests,
            embeddings_queue_duration,
            embeddings_inference_duration,
            embeddings_input_length,
            embeddings_batch_size,
            embeddings_batch_tokens,
            image_generation_latency,
            image_generation_num_running_requests,
        })
    }

    /// Retrieves the best available nodes for chat completions based on performance metrics.
    ///
    /// This method queries the Prometheus metrics to find the most efficient nodes for handling
    /// chat completion requests. The selection is based on a combination of:
    /// - Time to first token (TTFT)
    /// - Time per output token (TPOT)
    /// - Load balancing consideration (ratio of waiting to running requests)
    ///
    /// The nodes are ranked using a scoring formula that considers both latency metrics (TTFT + TPOT)
    /// and excludes overloaded nodes where the ratio of waiting to running requests exceeds 10%.
    ///
    /// # Arguments
    ///
    /// * `metrics_url` - The URL of the Prometheus metrics endpoint
    /// * `model` - The model identifier to filter metrics for specific model types
    /// * `top_k` - Optional number of best nodes to return. Defaults to [`DEFAULT_TOP_K_BEST_AVAILABLE_NODES`]
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing a vector of node small IDs, sorted by their performance score
    /// (best performing nodes first). Returns an error if the Prometheus query fails.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// # use your_crate::NodeMetricsCollector;
    /// # async fn example(collector: &NodeMetricsCollector) -> Result<(), Box<dyn std::error::Error>> {
    /// // Get top 5 best nodes for the "gpt-4" model
    /// let best_nodes = collector.retrieve_best_available_node_for_chat_completions("gpt-4", Some(5)).await?;
    /// println!("Best nodes for chat completions: {:?}", best_nodes);
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(
        level = "debug",
        skip_all,
        fields(
            model = model,
            top_k = top_k.unwrap_or(DEFAULT_TOP_K_BEST_AVAILABLE_NODES),
        )
    )]
    pub async fn retrieve_best_available_nodes_for_chat_completions(
        metrics_url: &str,
        model: &str,
        top_k: Option<usize>,
    ) -> Result<Vec<i64>> {
        let top_k = top_k.unwrap_or(DEFAULT_TOP_K_BEST_AVAILABLE_NODES);
        let query = Self::chat_completions_query(model, top_k);

        Self::retrieve_prometheus_metrics(metrics_url, &query, top_k).await
    }

    /// Retrieves the best available nodes for embeddings based on performance metrics.
    ///
    /// This method queries the Prometheus metrics to find the most efficient nodes for handling
    /// embedding operations. The selection is based on the processing latency for embedding requests.
    ///
    /// # Arguments
    ///
    /// * `metrics_url` - The URL of the Prometheus metrics endpoint
    /// * `model` - The model identifier to filter metrics for specific model types
    /// * `top_k` - Optional number of best nodes to return. Defaults to [`DEFAULT_TOP_K_BEST_AVAILABLE_NODES`]
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing a vector of node small IDs, sorted by their performance score
    /// (best performing nodes first). Returns an error if the Prometheus query fails.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// # use your_crate::NodeMetricsCollector;
    /// # async fn example(collector: &NodeMetricsCollector) -> Result<(), Box<dyn std::error::Error>> {
    /// // Get top 5 best nodes for the "gpt-4" model
    /// let best_nodes = collector.retrieve_best_available_nodes_for_embeddings("gpt-4", Some(5)).await?;
    /// println!("Best nodes for embeddings: {:?}", best_nodes);
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(
        level = "debug",
        skip_all,
        fields(
            model = model,
            top_k = top_k.unwrap_or(DEFAULT_TOP_K_BEST_AVAILABLE_NODES),
        )
    )]
    pub async fn retrieve_best_available_nodes_for_embeddings(
        metrics_url: &str,
        model: &str,
        top_k: Option<usize>,
    ) -> Result<Vec<i64>> {
        let top_k = top_k.unwrap_or(DEFAULT_TOP_K_BEST_AVAILABLE_NODES);
        let query = Self::embeddings_query(model, top_k);

        Self::retrieve_prometheus_metrics(metrics_url, &query, top_k).await
    }

    /// Retrieves the best available nodes for image generation based on performance metrics.
    ///
    /// This method queries the Prometheus metrics to find the most efficient nodes for handling
    /// image generation operations. The selection is based on the processing latency for image generation requests.
    ///
    /// # Arguments
    ///
    /// * `metrics_url` - The URL of the Prometheus metrics endpoint
    /// * `model` - The model identifier to filter metrics for specific model types
    /// * `top_k` - Optional number of best nodes to return. Defaults to [`DEFAULT_TOP_K_BEST_AVAILABLE_NODES`]
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing a vector of node small IDs, sorted by their performance score
    /// (best performing nodes first). Returns an error if the Prometheus query fails.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// # use your_crate::NodeMetricsCollector;
    /// # async fn example(collector: &NodeMetricsCollector) -> Result<(), Box<dyn std::error::Error>> {
    /// // Get top 5 best nodes for the "gpt-4" model
    /// let best_nodes = collector.retrieve_best_available_nodes_for_image_generation("gpt-4", Some(5)).await?;
    /// println!("Best nodes for image generation: {:?}", best_nodes);
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(
        level = "debug",
        skip_all,
        fields(
            model = model,
            top_k = top_k.unwrap_or(DEFAULT_TOP_K_BEST_AVAILABLE_NODES),
        )
    )]
    pub async fn retrieve_best_available_nodes_for_image_generation(
        metrics_url: &str,
        model: &str,
        top_k: Option<usize>,
    ) -> Result<Vec<i64>> {
        let top_k = top_k.unwrap_or(DEFAULT_TOP_K_BEST_AVAILABLE_NODES);
        let query = Self::image_generation_query(model, top_k);

        Self::retrieve_prometheus_metrics(metrics_url, &query, top_k).await
    }

    /// Queries Prometheus and parses the response to get node IDs.
    ///
    /// This helper function handles the common pattern of querying Prometheus and extracting
    /// node IDs from the response. It's used by the various `retrieve_best_available_nodes_*`
    /// functions to avoid code duplication.
    ///
    /// # Arguments
    ///
    /// * `metrics_url` - The URL of the Prometheus endpoint to query
    /// * `query` - The PromQL query string to execute
    /// * `top_k` - Optional limit on number of results to return. Defaults to [`DEFAULT_TOP_K_BEST_AVAILABLE_NODES`]
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing a vector of node small IDs (i64) sorted according to the query criteria.
    /// Returns an error if:
    /// - The HTTP request fails
    /// - The response status is not successful
    /// - The response JSON cannot be parsed
    /// - A node ID cannot be parsed as an i64
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let query = r#"topk(5,
    ///     -1 * (embeddings_latency{model="text-embedding-ada-002"})
    /// )"#;
    ///
    /// let node_ids = retrieve_prometheus_metrics(
    ///     "http://prometheus:9090",
    ///     query,
    ///     Some(5)
    /// ).await?;
    /// ```
    #[instrument(
        level = "debug",
        skip_all,
        fields(
            metrics_url = metrics_url,
            query = query,
            top_k = top_k,
        )
    )]
    async fn retrieve_prometheus_metrics(
        metrics_url: &str,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<i64>> {
        let node_metrics: PromQueryResponse = HTTP_CLIENT
            .get(format!("{metrics_url}/api/v1/query"))
            .query(&[("query", query)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let mut best_available_node_small_ids: Vec<i64> = Vec::with_capacity(top_k);
        for result in node_metrics.data.result {
            let node_small_id = result.metric.get(NODE_SMALL_ID_LABEL).unwrap();
            let node_small_id = node_small_id.parse::<i64>().unwrap();
            best_available_node_small_ids.push(node_small_id);
        }

        Ok(best_available_node_small_ids)
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
        for (model, model_metrics) in &node_metrics.model_metrics {
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
        self.chat_completions_gpu_kv_cache_usage
            .with_label_values(&[model, node_small_id.to_string().as_str()])
            .set(chat_completions.gpu_kv_cache_usage_perc);
        self.chat_completions_cpu_kv_cache_usage
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
            .set(f64::from(chat_completions.num_running_requests));
        self.chat_completions_num_waiting_requests
            .with_label_values(&[model, node_small_id.to_string().as_str()])
            .set(f64::from(chat_completions.num_waiting_requests));
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
        self.embeddings_queue_duration
            .with_label_values(&[model, node_small_id.to_string().as_str()])
            .set(embeddings.embeddings_queue_duration);
        self.embeddings_inference_duration
            .with_label_values(&[model, node_small_id.to_string().as_str()])
            .set(embeddings.embeddings_inference_duration);
        self.embeddings_input_length
            .with_label_values(&[model, node_small_id.to_string().as_str()])
            .set(embeddings.embeddings_input_length);
        self.embeddings_batch_size
            .with_label_values(&[model, node_small_id.to_string().as_str()])
            .set(embeddings.embeddings_batch_size);
        self.embeddings_batch_tokens
            .with_label_values(&[model, node_small_id.to_string().as_str()])
            .set(embeddings.embeddings_batch_tokens);
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
            .set(f64::from(image_generation.num_running_requests));
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
    ///
    /// # Errors
    ///
    /// Returns an error if the metrics cannot be registered.
    fn chat_completions_registry(
        registry: &Registry,
    ) -> Result<(GaugeVec, GaugeVec, GaugeVec, GaugeVec, GaugeVec, GaugeVec)> {
        let gpu_kv_cache_usage_opts = Opts::new(
            "chat_gpu_kv_cache_usage_perc",
            "GPU KV cache usage percentage for chat completions",
        );
        let gpu_kv_cache_usage =
            GaugeVec::new(gpu_kv_cache_usage_opts, &[MODEL_LABEL, NODE_SMALL_ID_LABEL])
                .expect("Failed to create gauge");
        let cpu_kv_cache_usage_opts = Opts::new(
            "chat_cpu_kv_cache_usage_perc",
            "CPU KV cache usage percentage for chat completions",
        );
        let cpu_kv_cache_usage =
            GaugeVec::new(cpu_kv_cache_usage_opts, &[MODEL_LABEL, NODE_SMALL_ID_LABEL])
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

        registry.register(Box::new(gpu_kv_cache_usage.clone()))?;
        registry.register(Box::new(cpu_kv_cache_usage.clone()))?;
        registry.register(Box::new(ttft.clone()))?;
        registry.register(Box::new(tpot.clone()))?;
        registry.register(Box::new(num_running_requests.clone()))?;
        registry.register(Box::new(num_waiting_requests.clone()))?;

        Ok((
            gpu_kv_cache_usage,
            cpu_kv_cache_usage,
            ttft,
            tpot,
            num_running_requests,
            num_waiting_requests,
        ))
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
    ///
    /// # Errors
    ///
    /// Returns an error if the metrics cannot be registered.
    fn embeddings_registry(
        registry: &Registry,
    ) -> Result<(GaugeVec, GaugeVec, GaugeVec, GaugeVec, GaugeVec)> {
        let embeddings_queue_duration_opts =
            Opts::new("embeddings_queue_duration", "Queue duration for embeddings");
        let embeddings_queue_duration = GaugeVec::new(
            embeddings_queue_duration_opts,
            &[MODEL_LABEL, NODE_SMALL_ID_LABEL],
        )
        .expect("Failed to create gauge");
        let embeddings_inference_duration_opts = Opts::new(
            "embeddings_inference_duration",
            "Inference duration for embeddings",
        );
        let embeddings_inference_duration = GaugeVec::new(
            embeddings_inference_duration_opts,
            &[MODEL_LABEL, NODE_SMALL_ID_LABEL],
        )
        .expect("Failed to create gauge");
        let embeddings_input_length_opts =
            Opts::new("embeddings_input_length", "Input length for embeddings");
        let embeddings_input_length = GaugeVec::new(
            embeddings_input_length_opts,
            &[MODEL_LABEL, NODE_SMALL_ID_LABEL],
        )
        .expect("Failed to create gauge");
        let embeddings_batch_size_opts =
            Opts::new("embeddings_batch_size", "Batch size for embeddings");
        let embeddings_batch_size = GaugeVec::new(
            embeddings_batch_size_opts,
            &[MODEL_LABEL, NODE_SMALL_ID_LABEL],
        )
        .expect("Failed to create gauge");
        let embeddings_batch_tokens_opts =
            Opts::new("embeddings_batch_tokens", "Batch tokens for embeddings");
        let embeddings_batch_tokens = GaugeVec::new(
            embeddings_batch_tokens_opts,
            &[MODEL_LABEL, NODE_SMALL_ID_LABEL],
        )
        .expect("Failed to create gauge");

        registry.register(Box::new(embeddings_queue_duration.clone()))?;
        registry.register(Box::new(embeddings_inference_duration.clone()))?;
        registry.register(Box::new(embeddings_input_length.clone()))?;
        registry.register(Box::new(embeddings_batch_size.clone()))?;
        registry.register(Box::new(embeddings_batch_tokens.clone()))?;

        Ok((
            embeddings_queue_duration,
            embeddings_inference_duration,
            embeddings_input_length,
            embeddings_batch_size,
            embeddings_batch_tokens,
        ))
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
    ///
    /// # Errors
    ///
    /// Returns an error if the metrics cannot be registered.
    fn image_generation_registry(registry: &Registry) -> Result<(GaugeVec, GaugeVec)> {
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

        registry.register(Box::new(image_generation_latency.clone()))?;
        registry.register(Box::new(num_running_requests.clone()))?;

        Ok((image_generation_latency, num_running_requests))
    }

    /// Creates a Prometheus query string for chat completions metrics.
    ///
    /// This function generates a Prometheus query string that retrieves the top `top_k` nodes
    /// with the lowest chat completions latency for a given model.
    ///
    /// # Arguments
    ///
    /// * `model` - The model identifier string
    /// * `top_k` - The number of top nodes to retrieve
    ///
    /// # Returns
    ///
    /// A string containing the Prometheus query.
    fn chat_completions_query(model: &str, top_k: usize) -> String {
        format!(
            r#"topk({top_k},
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
        )
    }

    /// Creates a Prometheus query string for embeddings metrics.
    ///
    /// This function generates a Prometheus query string that retrieves the top `top_k` nodes
    /// with the lowest embeddings processing time for a given model. The query uses a scoring formula
    /// that considers:
    ///
    /// - Queue duration: Time spent waiting in queue
    /// - Inference duration: Time spent on actual embedding computation
    /// - Batch efficiency: Penalizes nodes processing fewer tokens per batch
    ///
    /// The formula multiplies the total processing time (queue + inference) by a batch efficiency factor
    /// that increases the score for nodes with lower tokens-per-batch efficiency.
    ///
    /// # Arguments
    ///
    /// * `model` - The model identifier string
    /// * `top_k` - The number of top nodes to retrieve
    ///
    /// # Returns
    ///
    /// A string containing the Prometheus query.
    fn embeddings_query(model: &str, top_k: usize) -> String {
        format!(
            r#"topk({top_k},
                -1 * (
                    (embeddings_queue_duration{{model="{model}"}} + 
                    embeddings_inference_duration{{model="{model}"}}) * 
                    (1 + (embeddings_batch_tokens{{model="{model}"}} / 
                          max(embeddings_batch_size{{model="{model}"}}, 1) / 1000000))
                )
            )"#,
        )
    }

    /// Creates a Prometheus query string for image generation metrics.
    ///
    /// This function generates a Prometheus query string that retrieves the top `top_k` nodes
    /// with the lowest image generation latency for a given model.
    ///
    /// # Arguments
    ///
    /// * `model` - The model identifier string
    /// * `top_k` - The number of top nodes to retrieve
    ///
    /// # Returns
    ///
    /// A string containing the Prometheus query.
    fn image_generation_query(model: &str, top_k: usize) -> String {
        format!(
            r#"topk({top_k},
                -1 * (
                    image_generation_latency{{model="{model}"}}
                )
            )"#,
        )
    }

    /// Resets all the metrics in the Prometheus registry.
    pub fn reset_metrics(&self) {
        self.chat_completions_gpu_kv_cache_usage.reset();
        self.chat_completions_cpu_kv_cache_usage.reset();
        self.chat_completions_ttft.reset();
        self.chat_completions_tpot.reset();
        self.chat_completions_num_running_requests.reset();
        self.chat_completions_num_waiting_requests.reset();
        self.embeddings_queue_duration.reset();
        self.embeddings_inference_duration.reset();
        self.embeddings_input_length.reset();
        self.embeddings_batch_size.reset();
        self.embeddings_batch_tokens.reset();
        self.image_generation_latency.reset();
        self.image_generation_num_running_requests.reset();
    }

    #[cfg(test)]
    #[must_use]
    pub const fn get_chat_completions_gpu_kv_cache_usage(&self) -> &GaugeVec {
        &self.chat_completions_gpu_kv_cache_usage
    }

    #[cfg(test)]
    #[must_use]
    pub const fn get_chat_completions_cpu_kv_cache_usage(&self) -> &GaugeVec {
        &self.chat_completions_cpu_kv_cache_usage
    }

    #[cfg(test)]
    #[must_use]
    pub const fn get_chat_completions_ttft(&self) -> &GaugeVec {
        &self.chat_completions_ttft
    }

    #[cfg(test)]
    #[must_use]
    pub const fn get_chat_completions_tpot(&self) -> &GaugeVec {
        &self.chat_completions_tpot
    }

    #[cfg(test)]
    #[must_use]
    pub const fn get_chat_completions_num_running_requests(&self) -> &GaugeVec {
        &self.chat_completions_num_running_requests
    }

    #[cfg(test)]
    #[must_use]
    pub const fn get_chat_completions_num_waiting_requests(&self) -> &GaugeVec {
        &self.chat_completions_num_waiting_requests
    }

    #[cfg(test)]
    #[must_use]
    pub const fn get_embeddings_queue_duration(&self) -> &GaugeVec {
        &self.embeddings_queue_duration
    }

    #[cfg(test)]
    #[must_use]
    pub const fn get_embeddings_inference_duration(&self) -> &GaugeVec {
        &self.embeddings_inference_duration
    }

    #[cfg(test)]
    #[must_use]
    pub const fn get_embeddings_input_length(&self) -> &GaugeVec {
        &self.embeddings_input_length
    }

    #[cfg(test)]
    #[must_use]
    pub const fn get_embeddings_batch_size(&self) -> &GaugeVec {
        &self.embeddings_batch_size
    }

    #[cfg(test)]
    #[must_use]
    pub const fn get_embeddings_batch_tokens(&self) -> &GaugeVec {
        &self.embeddings_batch_tokens
    }

    #[cfg(test)]
    #[must_use]
    pub const fn get_image_generation_latency(&self) -> &GaugeVec {
        &self.image_generation_latency
    }

    #[cfg(test)]
    #[must_use]
    pub const fn get_image_generation_num_running_requests(&self) -> &GaugeVec {
        &self.image_generation_num_running_requests
    }
}

/// Prometheus query response format following the API description of
/// https://prometheus.io/docs/prometheus/latest/querying/api/
#[derive(Debug, Deserialize)]
struct PromQueryResponse {
    /// The status of the query
    #[allow(dead_code)]
    status: String,
    /// The data of the query
    data: PromData,
}

/// The data of the query
#[derive(Debug, Deserialize)]
struct PromData {
    /// The type of the result
    #[serde(rename = "resultType")]
    #[allow(dead_code)]
    result_type: String,
    /// The result of the query
    result: Vec<PromResult>,
}

/// The result of the query
#[derive(Debug, Deserialize)]
struct PromResult {
    /// The metric of the result with the label name as the key and the label value as the value
    metric: HashMap<String, String>,
    /// The value of the result as a tuple of [timestamp, value_as_string]
    #[allow(dead_code)]
    value: (f64, String),
}
