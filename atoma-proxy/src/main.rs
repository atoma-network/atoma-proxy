use std::path::Path;

use anyhow::{Context, Result};
use atoma_state::{AtomaStateManager, AtomaStateManagerConfig};
use atoma_sui::{client::AtomaSuiClient, AtomaSuiConfig};
use clap::Parser;
use server::{start_server, AtomaServiceConfig};
use sui::Sui;
use sui_keys::keystore::FileBasedKeystore;
use sui_sdk::wallet_context::WalletContext;
use tokio::{sync::watch, try_join};
use tracing_appender::{
    non_blocking,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{
    fmt::{self, format::FmtSpan, time::UtcTime},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter, Registry,
};

mod server;
mod sui;

/// The directory where the logs are stored.
const LOGS: &str = "./logs";
/// The log file name.
const LOG_FILE: &str = "atoma-proxy-service.log";

/// Command line arguments for the Atoma node
#[derive(Parser)]
struct Args {
    /// Path to the configuration file
    #[arg(short, long)]
    config_path: String,
}

/// Configuration for the Atoma proxy.
///
/// This struct holds the configuration settings for various components
/// of the Atoma proxy, including the Sui, service, and state manager configurations.
#[derive(Debug)]
struct Config {
    /// Configuration for the Sui component.
    sui: AtomaSuiConfig,

    /// Configuration for the service component.
    service: AtomaServiceConfig,

    /// Configuration for the state manager component.
    state: AtomaStateManagerConfig,
}

impl Config {
    async fn load(path: String) -> Self {
        Self {
            sui: AtomaSuiConfig::from_file_path(path.clone()),
            service: AtomaServiceConfig::from_file_path(path.clone()),
            state: AtomaStateManagerConfig::from_file_path(path),
        }
    }
}

/// Configure logging with JSON formatting, file output, and console output
fn setup_logging<P: AsRef<Path>>(log_dir: P) -> Result<()> {
    // Set up file appender with rotation
    let file_appender = RollingFileAppender::new(Rotation::DAILY, log_dir, LOG_FILE);

    // Create a non-blocking writer
    let (non_blocking_appender, _guard) = non_blocking(file_appender);

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
        .with_span_events(FmtSpan::ENTER);

    // Create filter from environment variable or default to info
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,atoma_node_service=debug"));

    // Combine layers with filter
    Registry::default()
        .with(env_filter)
        .with(console_layer)
        .with(file_layer)
        .init();

    Ok(())
}

#[tokio::main]
async fn main() {
    setup_logging(LOGS)
        .context("Failed to setup logging")
        .unwrap();
    let args = Args::parse();
    let config = Config::load(args.config_path).await;

    let (_shutdown_sender, shutdown_receiver) = watch::channel(false);
    let (event_subscriber_sender, event_subscriber_receiver) = flume::unbounded();
    let (state_manager_sender, state_manager_receiver) = flume::unbounded();

    let sui_subscriber = atoma_sui::SuiEventSubscriber::new(
        config.sui.clone(),
        event_subscriber_sender,
        shutdown_receiver.clone(),
    );

    // Initialize your StateManager here
    let state_manager = AtomaStateManager::new_from_url(
        config.state.database_url,
        event_subscriber_receiver,
        state_manager_receiver,
    )
    .await
    .unwrap();

    let shutdown_receiver_clone = shutdown_receiver.clone();
    let state_manager_handle = tokio::spawn(async move {
        state_manager.run(shutdown_receiver_clone).await.unwrap();
    });

    let sui_subscriber_handle = tokio::spawn(async move {
        sui_subscriber.run().await.unwrap();
    });
    let server_handle = tokio::spawn(async move {
        let atoma_sui_client = AtomaSuiClient::new(config.sui.clone()).await.unwrap();
        let sui = Sui::new(&config.sui).await.unwrap();
        start_server(config.service, atoma_sui_client, state_manager_sender, sui).await;
    });

    try_join!(sui_subscriber_handle, server_handle, state_manager_handle).unwrap();
}
