#![allow(clippy::cognitive_complexity)]
#![allow(clippy::too_many_lines)]

use atoma_state::types::AtomaAtomaStateManagerEvent;
use axum::body::Bytes;
use axum::{response::sse::Event, Error};
use flume::{RecvError, Sender};
use futures::{FutureExt, Stream};
use opentelemetry::KeyValue;
use reqwest;
use serde_json::Value;
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};
use tracing::{error, instrument, trace};

use crate::server::handlers::{
    chat_completions::CHAT_COMPLETIONS_PATH, completions::COMPLETIONS_PATH, update_state_manager,
};

use super::handlers::chat_completions::CONFIDENTIAL_CHAT_COMPLETIONS_PATH;
use super::handlers::metrics::{
    CANCELLED_STREAM_CHAT_COMPLETION_REQUESTS_PER_USER, CHAT_COMPLETIONS_COMPLETIONS_TOKENS,
    CHAT_COMPLETIONS_COMPLETIONS_TOKENS_PER_USER, CHAT_COMPLETIONS_INPUT_TOKENS,
    CHAT_COMPLETIONS_INPUT_TOKENS_PER_USER, CHAT_COMPLETIONS_INTER_TOKEN_GENERATION_TIME,
    CHAT_COMPLETIONS_STREAMING_LATENCY_METRICS, CHAT_COMPLETIONS_TIME_TO_FIRST_TOKEN,
    CHAT_COMPLETIONS_TOTAL_TOKENS, CHAT_COMPLETIONS_TOTAL_TOKENS_PER_USER,
    CHAT_COMPLETION_REQUESTS_PER_USER, TOTAL_COMPLETED_REQUESTS,
};
use super::handlers::{
    update_state_manager_fiat, verify_response_hash_and_signature, RESPONSE_HASH_KEY,
};
use super::ONE_MILLION;

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
    /// Estimated amount for fiat currency.
    fiat_estimated_amount: Option<i64>,
    /// Price per million tokens for this request.
    price_per_million: Option<i64>,
    /// Stack small id
    stack_small_id: Option<i64>,
    /// State manager sender
    state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,
    /// Start time of the request
    start: Instant,
    /// The user id which requested the inference
    user_id: i64,
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
    ///
    /// Either `stack_small_id` or `fiat_estimated_amount` must be provided.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
        state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,
        stack_small_id: Option<i64>,
        num_input_tokens: i64,
        estimated_total_tokens: i64,
        fiat_estimated_amount: Option<i64>,
        price_per_million: Option<i64>,
        start: Instant,
        user_id: i64,
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
            user_id,
            model_name,
            endpoint,
            chunk_buffer: String::new(),
            keep_alive_pos: 0,
            first_token_generation_timer: Some(start),
            inter_stream_token_latency_timer: None,
            is_final_chunk_handled: false,
            num_generated_tokens: num_input_tokens,
            fiat_estimated_amount,
            price_per_million,
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
    fn handle_final_chunk(
        &mut self,
        usage: &Value,
        response_hash: Option<&Value>,
    ) -> Result<(), Error> {
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
        CHAT_COMPLETIONS_TOTAL_TOKENS_PER_USER.add(
            total_tokens as u64,
            &[KeyValue::new("user_id", self.user_id)],
        );
        CHAT_COMPLETIONS_INPUT_TOKENS_PER_USER.add(
            input_tokens as u64,
            &[KeyValue::new("user_id", self.user_id)],
        );
        CHAT_COMPLETIONS_COMPLETIONS_TOKENS_PER_USER.add(
            output_tokens as u64,
            &[KeyValue::new("user_id", self.user_id)],
        );
        match self.stack_small_id {
            Some(stack_small_id) => {
                if let Err(e) = update_state_manager(
                    &self.state_manager_sender,
                    stack_small_id,
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
            }
            None => {
                if let Err(e) = update_state_manager_fiat(
                    &self.state_manager_sender,
                    self.user_id,
                    self.fiat_estimated_amount.unwrap_or_default(),
                    total_tokens * self.price_per_million.unwrap_or_default() / ONE_MILLION as i64,
                    &self.endpoint,
                ) {
                    error!(
                        target = "atoma-service-streamer",
                        level = "error",
                        "Error updating fiat num tokens: {}",
                        e
                    );
                    return Err(Error::new(format!("Error updating fiat num tokens: {e:?}")));
                }
            }
        }
        self.is_final_chunk_handled = true;
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
                    cx.waker().wake_by_ref();
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
                        trace!(
                            target: "atoma-service-streamer",
                            "Partial keep-alive received, current position: {}",
                            self.keep_alive_pos
                        );
                        if self.keep_alive_pos >= KEEP_ALIVE_CHUNK_STR.len() {
                            // Full keep-alive received
                            self.keep_alive_pos = 0;
                            trace!(
                                target: "atoma-service-streamer",
                                "Full keep-alive received, resetting position"
                            );
                        }
                        cx.waker().wake_by_ref();
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
                            trace!(
                                target = "atoma-service-streamer",
                                parse_chunk = "eof_chunk",
                                "EOF reached, pushing chunk to buffer: {}",
                                chunk_str
                            );
                            self.chunk_buffer.push_str(chunk_str);
                            cx.waker().wake_by_ref();
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
                                trace!(
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
                                    cx.waker().wake_by_ref();
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

                if self.endpoint == CHAT_COMPLETIONS_PATH || self.endpoint == COMPLETIONS_PATH {
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

                    if let Some(usage) = chunk.get(USAGE) {
                        if !usage.is_null() {
                            self.status = StreamStatus::Completed;
                            self.handle_final_chunk(usage, chunk.get(RESPONSE_HASH_KEY))?;
                            if !choices.is_empty() {
                                trace!(
                                target = "atoma-service-streamer",
                                level = "trace",
                                "Client disconnected before the final chunk was processed, using usage transmitted by the node last live chunk to update the stack num tokens"
                            );
                            }
                        }
                    }
                } else if self.endpoint == COMPLETIONS_PATH {
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

                    if let Some(usage) = chunk.get(USAGE) {
                        if !usage.is_null() {
                            self.status = StreamStatus::Completed;
                            self.handle_final_chunk(usage, chunk.get(RESPONSE_HASH_KEY))?;
                        }
                        if !choices.is_empty() {
                            trace!(
                                target = "atoma-service-streamer",
                                level = "trace",
                                "Client disconnected before the final chunk was processed, using usage transmitted by the node last live chunk to update the stack num tokens"
                            );
                        }
                    }
                } else if let Some(usage) = chunk.get(USAGE) {
                    if !usage.is_null() {
                        self.status = StreamStatus::Completed;
                        self.handle_final_chunk(usage, chunk.get(RESPONSE_HASH_KEY))?;
                    }
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
    /// Drops the streamer, updating the state manager with the final token count.
    ///
    /// # Arguments
    ///
    /// * `self` - The streamer to drop
    ///
    /// # Returns
    ///
    /// Returns a `Result<(), Error>` where:
    /// * `()` - The streamer is dropped
    /// * `Error` - An error that can occur during:
    ///   - State manager update
    ///
    /// # State Updates
    ///
    /// This method sends an event to the state manager to update the stack num tokens.
    /// If the final chunk has not been handled, it will not update the stack num tokens.
    #[instrument(
        level = "info",
        skip_all,
        fields(
            streamer = "drop-streamer",
            num_generated_tokens = self.num_generated_tokens,
            estimated_total_tokens = self.estimated_total_tokens,
            stack_small_id = self.stack_small_id,
            endpoint = self.endpoint,
        )
    )]
    fn drop(&mut self) {
        if self.is_final_chunk_handled {
            TOTAL_COMPLETED_REQUESTS.add(1, &[KeyValue::new("model", self.model_name.clone())]);
            // Record the request in the chat completions num requests metric
            CHAT_COMPLETIONS_STREAMING_LATENCY_METRICS.record(
                self.start.elapsed().as_secs_f64(),
                &[KeyValue::new("model", self.model_name.clone())],
            );
            CHAT_COMPLETION_REQUESTS_PER_USER.add(1, &[KeyValue::new("user_id", self.user_id)]);
            return;
        }
        CANCELLED_STREAM_CHAT_COMPLETION_REQUESTS_PER_USER.add(
            1,
            &[
                KeyValue::new("user_id", self.user_id),
                KeyValue::new("model", self.model_name.clone()),
            ],
        );
        match self.stack_small_id {
            Some(stack_small_id) => {
                if let Err(e) = update_state_manager(
                    &self.state_manager_sender,
                    stack_small_id,
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
            }
            None => {
                if let Err(e) = update_state_manager_fiat(
                    &self.state_manager_sender,
                    self.user_id,
                    self.fiat_estimated_amount.unwrap_or_default(),
                    self.num_generated_tokens * self.price_per_million.unwrap_or_default()
                        / ONE_MILLION as i64,
                    &self.endpoint,
                ) {
                    error!(
                        target = "atoma-service-streamer",
                        level = "error",
                        "Error updating fiat num tokens: {}",
                        e
                    );
                }
            }
        }
        self.status = StreamStatus::Completed;
    }
}

/// A structure for streaming chat completion chunks from the client.
pub struct ClientStreamer {
    /// The receiver for the event chunks
    pub chunk_receiver: flume::Receiver<Event>,
    /// The sender for the kill signal
    pub kill_signal: flume::Sender<()>,
    /// If the streamer has reached done state
    pub done: bool,
}

impl ClientStreamer {
    /// Creates a new client streamer
    pub const fn new(
        chunk_receiver: flume::Receiver<Event>,
        kill_signal: flume::Sender<()>,
    ) -> Self {
        Self {
            chunk_receiver,
            kill_signal,
            done: false,
        }
    }
}

impl Stream for ClientStreamer {
    type Item = Result<Event, Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let mut future = this.chunk_receiver.recv_async();
        match future.poll_unpin(cx) {
            Poll::Ready(Ok(chunk)) => Poll::Ready(Some(Ok(chunk))),
            Poll::Ready(Err(RecvError::Disconnected)) => {
                trace!(
                    target = "atoma-service-streamer",
                    "ClientStreamer received disconnect signal, marking as done."
                );
                this.done = true;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for ClientStreamer {
    fn drop(&mut self) {
        if !self.done {
            let _ = self.kill_signal.send(());
        }
    }
}
