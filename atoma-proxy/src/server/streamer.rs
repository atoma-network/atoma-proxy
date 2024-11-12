use std::{
    pin::Pin,
    task::{Context, Poll},
};

use atoma_state::types::AtomaAtomaStateManagerEvent;
use axum::body::Bytes;
use axum::{response::sse::Event, Error};
use flume::Sender;
use futures::Stream;
use reqwest;
use serde_json::Value;
use tracing::{error, instrument};

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
    pub fn new(
        stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
        state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,
        stack_small_id: i64,
        estimated_total_tokens: i64,
    ) -> Self {
        Self {
            stream: Box::pin(stream),
            status: StreamStatus::NotStarted,
            estimated_total_tokens,
            stack_small_id,
            state_manager_sender,
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
        // Get total tokens
        let total_tokens = usage
            .get("total_tokens")
            .and_then(|t| t.as_i64())
            .ok_or_else(|| {
                error!("Error getting total tokens from usage");
                Error::new("Error getting total tokens from usage")
            })?;

        // Update stack num tokens
        if let Err(e) =
            self.state_manager_sender
                .send(AtomaAtomaStateManagerEvent::UpdateStackNumTokens {
                    stack_small_id: self.stack_small_id,
                    estimated_total_tokens: self.estimated_total_tokens,
                    total_tokens,
                })
        {
            error!("Error updating stack num tokens: {}", e);
            return Err(Error::new(format!(
                "Error updating stack num tokens: {}",
                e
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
                        error!("Invalid UTF-8 sequence: {}", e);
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

                let chunk = serde_json::from_slice::<Value>(chunk_str.as_bytes()).map_err(|e| {
                    error!("Error parsing chunk: {}", e);
                    Error::new(format!("Error parsing chunk: {}", e))
                })?;
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
