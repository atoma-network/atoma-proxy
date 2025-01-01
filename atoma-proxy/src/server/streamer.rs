use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
};

use atoma_auth::Sui;
use atoma_state::types::AtomaAtomaStateManagerEvent;
use axum::body::Bytes;
use axum::{response::sse::Event, Error};
use flume::Sender;
use futures::Stream;
use reqwest;
use serde_json::Value;
use sqlx::types::chrono::{DateTime, Utc};
use tokio::sync::RwLockReadGuard;
use tracing::{error, instrument};

use crate::server::handlers::{chat_completions::CHAT_COMPLETIONS_PATH, update_state_manager};

use super::handlers::verify_and_sign_response;

/// The chunk that indicates the end of a streaming response
const DONE_CHUNK: &str = "[DONE]";

/// The prefix for the data chunk
const DATA_PREFIX: &str = "data: ";

/// The keep-alive chunk
const KEEP_ALIVE_CHUNK: &[u8] = b": keep-alive\n\n";

/// The choices key
const CHOICES: &str = "choices";

/// The usage key
const USAGE: &str = "usage";

/// A structure for streaming chat completion chunks.
pub struct Streamer<'a> {
    /// The stream of bytes currently being processed
    stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    /// Current status of the stream
    status: StreamStatus,
    /// Estimated total tokens for the stream
    estimated_total_tokens: i64,
    /// Keystore
    keystore: RwLockReadGuard<'a, Sui>,
    /// Stack small id
    stack_small_id: i64,
    /// State manager sender
    state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,
    /// Start time of the request
    start: Instant,
    /// Start time of the decode
    start_decode: Option<Instant>,
    /// Node id that's running this request
    node_id: i64,
    /// Model name
    model_name: String,
    /// Endpoint
    endpoint: String,
}

/// Represents the various states of a streaming process
#[derive(Debug, PartialEq, Eq)]
pub enum StreamStatus {
    /// Stream has not started
    NotStarted,
    /// Stream is actively receiving data
    Started,
    /// Stream has completed successfully
    Completed,
    /// Stream failed with an error
    Failed(String),
}

impl<'a> Streamer<'a> {
    /// Creates a new Streamer instance
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
        state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,
        stack_small_id: i64,
        estimated_total_tokens: i64,
        keystore: RwLockReadGuard<'a, Sui>,
        start: Instant,
        node_id: i64,
        model_name: String,
        endpoint: String,
    ) -> Self {
        Self {
            stream: Box::pin(stream),
            status: StreamStatus::NotStarted,
            estimated_total_tokens,
            keystore,
            stack_small_id,
            state_manager_sender,
            start,
            start_decode: None,
            node_id,
            model_name,
            endpoint,
        }
    }

    /// Processes the final chunk of a streaming response, performing signature generation,
    /// token counting, and state updates.
    ///
    /// This method:
    /// 1. Signs the accumulated response data
    /// 2. Extracts and validates token usage information
    /// 3. Updates the state manager with token counts
    /// 4. Calculates a total hash combining payload and response hashes
    /// 5. Updates the state manager with the total hash
    /// 6. Creates a final SSE message containing signature and metadata
    ///
    /// # Arguments
    ///
    /// * `usage` - A JSON Value containing token usage information, expected to have a
    ///             "total_tokens" field with an integer value
    ///
    /// # Returns
    ///
    /// Returns a `Result<Event, Error>` where:
    /// * `Event` - An SSE event containing the final message with signature
    /// * `Error` - An error that can occur during:
    ///   - Response signing
    ///   - Token usage extraction
    ///   - JSON serialization
    ///
    /// # State Updates
    ///
    /// This method sends two events to the state manager:
    /// * `UpdateStackNumTokens` - Updates the token count for the stack
    /// * `UpdateStackTotalHash` - Updates the combined hash of payload and response
    #[instrument(
        level = "info",
        skip(self, usage),
        fields(
            endpoint = "handle_final_chunk",
            estimated_total_tokens = self.estimated_total_tokens,
        )
    )]
    fn handle_final_chunk(&mut self, usage: &Value) -> Result<(), Error> {
        // Get input tokens
        let input_tokens = usage
            .get("prompt_tokens")
            .and_then(|t| t.as_i64())
            .ok_or_else(|| {
                error!("Error getting prompt tokens from usage");
                Error::new("Error getting prompt tokens from usage")
            })?;
        // Get output tokens
        let output_tokens = usage
            .get("completion_tokens")
            .and_then(|t| t.as_i64())
            .ok_or_else(|| {
                error!("Error getting completion tokens from usage");
                Error::new("Error getting completion tokens from usage")
            })?;
        // Get total tokens
        let total_tokens = usage
            .get("total_tokens")
            .and_then(|t| t.as_i64())
            .ok_or_else(|| {
                error!("Error getting total tokens from usage");
                Error::new("Error getting total tokens from usage")
            })?;

        // Update the nodes throughput performance
        if let Err(e) = self.state_manager_sender.send(
            AtomaAtomaStateManagerEvent::UpdateNodeThroughputPerformance {
                timestamp: DateTime::<Utc>::from(std::time::SystemTime::now()),
                model_name: self.model_name.clone(),
                node_small_id: self.node_id,
                input_tokens,
                output_tokens,
                time: self.start.elapsed().as_secs_f64(),
            },
        ) {
            error!("Error updating node throughput performance: {}", e);
            return Err(Error::new(format!(
                "Error updating node throughput performance: {}",
                e
            )));
        }

        if let Err(e) = self.state_manager_sender.send(
            AtomaAtomaStateManagerEvent::UpdateNodeDecodePerformance {
                node_small_id: self.node_id,
                tokens: output_tokens,
                time: self
                    .start_decode
                    .expect("This should be filled on the first token")
                    .elapsed()
                    .as_secs_f64(),
            },
        ) {
            error!("Error updating node decode performance: {}", e);
            return Err(Error::new(format!(
                "Error updating node decode performance: {}",
                e
            )));
        }
        if let Err(e) = self.state_manager_sender.send(
            AtomaAtomaStateManagerEvent::UpdateNodePrefillPerformance {
                node_small_id: self.node_id,
                tokens: input_tokens,
                time: (self.start_decode.unwrap() - self.start).as_secs_f64(),
            },
        ) {
            error!("Error updating node prefill performance: {}", e);
            return Err(Error::new(format!(
                "Error updating node prefill performance: {}",
                e
            )));
        }

        // Update stack num tokens
        if let Err(e) = update_state_manager(
            &self.state_manager_sender,
            self.stack_small_id,
            self.estimated_total_tokens,
            total_tokens,
            &self.endpoint,
        ) {
            error!("Error updating stack num tokens: {}", e);
            return Err(Error::new(format!(
                "Error updating stack num tokens: {}",
                e
            )));
        }

        Ok(())
    }
}

