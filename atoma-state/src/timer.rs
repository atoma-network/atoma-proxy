use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};
use tracing::instrument;

use crate::{
    errors::MetricsServiceError,
    metrics::{NodeMetricsCollector, DURATION_UNTIL_NEXT_TOP_K_BEST_AVAILABLE_NODES_SELECTION},
};

type Result<T> = std::result::Result<T, MetricsServiceError>;

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
