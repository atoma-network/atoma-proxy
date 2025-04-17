use once_cell::sync::Lazy;
use opentelemetry::{
    global,
    metrics::{Counter, Histogram, Meter},
};

// Add global metrics
static GLOBAL_METER: Lazy<Meter> = Lazy::new(|| global::meter("atoma-proxy"));

const LATENCY_HISTOGRAM_BUCKETS: [f64; 15] = [
    0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0,
];

/// Counter metric that tracks the total number of chat completion requests.
///
/// This metric counts the number of incoming requests for chat completions,
/// broken down by model type. This helps monitor usage patterns and load
/// across different models.
///
/// # Metric Details
/// - Name: `atoma_chat_completions_num_requests`
/// - Type: Counter
/// - Labels: `model`
/// - Unit: requests (count)
pub static CHAT_COMPLETIONS_NUM_REQUESTS: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_chat_completions_num_requests")
        .with_description("The number of incoming requests for chat completions tasks")
        .with_unit("s")
        .build()
});

/// Counter metric that tracks the total number of image generation requests.
///
/// This metric counts the number of incoming requests for image generations,
/// broken down by model type. This helps monitor usage patterns and load
/// across different image generation models.
///
/// # Metric Details
/// - Name: `atoma_image_gen_num_requests`
/// - Type: Counter
/// - Labels: `model`
/// - Unit: requests (count)
pub static IMAGE_GEN_NUM_REQUESTS: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_image_gen_num_requests")
        .with_description("The number of incoming requests for image generation tasks")
        .with_unit("requests")
        .build()
});

/// Counter metric that tracks the total number of text embedding requests.
///
/// This metric counts the number of incoming requests for text embeddings,
/// broken down by model type. This helps monitor usage patterns and load
/// across different embedding models.
///
/// # Metric Details
/// - Name: `atoma_text_embs_num_requests`
/// - Type: Counter
/// - Labels: `model`
/// - Unit: requests (count)
pub static TEXT_EMBEDDINGS_NUM_REQUESTS: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_text_embs_num_requests")
        .with_description("The number of incoming requests for text embeddings tasks")
        .with_unit("requests")
        .build()
});

/// Histogram metric that tracks the latency of chat completion token generation.
///
/// This metric measures the time taken to generate each token during chat completions,
/// broken down by model type. The histogram buckets range from 10ms to 10 minutes to
/// capture both fast and slow token generation scenarios.
///
/// # Metric Details
/// - Name: `atoma_chat_completions_token_latency`
/// - Type: Histogram
/// - Labels: `model`
/// - Unit: seconds
/// - Buckets: [0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0]
pub static CHAT_COMPLETIONS_LATENCY_METRICS: Lazy<Histogram<f64>> = Lazy::new(|| {
    GLOBAL_METER
        .f64_histogram("atoma_chat_completions_token_latency")
        .with_description("The latency of chat completion generation in seconds")
        .with_unit("s")
        .with_boundaries(LATENCY_HISTOGRAM_BUCKETS.to_vec())
        .build()
});

/// Histogram metric that tracks the latency of chat completion streaming requests.
///
/// This metric measures the time taken to stream chat completions, broken down by model type.
/// The histogram buckets range from 1ms to 10 minutes to capture both fast and slow
/// streaming scenarios.
///
/// # Metric Details
/// - Name: `atoma_chat_completions_streaming_latency`
/// - Type: Histogram
/// - Labels: `model`
/// - Unit: seconds
/// - Buckets: [0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0]
pub static CHAT_COMPLETIONS_STREAMING_LATENCY_METRICS: Lazy<Histogram<f64>> = Lazy::new(|| {
    GLOBAL_METER
        .f64_histogram("atoma_chat_completions_streaming_latency")
        .with_description("The latency of chat completion streaming in seconds")
        .with_unit("s")
        .build()
});

