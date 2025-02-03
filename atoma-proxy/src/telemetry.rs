use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use opentelemetry::{
    global,
    metrics::{Counter, Histogram, Meter, Unit},
    trace::TracerProvider,
    KeyValue,
};
use opentelemetry_otlp::{new_exporter, WithExportConfig};
use opentelemetry_sdk::{
    metrics::{self as sdkmetrics},
    trace as sdktrace, Resource,
};
use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::{
    non_blocking,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{
    fmt::{self, format::FmtSpan, time::UtcTime},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter, Registry,
};

const LOG_FILE: &str = "atoma-proxy-service.log";
const BASELIME_URL: &str = "https://otel-ingest.baselime.io:8443";

static RESOURCE: Lazy<Resource> =
    Lazy::new(|| Resource::new(vec![KeyValue::new("service.name", "atoma-proxy")]));

// Add global metrics
static GLOBAL_METER: Lazy<Meter> = Lazy::new(|| global::meter("atoma-proxy"));

// Define metric instruments
static REQUEST_DURATION: Lazy<Histogram<f64>> = Lazy::new(|| {
    GLOBAL_METER
        .f64_histogram("request_duration")
        .with_description("Total request latency in seconds")
        .with_unit(Unit::new("s"))
        .init()
});

static MIDDLEWARE_DURATION: Lazy<Histogram<f64>> = Lazy::new(|| {
    GLOBAL_METER
        .f64_histogram("middleware_duration")
        .with_description("Middleware execution time in seconds")
        .with_unit(Unit::new("s"))
        .init()
});

static DB_READ_DURATION: Lazy<Histogram<f64>> = Lazy::new(|| {
    GLOBAL_METER
        .f64_histogram("db_read_duration")
        .with_description("Database read operation duration in seconds")
        .with_unit(Unit::new("s"))
        .init()
});

static DB_WRITE_DURATION: Lazy<Histogram<f64>> = Lazy::new(|| {
    GLOBAL_METER
        .f64_histogram("db_write_duration")
        .with_description("Database write operation duration in seconds")
        .with_unit(Unit::new("s"))
        .init()
});

static DB_READ_COUNT: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("db_read_count")
        .with_description("Number of database read operations")
        .init()
});

static DB_WRITE_COUNT: Lazy<Counter<u64>> = Lazy::new(|| {
    GLOBAL_METER
        .u64_counter("db_write_count")
        .with_description("Number of database write operations")
        .init()
});

/// Initialize metrics with OpenTelemetry SDK
fn init_metrics() -> sdkmetrics::MeterProvider {
    sdkmetrics::MeterProvider::builder()
        .with_resource(RESOURCE.clone())
        .build()
}

/// Initialize tracing with OpenTelemetry SDK
fn init_traces() -> Result<sdktrace::Tracer> {
    let mut map = tonic::metadata::MetadataMap::new();
    map.insert(
        "x-api-key",
        std::env::var("BASELIME_API_KEY")
            .context("BASELIME_API_KEY not set")?
            .parse()?,
    );

    let exporter = new_exporter()
        .tonic()
        .with_endpoint(BASELIME_URL)
        .with_metadata(map)
        .build_span_exporter()?;

    let config = sdktrace::config().with_resource(RESOURCE.clone());
    let provider = sdktrace::TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .with_config(config)
        .build();

    let tracer = provider.tracer("atoma-proxy");
    global::set_tracer_provider(provider);

    Ok(tracer)
}

/// Configure logging with JSON formatting, file output, and console output
pub fn setup_logging<P: AsRef<Path>>(log_dir: P) -> Result<(WorkerGuard, WorkerGuard)> {
    // Create logs directory if it doesn't exist
    std::fs::create_dir_all(&log_dir).context("Failed to create logs directory")?;

    // Set up metrics
    let metrics_provider = init_metrics();
    global::set_meter_provider(metrics_provider);

    // Set up file appender with rotation
    let file_appender = RollingFileAppender::new(Rotation::DAILY, log_dir, LOG_FILE);

    // Create non-blocking writers
    let (non_blocking_appender, file_guard) = non_blocking(file_appender);
    let (non_blocking_stdout, stdout_guard) = non_blocking(std::io::stdout());

    // Initialize OpenTelemetry tracing
    let tracer = init_traces()?;
    let opentelemetry_layer = OpenTelemetryLayer::new(tracer);

    // Create JSON formatter for file output
    let file_layer = fmt::layer()
        .json()
        .with_timer(UtcTime::rfc_3339())
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_target(true)
        .with_line_number(true)
        .with_file(true)
        .with_current_span(true)
        .with_span_list(true)
        .with_writer(non_blocking_appender);

    // Create console formatter for development
    let console_layer = fmt::layer()
        .pretty()
        .with_target(true)
        .with_thread_ids(true)
        .with_line_number(true)
        .with_file(true)
        .with_span_events(FmtSpan::ENTER)
        .with_writer(non_blocking_stdout);

    // Create filter from environment variable or default to info
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,atoma_proxy=info"));

    // Combine layers with filter
    Registry::default()
        .with(env_filter)
        .with(console_layer)
        .with(file_layer)
        .with(opentelemetry_layer)
        .init();

    // Return both guards so they can be stored in main
    Ok((file_guard, stdout_guard))
}

/// Record a database operation metric
pub fn record_db_operation(is_write: bool, duration: f64) {
    if is_write {
        DB_WRITE_DURATION.record(duration, &[]);
        DB_WRITE_COUNT.add(1, &[]);
    } else {
        DB_READ_DURATION.record(duration, &[]);
        DB_READ_COUNT.add(1, &[]);
    }
}

pub fn record_request_duration(duration: f64) {
    REQUEST_DURATION.record(duration, &[]);
}

pub fn record_middleware_duration(duration: f64) {
    MIDDLEWARE_DURATION.record(duration, &[]);
}

/// Ensure all spans are exported before shutdown
pub fn shutdown() {
    global::shutdown_tracer_provider();
}
