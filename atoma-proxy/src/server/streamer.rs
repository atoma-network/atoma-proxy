#![allow(clippy::cognitive_complexity)]
#![allow(clippy::too_many_lines)]

use atoma_state::types::AtomaAtomaStateManagerEvent;
use axum::body::Bytes;
use axum::{response::sse::Event, Error};
use flume::Sender;
use futures::Stream;
use opentelemetry::KeyValue;
use reqwest;
use serde_json::Value;
use sqlx::types::chrono::{DateTime, Utc};
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};
use tracing::{error, info, instrument, warn};

use crate::server::handlers::{chat_completions::CHAT_COMPLETIONS_PATH, update_state_manager};

use super::handlers::chat_completions::CONFIDENTIAL_CHAT_COMPLETIONS_PATH;
use super::handlers::metrics::{
    CHAT_COMPLETIONS_COMPLETIONS_TOKENS, CHAT_COMPLETIONS_INPUT_TOKENS,
    CHAT_COMPLETIONS_INTER_TOKEN_GENERATION_TIME, CHAT_COMPLETIONS_TIME_TO_FIRST_TOKEN,
    CHAT_COMPLETIONS_TOTAL_TOKENS,
};
use super::handlers::verify_response_hash_and_signature;

/// The chunk that indicates the end of a streaming response
const DONE_CHUNK: &str = "[DONE]";

/// The prefix for the data chunk
const DATA_PREFIX: &str = "data: ";

/// The keep-alive chunk
const KEEP_ALIVE_CHUNK: &[u8] = b": keep-alive\n\n";

/// The keep-alive chunk as a string
const KEEP_ALIVE_CHUNK_STR: &str = ": keep-alive\n\n";

/// The choices key
const CHOICES: &str = "choices";

/// The usage key
const USAGE: &str = "usage";

/// A structure for streaming chat completion chunks.
pub struct Streamer {
    /// The stream of bytes currently being processed
    stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    /// Current status of the stream
    status: StreamStatus,
    /// Estimated total tokens for the stream
    estimated_total_tokens: i64,
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
    /// A chunk buffer (needed as some chunks might be split into multiple parts)
    chunk_buffer: String,
    /// The position in the keep-alive chunk buffer
    keep_alive_pos: usize,
    /// Timer for measuring time between token generations
    inter_stream_token_latency_timer: Option<Instant>,
    /// The first token generation (prefill phase) timer for the request.
    /// We need store it as an option because we need to consume its value
    /// once the first token is generated
    first_token_generation_timer: Option<Instant>,
    /// Whether the final chunk has been handled, this is used to prevent
    /// updating the stack num tokens if the final chunk has not been handled
    /// in case the client has dropped the connection.
    is_final_chunk_handled: bool,
    /// Number of generated tokens so far. It is only used when the client
    /// drops the connection, before the final chunk is processed.
    num_generated_tokens: i64,
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

impl Streamer {
    /// Creates a new Streamer instance
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
        state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,
        stack_small_id: i64,
        num_input_tokens: i64,
        estimated_total_tokens: i64,
        start: Instant,
        node_id: i64,
        model_name: String,
        endpoint: String,
    ) -> Self {
        Self {
            stream: Box::pin(stream),
            status: StreamStatus::NotStarted,
            estimated_total_tokens,
            stack_small_id,
            state_manager_sender,
            start,
            start_decode: None,
            node_id,
            model_name,
            endpoint,
            chunk_buffer: String::new(),
            keep_alive_pos: 0,
            first_token_generation_timer: Some(start),
            inter_stream_token_latency_timer: None,
            is_final_chunk_handled: false,
            num_generated_tokens: num_input_tokens,
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
        let input_tokens = usage
            .get("prompt_tokens")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                error!(
                    target = "atoma-service-streamer",
                    level = "error",
                    "Error getting prompt tokens from usage"
                );
                Error::new("Error getting prompt tokens from usage")
            })?;
        let output_tokens = usage
            .get("completion_tokens")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                error!(
                    target = "atoma-service-streamer",
                    level = "error",
                    "Error getting completion tokens from usage"
                );
                Error::new("Error getting completion tokens from usage")
            })?;
        let total_tokens = usage
            .get("total_tokens")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                error!(
                    target = "atoma-service-streamer",
                    level = "error",
                    "Error getting total tokens from usage"
                );
                Error::new("Error getting total tokens from usage")
            })?;
        CHAT_COMPLETIONS_TOTAL_TOKENS.add(
            total_tokens as u64,
            &[KeyValue::new("model", self.model_name.clone())],
        );
        CHAT_COMPLETIONS_INPUT_TOKENS.add(
            input_tokens as u64,
            &[KeyValue::new("model", self.model_name.clone())],
        );
        CHAT_COMPLETIONS_COMPLETIONS_TOKENS.add(
            output_tokens as u64,
            &[KeyValue::new("model", self.model_name.clone())],
        );
        if let Err(e) = update_state_manager(
            &self.state_manager_sender,
            self.stack_small_id,
            self.estimated_total_tokens,
            total_tokens,
            &self.endpoint,
        ) {
            error!(
                target = "atoma-service-streamer",
                level = "error",
                "Error updating stack num tokens: {}",
                e
            );
            return Err(Error::new(format!(
                "Error updating stack num tokens: {e:?}"
            )));
        }
        Ok(())
    }
}