impl<'a> Stream for Streamer<'a> {
    type Item = Result<Event, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.status == StreamStatus::Completed {
            return Poll::Ready(None);
        }

        match self.stream.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                if self.status != StreamStatus::Started {
                    self.status = StreamStatus::Started;
                }

                if chunk.as_ref() == KEEP_ALIVE_CHUNK {
                    return Poll::Pending;
                }

                let chunk_str = match std::str::from_utf8(&chunk) {
                    Ok(v) => v,
                    Err(e) => {
                        error!(
                            target = "atoma-service",
                            level = "error",
                            "Invalid UTF-8 sequence: {}",
                            e
                        );
                        return Poll::Ready(Some(Err(Error::new(format!(
                            "Invalid UTF-8 sequence: {}",
                            e
                        )))));
                    }
                };

                let chunk_str = chunk_str.strip_prefix(DATA_PREFIX).unwrap_or(chunk_str);

                if chunk_str.starts_with(DONE_CHUNK) {
                    // This is the last chunk, meaning the inference streaming is complete
                    self.status = StreamStatus::Completed;
                    return Poll::Ready(None);
                }

                let chunk = serde_json::from_str::<Value>(chunk_str).map_err(|e| {
                    error!(
                        target = "atoma-service",
                        level = "error",
                        "Error parsing chunk {chunk_str}: {}",
                        e
                    );
                    Error::new(format!("Error parsing chunk: {}", e))
                })?;

                if self.start_decode.is_none() {
                    self.start_decode = Some(Instant::now());
                    let latency = self.start.elapsed().as_secs_f64();
                    self.state_manager_sender
                        .send(AtomaAtomaStateManagerEvent::UpdateNodeLatencyPerformance {
                            timestamp: DateTime::<Utc>::from(std::time::SystemTime::now()), // Convert to chrono::DateTime<Utc>
                            node_small_id: self.node_id,
                            latency,
                        })
                        .map_err(|e| {
                            error!("Error updating node latency performance: {}", e);
                            Error::new(format!("Error updating node latency performance: {}", e))
                        })?;
                }

                if self.endpoint == CHAT_COMPLETIONS_PATH {
                    let choices = match chunk.get(CHOICES).and_then(|choices| choices.as_array()) {
                        Some(choices) => choices,
                        None => {
                            error!("Error getting choices from chunk");
                            return Poll::Ready(Some(Err(Error::new(
                                "Error getting choices from chunk",
                            ))));
                        }
                    };

                    if choices.is_empty() {
                        if let Some(usage) = chunk.get(USAGE) {
                            self.status = StreamStatus::Completed;
                            self.handle_final_chunk(usage)?;
                        }
                    }
                } else if let Some(usage) = chunk.get(USAGE) {
                    self.status = StreamStatus::Completed;
                    verify_and_sign_response(&chunk, self.keystore.get_keystore())
                        .map_err(|e| Error::new(e.to_string()))?;
                    self.handle_final_chunk(usage)?;
                }

                Poll::Ready(Some(Ok(Event::default().json_data(&chunk)?)))
            }
            Poll::Ready(Some(Err(e))) => {
                self.status = StreamStatus::Failed(e.to_string());
                Poll::Ready(None)
            }
            Poll::Ready(None) => {
                self.status = StreamStatus::Completed;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