/// Histogram metric that tracks the latency of image generation requests.
///
/// This metric measures the time taken to generate images, broken down by model type.
/// The histogram buckets range from 1ms to 10 minutes to capture both fast and slow
/// generation scenarios.
///
/// # Metric Details
/// - Name: `atoma_image_generation_latency`
/// - Type: Histogram
/// - Labels: `model`
/// - Unit: seconds
/// - Buckets: [0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0]
pub static IMAGE_GEN_LATENCY_METRICS: Lazy<Histogram<f64>> = Lazy::new(|| {
    GLOBAL_METER
        .f64_histogram("atoma_image_generation_latency")
        .with_description("The latency of image generation in seconds")
        .with_unit("s")
        .with_boundaries(LATENCY_HISTOGRAM_BUCKETS.to_vec())
        .build()
});

/// Histogram metric that tracks the latency of text embedding requests.
///
/// This metric measures the time taken to generate text embeddings, broken down by model type.
/// The histogram buckets range from 1ms to 10 minutes to capture both fast and slow
/// embedding generation scenarios.
///
/// # Metric Details
/// - Name: `atoma_text_embeddings_latency`
/// - Type: Histogram
/// - Labels: `model`
/// - Unit: seconds
/// - Buckets: [0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0]
pub static TEXT_EMBEDDINGS_LATENCY_METRICS: Lazy<Histogram<f64>> = Lazy::new(|| {
    GLOBAL_METER
        .f64_histogram("atoma_text_embeddings_latency")
        .with_description("The latency of text embeddings in seconds")
        .with_unit("s")
        .with_boundaries(LATENCY_HISTOGRAM_BUCKETS.to_vec())
        .build()
});

/// Counter metric that tracks the total number of tokens processed in chat completions.
///
/// This metric counts the cumulative number of tokens in the input prompts,
/// broken down by model type. This helps monitor token usage and costs
/// across different models and client applications.
///
/// # Metric Details
/// - Name: `atoma_chat_completions_input_tokens_metrics`
/// - Type: Counter
/// - Labels:
///   - `model`: The model used for completion
/// - Unit: tokens (count)
pub static CHAT_COMPLETIONS_TOTAL_TOKENS: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_chat_completions_total_tokens")
        .with_description("The estimated total number of tokens processed")
        .with_unit("tokens")
        .build()
});

/// Counter metric that tracks the total number of input tokens processed in chat completions.
///
/// This metric counts the cumulative number of tokens in the input prompts,
/// broken down by model type. This helps monitor token usage and costs
/// across different models and client applications.
///
/// # Metric Details
/// - Name: `atoma_chat_completions_input_tokens`
/// - Type: Counter
/// - Labels: `model`
/// - Unit: tokens (count)
pub static CHAT_COMPLETIONS_INPUT_TOKENS: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_chat_completions_input_tokens")
        .with_description("The number of input tokens processed")
        .with_unit("tokens")
        .build()
});

/// Counter metric that tracks the total number of completions tokens processed in chat completions.
///
/// This metric counts the cumulative number of tokens in the completions,
/// broken down by model type. This helps monitor token usage and costs
/// across different models and client applications.
///
/// # Metric Details
/// - Name: `atoma_chat_completions_completions_tokens`
/// - Type: Counter
/// - Labels: `model`
/// - Unit: tokens (count)
pub static CHAT_COMPLETIONS_COMPLETIONS_TOKENS: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_chat_completions_completions_tokens")
        .with_description("The number of completions tokens processed")
        .with_unit("tokens")
        .build()
});

/// Histogram metric that tracks the time until the first token is generated in chat completions.
///
/// This metric measures the initial latency before token generation begins,
/// broken down by model type. The histogram buckets range from 0.1ms to 30 seconds to
/// capture both very fast and slow initial response times.
///
/// # Metric Details
/// - Name: `atoma_chat_completions_time_to_first_token`
/// - Type: Histogram
/// - Labels: `model`
/// - Unit: seconds
/// - Buckets: [[0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0]
pub static CHAT_COMPLETIONS_TIME_TO_FIRST_TOKEN: Lazy<Histogram<f64>> = Lazy::new(|| {
    GLOBAL_METER
        .f64_histogram("atoma_chat_completions_time_to_first_token")
        .with_description("Time taken until first token is generated in seconds")
        .with_unit("s")
        .with_boundaries(LATENCY_HISTOGRAM_BUCKETS.to_vec())
        .build()
});

