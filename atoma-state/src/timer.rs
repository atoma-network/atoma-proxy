use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};
use tracing::instrument;

use crate::{errors::MetricsServiceError, metrics::NodeMetricsCollector};

/// Duration until next top k best available nodes selection
const DURATION_UNTIL_NEXT_TOP_K_BEST_AVAILABLE_NODES_SELECTION: std::time::Duration =
    std::time::Duration::from_secs(3 * 60); // 3 minutes

type Result<T> = std::result::Result<T, MetricsServiceError>;

/// Spawns a task that periodically collects metrics for different modalities and their models.
///
/// This function creates a background task that runs indefinitely until a shutdown signal is received.
/// At regular intervals (defined by `DURATION_UNTIL_NEXT_TOP_K_BEST_AVAILABLE_NODES_SELECTION`), it
/// collects metrics for each modality-model pair and sends the best available nodes through a channel.
///
/// # Arguments
///
/// * `metrics_url` - The URL endpoint where metrics can be collected from
/// * `models` - A vector of tuples containing modality types and their corresponding model identifiers
/// * `top_k` - Optional parameter to limit the number of best nodes returned
/// * `tx_best_available_nodes` - Channel sender for transmitting the collected best available nodes
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
        metrics_url = %metrics_url,
        models = ?models,
        top_k = ?top_k
    )
)]
pub fn trigger_new_metrics_collection_task(
    metrics_url: String,
    models: Vec<(Modalities, String)>,
    top_k: Option<usize>,
    tx_best_available_nodes: mpsc::UnboundedSender<Vec<(Modalities, Vec<i64>)>>,
    mut shutdown_signal: watch::Receiver<bool>,
) -> JoinHandle<Result<()>> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(DURATION_UNTIL_NEXT_TOP_K_BEST_AVAILABLE_NODES_SELECTION) => {
                    let futures = models
                        .iter()
                        .map(|(modality, modality_model)| {
                            let metrics_url = metrics_url.clone();
                            async move {
                                let best_available_nodes = match modality {
                                    Modalities::ChatCompletions => {
                                        NodeMetricsCollector::retrieve_best_available_nodes_for_chat_completions(
                                            &metrics_url,
                                            &modality_model,
                                            top_k,
                                        )
                                        .await?
                                    }
                                    Modalities::Embeddings => {
                                        NodeMetricsCollector::retrieve_best_available_nodes_for_embeddings(
                                            &metrics_url,
                                            modality_model,
                                            top_k,
                                        )
                                        .await?
                                    }
                                    Modalities::ImageGeneration => {
                                        NodeMetricsCollector::retrieve_best_available_nodes_for_image_generation(
                                            &metrics_url,
                                            modality_model,
                                            top_k,
                                        )
                                        .await?
                                    }
                                };

                                Ok::<_, MetricsServiceError>((*modality, best_available_nodes))
                            }
                        });

                    let best_available_nodes = futures::future::try_join_all(futures).await?;
                    tx_best_available_nodes.send(best_available_nodes)?;
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
    })
}

/// The modalities that can be used to collect metrics.
#[derive(Debug, Clone, Copy)]
pub enum Modalities {
    ChatCompletions,
    Embeddings,
    ImageGeneration,
}
