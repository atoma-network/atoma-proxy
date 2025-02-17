use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use atoma_auth::{AtomaAuthConfig, Auth, Sui};
use atoma_p2p::{AtomaP2pNode, AtomaP2pNodeConfig};
use atoma_proxy_service::{run_proxy_service, AtomaProxyServiceConfig, Grafana, ProxyServiceState};
use atoma_state::{AtomaState, AtomaStateManager, AtomaStateManagerConfig};
use atoma_sui::{config::Config as AtomaSuiConfig, subscriber::Subscriber};
use atoma_utils::spawn_with_shutdown;
use clap::Parser;
use futures::future::try_join_all;
use hf_hub::{api::sync::ApiBuilder, Repo, RepoType};
use sui_keys::keystore::FileBasedKeystore;
use tokenizers::Tokenizer;
use tokio::{net::TcpListener, sync::watch, sync::RwLock, try_join};
use tracing::{error, info, instrument};

use crate::server::{start_server, AtomaServiceConfig};

mod server;
mod telemetry;

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

    /// Configuration for the proxy service component.
    proxy_service: AtomaProxyServiceConfig,

    /// Configuration for the authentication component.
    auth: AtomaAuthConfig,

    /// Configuration for the P2P component.
    p2p: AtomaP2pNodeConfig,
}

impl Config {
    fn load(path: String) -> Self {
        Self {
            sui: AtomaSuiConfig::from_file_path(path.clone()),
            service: AtomaServiceConfig::from_file_path(path.clone()),
            state: AtomaStateManagerConfig::from_file_path(path.clone()),
            proxy_service: AtomaProxyServiceConfig::from_file_path(path.clone()),
            auth: AtomaAuthConfig::from_file_path(path.clone()),
            p2p: AtomaP2pNodeConfig::from_file_path(path),
        }
    }
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<()> {
    // Store both guards to keep logging active for the duration of the program
    let (_file_guard, _stdout_guard) =
        telemetry::setup_logging("./logs").context("Failed to setup logging")?;

    info!(event = "startup", "Starting Atoma Proxy Service...");

    let args = Args::parse();
    tracing::info!("Loading configuration from: {}", args.config_path);

    let config = Config::load(args.config_path);
    tracing::info!("Configuration loaded successfully");

    let (shutdown_sender, mut shutdown_receiver) = watch::channel(false);
    let (event_subscriber_sender, event_subscriber_receiver) = flume::unbounded();
    let (state_manager_sender, state_manager_receiver) = flume::unbounded();
    let (atoma_p2p_sender, atoma_p2p_receiver) = flume::unbounded();
    let (confidential_compute_service_sender, _confidential_compute_service_receiver) =
        tokio::sync::mpsc::unbounded_channel();

    let sui = Arc::new(RwLock::new(Sui::new(&config.sui)?));

    let auth = Auth::new(config.auth, state_manager_sender.clone(), Arc::clone(&sui)).await?;

    let (_stack_retrieve_sender, stack_retrieve_receiver) = tokio::sync::mpsc::unbounded_channel();
    let sui_subscriber = Subscriber::new(
        config.sui.clone(),
        event_subscriber_sender,
        stack_retrieve_receiver,
        confidential_compute_service_sender,
        shutdown_receiver.clone(),
    );

    let keystore = FileBasedKeystore::new(&PathBuf::from(&config.sui.sui_keystore_path()))?;
    let atoma_p2p_node =
        AtomaP2pNode::start(config.p2p, Arc::new(keystore), atoma_p2p_sender, true)?;

    let sui = Arc::new(RwLock::new(Sui::new(&config.sui)?));

    // Initialize the `AtomaStateManager` service
    let state_manager = AtomaStateManager::new_from_url(
        &config.state.database_url,
        event_subscriber_receiver,
        state_manager_receiver,
        atoma_p2p_receiver,
        sui.write().await.get_wallet_address()?.to_string(),
    )
    .await?;

    let state_manager_handle = spawn_with_shutdown(
        state_manager.run(shutdown_receiver.clone()),
        shutdown_sender.clone(),
    );

    let sui_subscriber_handle = spawn_with_shutdown(sui_subscriber.run(), shutdown_sender.clone());
    let tokenizers = initialize_tokenizers(
        &config.service.models,
        &config.service.revisions,
        &config.service.hf_token,
    )
    .await?;

    let models_with_modalities = config
        .service
        .models
        .iter()
        .zip(config.service.modalities.iter())
        .map(|(model, modalities)| {
            let modalities = modalities.clone();
            (model.clone(), modalities)
        })
        .collect();

    let server_handle = spawn_with_shutdown(
        start_server(
            config.service,
            state_manager_sender,
            sui,
            tokenizers,
            shutdown_receiver.clone(),
        ),
        shutdown_sender.clone(),
    );

    let proxy_service_tcp_listener = TcpListener::bind(&config.proxy_service.service_bind_address)
        .await
        .context("Failed to bind proxy service TCP listener")?;

    let grafana = Grafana::new(
        config.proxy_service.grafana_url,
        config.proxy_service.grafana_api_token,
        config.proxy_service.grafana_dashboard_tag,
    );

    let proxy_service_state = ProxyServiceState {
        atoma_state: AtomaState::new_from_url(&config.state.database_url).await?,
        auth,
        models_with_modalities,
        grafana,
    };

    let proxy_service_handle = spawn_with_shutdown(
        run_proxy_service(
            proxy_service_state,
            proxy_service_tcp_listener,
            shutdown_receiver.clone(),
        ),
        shutdown_sender.clone(),
    );

    let atoma_p2p_node_handle = spawn_with_shutdown(
        atoma_p2p_node.run(shutdown_receiver.clone()),
        shutdown_sender.clone(),
    );

    #[allow(clippy::redundant_pub_crate)]
    let ctrl_c = tokio::task::spawn(async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("ctrl-c received, sending shutdown signal");
                shutdown_sender.send(true).unwrap();
            }
            _ = shutdown_receiver.changed() => {
            }
        }
    });

    let (
        sui_subscriber_result,
        server_result,
        state_manager_result,
        proxy_service_result,
        atoma_p2p_node_result,
        (),
    ) = try_join!(
        sui_subscriber_handle,
        server_handle,
        state_manager_handle,
        proxy_service_handle,
        atoma_p2p_node_handle,
        ctrl_c
    )?;

    handle_tasks_results(
        sui_subscriber_result,
        state_manager_result,
        server_result,
        proxy_service_result,
        atoma_p2p_node_result,
    )?;

    // Before the program exits, ensure all spans are exported
    telemetry::shutdown();
    Ok(())
}