/// Histogram metric that tracks the time taken between each token generation phase in chat completions.
///
/// This metric measures the time taken between each token generation phase,
/// broken down by model type. The histogram buckets range from 0.1ms to 30 seconds to
/// capture both very fast and slow intra-token generation scenarios.
///
/// # Metric Details
/// - Name: `atoma_chat_completions_intra_token_generation_time`
/// - Type: Histogram
/// - Labels: `model`
/// - Unit: seconds
/// - Buckets: [0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0]
pub static CHAT_COMPLETIONS_INTER_TOKEN_GENERATION_TIME: Lazy<Histogram<f64>> = Lazy::new(|| {
    GLOBAL_METER
        .f64_histogram("atoma_chat_completions_intra_token_generation_time")
        .with_description("Time taken to stream between each token generation phase in seconds")
        .with_unit("s")
        .with_boundaries(LATENCY_HISTOGRAM_BUCKETS.to_vec())
        .build()
});

/// Counter metrics that tracks the total number of successfully completed requests (including chat completions, image generation, and text embeddings)
///
/// # Metric Details
/// - Name: `atoma_total_completed_requests`
/// - Type: Counter
/// - Labels: `model`
/// - Unit: requests (count)
pub static TOTAL_COMPLETED_REQUESTS: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_total_completed_requests")
        .with_description("Total number of successfully completed requests")
        .with_unit("requests")
        .build()
});

/// Counter metric that tracks the total number of failed requests (including chat completions, image generation, and text embeddings)
///
/// # Metric Details
/// - Name: `atoma_total_failed_requests`
/// - Type: Counter
/// - Labels: `model`
/// - Unit: requests (count)
pub static TOTAL_FAILED_REQUESTS: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_total_failed_requests")
        .with_description("Total number of failed requests")
        .with_unit("requests")
        .build()
});

/// Counter metric that tracks the total number of failed chat requests.
///
/// # Metric Details
/// - Name: `atoma_total_failed_chat_requests`
/// - Type: Counter
/// - Labels: `model`
/// - Unit: requests (count)
pub static TOTAL_FAILED_CHAT_REQUESTS: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_total_failed_chat_requests")
        .with_description("Total number of failed chat requests")
        .with_unit("requests")
        .build()
});

/// Counter metric that tracks the total number of failed image generation requests.
///
/// # Metric Details
/// - Name: `atoma_total_failed_image_generation_requests`
/// - Type: Counter
/// - Labels: `model`
/// - Unit: requests (count)
pub static TOTAL_FAILED_IMAGE_GENERATION_REQUESTS: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_total_failed_image_generation_requests")
        .with_description("Total number of failed image generation requests")
        .with_unit("requests")
        .build()
});

/// Counter metric that tracks the total number of failed text embedding requests.
///
/// # Metric Details
/// - Name: `atoma_total_failed_text_embedding_requests`
/// - Type: Counter
/// - Labels: `model`
/// - Unit: requests (count)
pub static TOTAL_FAILED_TEXT_EMBEDDING_REQUESTS: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_total_failed_text_embedding_requests")
        .with_description("Total number of failed text embedding requests")
        .with_unit("requests")
        .build()
});

/// Counter metric that tracks the total number of stack unavailable errors.
///
/// # Metric Details
/// - Name: `atoma_stack_unavailable_counter`
/// - Type: Counter
/// - Labels: `stack_small_id`
/// - Unit: Unavailable counts
pub static STACK_UNAVAILABLE_COUNTER: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_stack_unavailable_counter")
        .with_description("Total number of stack unavailable errors")
        .with_unit("requests")
        .build()
});