impl Stream for Streamer {
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
                            "Invalid UTF-8 sequence: {e:?}"
                        )))));
                    }
                };

                if let Some(remaining) = KEEP_ALIVE_CHUNK_STR.get(self.keep_alive_pos..) {
                    if remaining.starts_with(chunk_str) {
                        self.keep_alive_pos += chunk_str.len();
                        info!(
                            target: "atoma-service-streamer",
                            "Partial keep-alive received, current position: {}",
                            self.keep_alive_pos
                        );
                        if self.keep_alive_pos >= KEEP_ALIVE_CHUNK_STR.len() {
                            // Full keep-alive received
                            self.keep_alive_pos = 0;
                            info!(
                                target: "atoma-service-streamer",
                                "Full keep-alive received, resetting position"
                            );
                        }
                        return Poll::Pending;
                    } else if self.keep_alive_pos > 0 {
                        // Reset position if we had partial match but current chunk doesn't continue it
                        error!(
                            target = "atoma-service-streamer",
                            level = "error",
                            "Keep-alive chunk interrupted by non-matching chunk, resetting position"
                        );
                        return Poll::Ready(Some(Err(Error::new(format!(
                            "Keep-alive chunk interrupted by non-matching chunk, {chunk_str}",
                        )))));
                    }
                }

                let chunk_str = chunk_str.strip_prefix(DATA_PREFIX).unwrap_or(chunk_str);

                // Observe the first token generation timer
                if let Some(timer) = self.first_token_generation_timer.take() {
                    CHAT_COMPLETIONS_TIME_TO_FIRST_TOKEN.record(
                        timer.elapsed().as_secs_f64(),
                        &[KeyValue::new("model", self.model_name.clone())],
                    );
                }

                if chunk_str.starts_with(DONE_CHUNK) {
                    // This is the last chunk, meaning the inference streaming is complete
                    self.status = StreamStatus::Completed;
                    return Poll::Ready(None);
                }

                let chunk = match serde_json::from_str::<Value>(chunk_str) {
                    Ok(chunk) => {
                        if let Some(timer) = self.inter_stream_token_latency_timer.take() {
                            let elapsed = timer.elapsed();
                            CHAT_COMPLETIONS_INTER_TOKEN_GENERATION_TIME.record(
                                elapsed.as_secs_f64(),
                                &[KeyValue::new("model", self.model_name.clone())],
                            );
                        }

                        // Start the timer after we've processed this chunk
                        self.inter_stream_token_latency_timer = Some(Instant::now());

                        if !self.chunk_buffer.is_empty() {
                            error!(
                                target = "atoma-service-streamer",
                                level = "error",
                                "Error parsing previous chunk(s), as chunk buffer is not empty: {}",
                                self.chunk_buffer
                            );
                            self.chunk_buffer.clear();
                        }
                        chunk
                    }
                    Err(e) => {
                        if e.is_eof() {
                            info!(
                                target = "atoma-service-streamer",
                                parse_chunk = "eof_chunk",
                                "EOF reached, pushing chunk to buffer: {}",
                                chunk_str
                            );
                            self.chunk_buffer.push_str(chunk_str);
                            return Poll::Pending;
                        }

                        if self.chunk_buffer.is_empty() {
                            error!(
                                target = "atoma-service-streamer",
                                level = "error",
                                "Error parsing chunk {chunk_str}: {}",
                                e
                            );
                            return Poll::Ready(Some(Err(Error::new(format!(
                                "Error parsing chunk: {e:?}"
                            )))));
                        }

                        self.chunk_buffer.push_str(chunk_str);
                        match serde_json::from_str::<Value>(&self.chunk_buffer) {
                            Ok(chunk) => {
                                info!(
                                    target = "atoma-service-streamer",
                                    parse_chunk = "eof_chunk",
                                    "Chunk parsed successfully, clearing buffer: {}",
                                    self.chunk_buffer
                                );
                                self.chunk_buffer.clear();
                                chunk
                            }
                            Err(e) => {
                                if e.is_eof() {
                                    // NOTE: We don't need to push the chunk to the buffer, as it was pushed already
                                    return Poll::Pending;
                                }
                                error!(
                                    target = "atoma-service-streamer",
                                    level = "error",
                                    "Error parsing chunk {}: {}",
                                    self.chunk_buffer,
                                    e
                                );
                                self.chunk_buffer.clear();
                                return Poll::Ready(Some(Err(Error::new(format!(
                                    "Error parsing chunk: {e:?}"
                                )))));
                            }
                        }
                    }
                };

                // We need to verify the hash when the endpoint is not confidential, as the node
                // is not running within a secure enclave. Otherwise, the fact that the node can process requests
                // with confidential data is proof of data integrity.
                let verify_hash = self.endpoint != CONFIDENTIAL_CHAT_COMPLETIONS_PATH;
                verify_response_hash_and_signature(&chunk, verify_hash).map_err(|e| {
                    error!(
                        target = "atoma-service-streamer",
                        level = "error",
                        "Error verifying response: {}",
                        e
                    );
                    Error::new(format!("Error verifying and signing response: {e:?}"))
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
                            error!(
                                target = "atoma-service-streamer",
                                level = "error",
                                "Error updating node latency performance: {}",
                                e
                            );
                            Error::new(format!("Error updating node latency performance: {e:?}"))
                        })?;
                }

                if self.endpoint == CHAT_COMPLETIONS_PATH {
                    let Some(choices) = chunk.get(CHOICES).and_then(|choices| choices.as_array())
                    else {
                        error!(
                            target = "atoma-service-streamer",
                            level = "error",
                            "Error getting choices from chunk"
                        );
                        return Poll::Ready(Some(Err(Error::new(
                            "Error getting choices from chunk",
                        ))));
                    };

                    if choices.is_empty() {
                        if let Some(usage) = chunk.get(USAGE) {
                            self.status = StreamStatus::Completed;
                            self.handle_final_chunk(usage)?;
                        }
                    }
                } else if let Some(usage) = chunk.get(USAGE) {
                    self.status = StreamStatus::Completed;
                    self.handle_final_chunk(usage)?;
                }
                self.num_generated_tokens += 1;
                Poll::Ready(Some(Ok(Event::default().json_data(&chunk)?)))
            }
            Poll::Ready(Some(Err(e))) => {
                self.status = StreamStatus::Failed(e.to_string());
                Poll::Ready(None)
            }
            Poll::Ready(None) => {
                if !self.chunk_buffer.is_empty() {
                    error!(
                        target = "atoma-service-streamer",
                        level = "error",
                        "Stream ended, but the chunk buffer is not empty, this should not happen: {}",
                        self.chunk_buffer
                    );
                }
                self.status = StreamStatus::Completed;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for Streamer {
    fn drop(&mut self) {
        if self.is_final_chunk_handled {
            return;
        }
        if let Err(e) = update_state_manager(
            &self.state_manager_sender,
            self.stack_small_id,
            self.estimated_total_tokens,
            self.num_generated_tokens,
            &self.endpoint,
        ) {
            error!(
                target = "atoma-service-streamer",
                level = "error",
                "Error updating stack num tokens: {}",
                e
            );
        }
        self.status = StreamStatus::Completed;
    }
}