/// Initializes tokenizers for multiple models by fetching their configurations from HuggingFace.
///
/// This function concurrently fetches tokenizer configurations for multiple models from HuggingFace's
/// repository and initializes them. Each tokenizer is wrapped in an Arc for safe sharing across threads.
///
/// # Arguments
///
/// * `models` - A slice of model names/paths on HuggingFace (e.g., ["facebook/opt-125m"])
/// * `revisions` - A slice of revision/branch names corresponding to each model (e.g., ["main"])
/// * `hf_token` - The HuggingFace API token to use for fetching the tokenizer configurations
///
/// # Returns
///
/// Returns a `Result<()>`, which is `Ok(())` if all tasks succeeded, or an error if:
/// - Failed to fetch tokenizer configuration from HuggingFace
/// - Failed to parse the tokenizer JSON
/// - Any other network or parsing errors occur
///
/// # Examples
///
/// ```rust,ignore
/// use anyhow::Result;
///
/// #[tokio::main]
/// async fn example() -> Result<()> {
///     let models = vec!["facebook/opt-125m".to_string()];
///     let revisions = vec!["main".to_string()];
///
///     let tokenizers = initialize_tokenizers(&models, &revisions).await?;
///     Ok(())
/// }
/// ```
#[instrument(level = "info", skip(models, revisions))]
async fn initialize_tokenizers(
    models: &[String],
    revisions: &[String],
    hf_token: &String,
) -> Result<Vec<Arc<Tokenizer>>> {
    let api = ApiBuilder::new()
        .with_progress(true)
        .with_token(Some(hf_token.clone()))
        .build()?;
    let fetch_futures: Vec<_> = models
        .iter()
        .zip(revisions.iter())
        .map(|(model, revision)| {
            let api = api.clone();
            async move {
                let repo = api.repo(Repo::with_revision(
                    model.clone(),
                    RepoType::Model,
                    revision.clone(),
                ));

                let tokenizer_filename = repo
                    .get("tokenizer.json")
                    .expect("Failed to get tokenizer.json");

                Tokenizer::from_file(tokenizer_filename)
                    .map_err(|e| {
                        anyhow::anyhow!(format!(
                            "Failed to parse tokenizer for model {}, with error: {}",
                            model, e
                        ))
                    })
                    .map(Arc::new)
            }
        })
        .collect();

    try_join_all(fetch_futures).await
}

/// Handles the results of various tasks (subscriber, state manager, and server).
///
/// This function checks the results of the subscriber, state manager, and server tasks.
/// If any of the tasks return an error, it logs the error and returns it.
/// This is useful for ensuring that the application can gracefully handle failures
/// in any of its components and provide appropriate logging for debugging.
///
/// # Arguments
///
/// * `subscriber_result` - The result of the subscriber task, which may contain an error.
/// * `state_manager_result` - The result of the state manager task, which may contain an error.
/// * `server_result` - The result of the server task, which may contain an error.
///
/// # Returns
///
/// Returns a `Result<()>`, which is `Ok(())` if all tasks succeeded, or an error if any task failed.
#[instrument(level = "info", skip_all)]
fn handle_tasks_results(
    sui_subscriber_result: Result<()>,
    state_manager_result: Result<()>,
    server_result: Result<()>,
    proxy_service_result: Result<()>,
    atoma_p2p_node_result: Result<()>,
) -> Result<()> {
    let result_handler = |result: Result<()>, message: &str| {
        if let Err(e) = result {
            error!(
                target = "atoma-node-service",
                event = "atoma_node_service_shutdown",
                error = ?e,
                "{message}"
            );
            return Err(e);
        }
        Ok(())
    };
    result_handler(sui_subscriber_result, "Subscriber terminated abruptly")?;
    result_handler(state_manager_result, "State manager terminated abruptly")?;
    result_handler(server_result, "Server terminated abruptly")?;
    result_handler(proxy_service_result, "Proxy service terminated abruptly")?;
    result_handler(atoma_p2p_node_result, "Atoma P2P node terminated abruptly")?;
    Ok(())
}
