use anyhow::{Context, Result};
use opentelemetry::{global, trace::TracerProvider, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    metrics::{self as sdkmetrics},
    trace::{self as sdktrace, RandomIdGenerator, Sampler},
    Resource,
};
use std::sync::LazyLock;
use url::Url;

use std::{path::Path, process};
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

// Default Grafana OTLP endpoint if not specified in environment

const DEFAULT_OTLP_ENDPOINT: &str = "http://otel-collector:4317";
const DEFAULT_LOKI_ENDPOINT: &str = "http://loki:3100";

static RESOURCE: LazyLock<Resource> =
    LazyLock::new(|| Resource::new(vec![KeyValue::new("service_name", "atoma-proxy")]));

/// Initialize metrics with OpenTelemetry SDK
fn init_metrics(otlp_endpoint: &str) -> sdkmetrics::SdkMeterProvider {
    let metrics_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .with_endpoint(otlp_endpoint)
        .build()
        .unwrap();

    let reader =
        sdkmetrics::PeriodicReader::builder(metrics_exporter, opentelemetry_sdk::runtime::Tokio)
            .with_interval(std::time::Duration::from_secs(3))
            .with_timeout(std::time::Duration::from_secs(10))
            .build();

    sdkmetrics::SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(RESOURCE.clone())
        .build()
}

/// Initialize tracing with OpenTelemetry SDK
fn init_traces(otlp_endpoint: &str) -> Result<sdktrace::Tracer> {
    let tracing_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(otlp_endpoint)
        .build()?;

    let tracer_provider = opentelemetry_sdk::trace::TracerProvider::builder()
        .with_batch_exporter(tracing_exporter, opentelemetry_sdk::runtime::Tokio)
        .with_sampler(Sampler::AlwaysOn)
        .with_id_generator(RandomIdGenerator::default())
        .with_max_events_per_span(64)
        .with_max_attributes_per_span(16)
        .with_max_events_per_span(16)
        .with_resource(RESOURCE.clone())
        .build();

    let tracer = tracer_provider.tracer("atoma-proxy");
    global::set_tracer_provider(tracer_provider);

    Ok(tracer)
}

/// Configure logging with JSON formatting, file output, and console output
pub fn setup_logging<P: AsRef<Path>>(log_dir: P) -> Result<(WorkerGuard, WorkerGuard)> {
    let otlp_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap_or_else(|_| DEFAULT_OTLP_ENDPOINT.to_string());
    let loki_endpoint =
        std::env::var("LOKI_ENDPOINT").unwrap_or_else(|_| DEFAULT_LOKI_ENDPOINT.to_string());
    // Create logs directory if it doesn't exist
    std::fs::create_dir_all(&log_dir).context("Failed to create logs directory")?;

    // Set up metrics
    let metrics_provider = init_metrics(&otlp_endpoint);
    global::set_meter_provider(metrics_provider);

    let (layer, task) = tracing_loki::builder()
        .label("service_name", "atoma-proxy")?
        .extra_field("pid", format!("{}", process::id()))?
        .build_url(Url::parse(&loki_endpoint).unwrap())?;

    // The background task needs to be spawned so the logs actually get
    // delivered.
    tokio::spawn(task);

    // Set up file appender with rotation
    let file_appender = RollingFileAppender::new(Rotation::DAILY, log_dir, LOG_FILE);

    // Create non-blocking writers
    let (non_blocking_appender, file_guard) = non_blocking(file_appender);
    let (non_blocking_stdout, stdout_guard) = non_blocking(std::io::stdout());

    // Initialize OpenTelemetry tracing
    let tracer = init_traces(&otlp_endpoint)?;
    let opentelemetry_layer = OpenTelemetryLayer::new(tracer);

    let logs_exporter = opentelemetry_otlp::LogExporter::builder()
        .with_tonic()
        .with_endpoint(otlp_endpoint)
        .build()?;

    let _logger = opentelemetry_sdk::logs::LoggerProvider::builder()
        .with_batch_exporter(logs_exporter, opentelemetry_sdk::runtime::Tokio)
        .with_resource(RESOURCE.clone())
        .build();

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
        .unwrap_or_else(|_| EnvFilter::new("info,atoma_proxy=info,atoma_p2p=info"));

    // Combine layers with filter
    Registry::default()
        .with(env_filter)
        .with(console_layer)
        .with(file_layer)
        .with(opentelemetry_layer)
        .with(layer)
        .with(sentry::integrations::tracing::layer())
        .try_init()
        .context("Failed to set global default subscriber")?;

    // Return both guards so they can be stored in main
    Ok((file_guard, stdout_guard))
}

/// Ensure all spans are exported before shutdown
pub fn shutdown() {
    global::shutdown_tracer_provider();
}