/// Counter metric that tracks the total number of stack locked errors.
///
/// # Metric Details
/// - Name: `atoma_stack_locked_counter`
/// - Type: Counter
/// - Labels: `stack_small_id`
/// - Unit: Locked counts
pub static STACK_LOCKED_COUNTER: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_stack_locked_counter")
        .with_description("Total number of stack locked errors")
        .with_unit("requests")
        .build()
});

/// Counter metric that tracks the total number of stack requests.
///
/// # Metric Details
/// - Name: `atoma_stack_num_requests_counter`
/// - Type: Counter
/// - Labels: `stack_small_id`
/// - Unit: requests (count)
pub static STACK_NUM_REQUESTS_COUNTER: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_stack_num_requests_counter")
        .with_description("Total number of stack requests")
        .with_unit("requests")
        .build()
});

/// Counter metric that tracks the total number of successful chat completion requests per user.
///
/// # Metric Details
/// - Name: `atoma_chat_completion_requests_per_user`
/// - Type: Counter
/// - Labels: `user_id`
/// - Unit: requests (count)
pub static CHAT_COMPLETION_REQUESTS_PER_USER: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_chat_completion_requests_per_user")
        .with_description("Total number of chat completion requests per user")
        .with_unit("requests")
        .build()
});

/// Counter metric that tracks the total number of unsuccessful chat completion requests per user.
///
/// # Metric Details
/// - Name: `atoma_unsuccessful_chat_completion_requests_per_user`
/// - Type: Counter
/// - Labels: `user_id`
/// - Unit: requests (count)
pub static UNSUCCESSFUL_CHAT_COMPLETION_REQUESTS_PER_USER: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_unsuccessful_chat_completion_requests_per_user")
        .with_description("Total number of unsuccessful chat completion requests per user")
        .with_unit("requests")
        .build()
});

/// Counter metric that tracks the total number of successful image generation requests per user.
///
/// # Metric Details
/// - Name: `atoma_successful_image_generation_requests_per_user`
/// - Type: Counter
/// - Labels: `user_id`
/// - Unit: requests (count)
pub static SUCCESSFUL_IMAGE_GENERATION_REQUESTS_PER_USER: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_successful_image_generation_requests_per_user")
        .with_description("Total number of successful image generation requests per user")
        .with_unit("requests")
        .build()
});

/// Counter metric that tracks the total number of unsuccessful image generation requests per user.
///
/// # Metric Details
/// - Name: `atoma_unsuccessful_image_generation_requests_per_user`
/// - Type: Counter
/// - Labels: `user_id`
/// - Unit: requests (count)
pub static UNSUCCESSFUL_IMAGE_GENERATION_REQUESTS_PER_USER: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_unsuccessful_image_generation_requests_per_user")
        .with_description("Total number of unsuccessful image generation requests per user")
        .with_unit("requests")
        .build()
});

/// Counter metric that tracks the total number of successful text embedding requests per user.
///
/// # Metric Details
/// - Name: `atoma_successful_text_embedding_requests_per_user`
/// - Type: Counter
/// - Labels: `user_id`
/// - Unit: requests (count)
pub static SUCCESSFUL_TEXT_EMBEDDING_REQUESTS_PER_USER: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_successful_text_embedding_requests_per_user")
        .with_description("Total number of successful text embedding requests per user")
        .with_unit("requests")
        .build()
});

/// Counter metric that tracks the total number of unsuccessful text embedding requests per user.
///
/// # Metric Details
/// - Name: `atoma_unsuccessful_text_embedding_requests_per_user`
/// - Type: Counter
/// - Labels: `user_id`
/// - Unit: requests (count)
pub static UNSUCCESSFUL_TEXT_EMBEDDING_REQUESTS_PER_USER: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_unsuccessful_text_embedding_requests_per_user")
        .with_description("Total number of unsuccessful text embedding requests per user")
        .with_unit("requests")
        .build()
});

/// Counter metric that tracks the total number of chat completion tokens per user.
///
/// # Metric Details
/// - Name: `atoma_chat_completions_total_tokens_per_user`
/// - Type: Counter
/// - Labels: `user_id`
/// - Unit: tokens (count)
pub static CHAT_COMPLETIONS_TOTAL_TOKENS_PER_USER: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_chat_completions_total_tokens_per_user")
        .with_description("Total number of chat completion tokens per user")
        .with_unit("tokens")
        .build()
});

/// Counter metric that tracks the total number of chat completion input tokens per user.
///
/// # Metric Details
/// - Name: `atoma_chat_completions_input_tokens_per_user`
/// - Type: Counter
/// - Labels: `user_id`
/// - Unit: tokens (count)
pub static CHAT_COMPLETIONS_INPUT_TOKENS_PER_USER: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_chat_completions_input_tokens_per_user")
        .with_description("Total number of chat completion input tokens per user")
        .with_unit("tokens")
        .build()
});

/// Counter metric that tracks the total number of chat completion completions tokens per user.
///
/// # Metric Details
/// - Name: `atoma_chat_completions_completions_tokens_per_user`
/// - Type: Counter
/// - Labels: `user_id`
/// - Unit: tokens (count)
pub static CHAT_COMPLETIONS_COMPLETIONS_TOKENS_PER_USER: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_chat_completions_completions_tokens_per_user")
        .with_description("Total number of chat completion completions tokens per user")
        .with_unit("tokens")
        .build()
});

/// Counter metric that tracks the total number of cancelled stream chat completion requests per user.
///
/// # Metric Details
/// - Name: `atoma_cancelled_stream_chat_completion_requests_per_user`
/// - Type: Counter
/// - Labels: `user_id`
/// - Unit: requests (count)
pub static CANCELLED_STREAM_CHAT_COMPLETION_REQUESTS_PER_USER: Lazy<Counter<u64>> =
    Lazy::new(|| {
        GLOBAL_METER
            .u64_counter("atoma_cancelled_stream_requests_per_user")
            .with_description("Total number of cancelled stream requests per user")
            .with_unit("requests")
            .build()
    });

/// Counter metric that tracks the total number of image generation tokens per user.
///
/// # Metric Details
/// - Name: `atoma_image_generation_total_tokens_per_user`
/// - Type: Counter
/// - Labels: `user_id`
/// - Unit: tokens (count)
pub static IMAGE_GENERATION_TOTAL_TOKENS_PER_USER: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_image_generation_total_tokens_per_user")
        .with_description("Total number of image generation tokens per user")
        .with_unit("tokens")
        .build()
});

/// Counter metric that tracks the total number of embedding tokens per user.
///
/// # Metric Details
/// - Name: `atoma_embedding_total_tokens_per_user`
/// - Type: Counter
/// - Labels: `user_id`
/// - Unit: tokens (count)
pub static EMBEDDING_TOTAL_TOKENS_PER_USER: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_embedding_total_tokens_per_user")
        .with_description("Total number of embedding tokens per user")
        .with_unit("tokens")
        .build()
});

/// Counter metric that tracks the total number of unavailable stack status codes per stack small id and user_id.
///
/// # Metric Details
/// - Name: `atoma_unavailable_stack_counter`
/// - Type: Counter
/// - Labels: `stack_small_id`, `user_id`
/// - Unit: requests (count)
pub static UNAVAILABLE_STACK_COUNTER_PER_USER: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_unavailable_stack_counter")
        .with_description(
            "Total number of unavailable stack status codes per stack small id and user_id",
        )
        .with_unit("requests")
        .build()
});

/// Counter metric that tracks the total number of locked stack status codes per stack small id and user_id.
///
/// # Metric Details
/// - Name: `atoma_locked_stack_counter`
/// - Type: Counter
/// - Labels: `stack_small_id`, `user_id`
/// - Unit: requests (count)
pub static LOCKED_STACK_COUNTER_PER_USER: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("atoma_locked_stack_counter_per_user")
        .with_description(
            "Total number of locked stack status codes per stack small id and user_id",
        )
        .with_unit("requests")
        .build()
});
