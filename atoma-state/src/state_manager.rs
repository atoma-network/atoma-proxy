use std::time::Duration;

use crate::handlers::{handle_atoma_event, handle_p2p_event, handle_state_manager_event};
use crate::network::NetworkMetrics;
use crate::types::{
    AtomaAtomaStateManagerEvent, CheapestNode, ComputedUnitsProcessedResponse, LatencyResponse,
    NodeAttestation, NodeDistribution, NodePublicKey, NodeSubscription, Pricing, Stack,
    StackAttestationDispute, StackSettlementTicket, StatsStackResponse, Task, TokenResponse,
    UserProfile,
};
use crate::{build_query_with_in, AtomaStateManagerError};

use atoma_p2p::broadcast_metrics::NodeMetrics;
use atoma_p2p::AtomaP2pEvent;
use atoma_sui::events::AtomaEvent;
use chrono::{DateTime, Timelike, Utc};
use flume::{Receiver as FlumeReceiver, Sender as FlumeSender};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use sqlx::{FromRow, Row};
use tokio::sync::oneshot;
use tokio::sync::watch::Receiver;
use tracing::instrument;

/// The maximum number of connections to the Postgres database.
const MAX_NUMBER_POOL_CONNECTIONS: u32 = 256;

pub type Result<T> = std::result::Result<T, AtomaStateManagerError>;

type AtomaP2pData = (AtomaP2pEvent, Option<oneshot::Sender<bool>>);

/// AtomaStateManager is a wrapper around a Postgres connection pool, responsible for managing the state of the Atoma system.
///
/// It provides an interface to interact with the Postgres database, handling operations
/// related to tasks, node subscriptions, stacks, and various other system components.
pub struct AtomaStateManager {
    /// The Postgres connection pool used for database operations.
    pub state: AtomaState,

    /// Receiver channel from the SuiEventSubscriber
    pub event_subscriber_receiver: FlumeReceiver<AtomaEvent>,

    /// Metrics collector sender
    pub metrics_collector_sender: FlumeSender<(i64, NodeMetrics)>,

    /// Atoma p2p event receiver
    pub p2p_event_receiver: FlumeReceiver<AtomaP2pData>,

    /// Atoma service receiver
    pub state_manager_receiver: FlumeReceiver<AtomaAtomaStateManagerEvent>,

    /// Sui address
    pub sui_address: String,
}

impl AtomaStateManager {
    /// Constructor
    #[must_use]
    pub const fn new(
        db: PgPool,
        event_subscriber_receiver: FlumeReceiver<AtomaEvent>,
        metrics_collector_sender: FlumeSender<(i64, NodeMetrics)>,
        p2p_event_receiver: FlumeReceiver<AtomaP2pData>,
        state_manager_receiver: FlumeReceiver<AtomaAtomaStateManagerEvent>,
        sui_address: String,
    ) -> Self {
        Self {
            state: AtomaState::new(db),
            event_subscriber_receiver,
            state_manager_receiver,
            p2p_event_receiver,
            metrics_collector_sender,
            sui_address,
        }
    }

    /// Creates a new `AtomaStateManager` instance from a database URL.
    ///
    /// This method establishes a connection to the Postgres database using the provided URL,
    /// creates all necessary tables in the database, and returns a new `AtomaStateManager` instance.
    ///
    /// # Arguments
    /// * `database_url` - PostgreSQL connection URL
    /// * `event_subscriber_receiver` - Channel for receiving Atoma events
    /// * `state_manager_receiver` - Channel for receiving state manager events
    /// * `sui_address` - Sui blockchain address
    ///
    /// # Errors
    /// Returns an error if:
    /// - Failed to connect to the database
    /// - Failed to create the database pool
    /// - Failed to run database migrations
    pub async fn new_from_url(
        database_url: &str,
        event_subscriber_receiver: FlumeReceiver<AtomaEvent>,
        metrics_collector_sender: FlumeSender<(i64, NodeMetrics)>,
        state_manager_receiver: FlumeReceiver<AtomaAtomaStateManagerEvent>,
        p2p_event_receiver: FlumeReceiver<AtomaP2pData>,
        sui_address: String,
    ) -> Result<Self> {
        let db = PgPoolOptions::new()
            .max_connections(MAX_NUMBER_POOL_CONNECTIONS)
            .connect(database_url)
            .await?;
        // run migrations
        sqlx::migrate!("./src/migrations").run(&db).await?;
        Ok(Self {
            state: AtomaState::new(db),
            event_subscriber_receiver,
            metrics_collector_sender,
            state_manager_receiver,
            p2p_event_receiver,
            sui_address,
        })
    }

    /// Runs the state manager, listening for events from the event subscriber and state manager receivers.
    ///
    /// This method continuously processes incoming events from the event subscriber and state manager receivers
    /// until a shutdown signal is received. It uses asynchronous select to handle multiple event sources concurrently.
    ///
    /// # Arguments
    ///
    /// * `shutdown_signal` - A `Receiver<bool>` that signals when the state manager should shut down.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - An error occurs while handling events from the event subscriber or state manager receivers.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn start_state_manager(state_manager: AtomaStateManager) -> Result<(), AtomaStateManagerError> {
    ///     let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    ///     state_manager.run(shutdown_rx).await
    /// }
    /// ```
    #[allow(clippy::too_many_lines)]
    #[instrument(level = "trace", skip_all)]
    pub async fn run(
        self,
        state_manager_sender: flume::Sender<AtomaAtomaStateManagerEvent>,
        mut shutdown_signal: Receiver<bool>,
    ) -> Result<()> {
        // Spawn a task that sends a message every interval
        let mut shutdown_signal_clone = shutdown_signal.clone();
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(15);
            let mut network_metrics = NetworkMetrics::new();
            loop {
                tokio::select! {
                    () = tokio::time::sleep(interval) => {
                        let (result_sender, result_receiver) = tokio::sync::oneshot::channel();
                        match state_manager_sender.send(
                            AtomaAtomaStateManagerEvent::RetrieveNodesPublicAddresses { result_sender },
                        ) {
                            Ok(()) => match result_receiver.await {
                                Ok(node_addresses) => match node_addresses {
                                    Ok(node_addresses) => {
                                        network_metrics.update_metrics(node_addresses).await;
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            target = "network_metrics",
                                            error = %e,
                                            "Failed to retrieve node addresses"
                                        );
                                    }
                                },
                                Err(e) => {
                                    tracing::error!(
                                        target = "network_metrics",
                                        error = %e,
                                        "Failed to receive node addresses"
                                    );
                                }
                            },
                            Err(e) => {
                                tracing::error!(
                                    target = "network_metrics",
                                    error = %e,
                                    "Failed to send retrieve nodes public addresses event"
                                );
                            }
                        }
                    }
                    shutdown_signal_changed = shutdown_signal_clone.changed() => {
                        match shutdown_signal_changed {
                            Ok(()) => {
                                if *shutdown_signal_clone.borrow() {
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
                                // NOTE: We want to break here as well, since no one can signal shutdown anymore
                                break;
                            }
                        }
                    }
                }
            }
        });

        loop {
            tokio::select! {
                atoma_event = self.event_subscriber_receiver.recv_async() => {
                    match atoma_event {
                        Ok(atoma_event) => {
                            tracing::trace!(
                                target = "atoma-state-manager",
                                event = "event_subscriber_receiver",
                                "Event received from event subscriber receiver"
                            );
                            if let Err(e) = handle_atoma_event(atoma_event, &self).await {
                                tracing::error!(
                                    target = "atoma-state-manager",
                                    event = "event_subscriber_receiver_error",
                                    error = %e,
                                    "Error handling Atoma event"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                target = "atoma-state-manager",
                                event = "event_subscriber_receiver_error",
                                error = %e,
                                "All event subscriber senders have been dropped, terminating the state manager running process"
                            );
                            break;
                        }
                    }
                }
                state_manager_event = self.state_manager_receiver.recv_async() => {
                    match state_manager_event {
                        Ok(state_manager_event) => {
                            if let Err(e) = handle_state_manager_event(&self, state_manager_event).await {
                                tracing::error!(
                                    target = "atoma-state-manager",
                                    event = "state_manager_receiver_error",
                                    error = %e,
                                    "Error handling state manager event"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                target = "atoma-state-manager",
                                event = "state_manager_receiver_error",
                                error = %e,
                                "All state manager senders have been dropped, we will not be able to handle any more events from the Atoma node inference service"
                            );
                            // NOTE: We continue the loop, as the inference service might be shutting down,
                            // but we want to keep the state manager running
                            // for event synchronization with the Atoma Network protocol.
                        }
                    }
                }
                p2p_event = self.p2p_event_receiver.recv_async() => {
                    match p2p_event {
                        Ok((p2p_event, sender)) => {
                            handle_p2p_event(&self, p2p_event, sender).await?;
                        }
                        Err(e) => {
                            tracing::error!(
                                target = "atoma-state-manager",
                                event = "p2p_event_receiver_error",
                                error = %e,
                                "All p2p event senders have been dropped, we will not be able to handle any more events from the Atoma Network protocol"
                            );
                            // NOTE: We continue the loop, as the inference service might be shutting down,
                            // but we want to keep the state manager running
                            // for event synchronization with the Atoma Network protocol.
                        }
                    }
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
                            // NOTE: We want to break here as well, since no one can signal shutdown anymore
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// AtomaState is a wrapper around a Postgres connection pool, responsible for managing the state of the Atoma system.
#[derive(Clone)]
pub struct AtomaState {
    /// The Postgres connection pool used for database operations.
    pub db: PgPool,
}

impl AtomaState {
    /// Constructor
    #[must_use]
    pub const fn new(db: PgPool) -> Self {
        Self { db }
    }

    /// Creates a new state manager instance from a database URL.
    ///
    /// # Arguments
    /// * `database_url` - PostgreSQL connection URL
    ///
    /// # Errors
    /// Returns an error if:
    /// - Failed to connect to the database
    /// - Failed to create the database pool
    pub async fn new_from_url(database_url: &str) -> Result<Self> {
        let db = PgPool::connect(database_url).await?;
        // run migrations
        sqlx::migrate!("./src/migrations").run(&db).await?;
        Ok(Self { db })
    }

    /// Get a task by its unique identifier.
    ///
    /// This method fetches a task from the database based on the provided `task_small_id`.
    ///
    /// # Arguments
    ///
    /// * `task_small_id` - The unique identifier for the task to be fetched.
    ///
    /// # Returns
    ///
    /// - `Result<Task>`: A result containing either:
    ///   - `Ok(Task)`: The task with the specified `task_id`.
    ///   - `Err(AtomaStateManagerError)`: An error if the task is not found or other database operation fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if the database query fails.
    #[instrument(level = "trace", skip_all, fields(%task_small_id))]
    pub async fn get_task_by_small_id(&self, task_small_id: i64) -> Result<Task> {
        let task = sqlx::query("SELECT * FROM tasks WHERE task_small_id = $1")
            .bind(task_small_id)
            .fetch_one(&self.db)
            .await?;
        Ok(Task::from_row(&task)?)
    }

    /// Get a stack by its unique identifier.
    ///
    /// This method fetches a stack from the database based on the provided `model` and `free_units`.
    ///
    /// # Arguments
    ///
    /// * `model` - The model name for the task.
    /// * `free_units` - The number of free units available.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<Stack>>`: A result containing either:
    ///  - `Ok(Vec<Stack>)`: A vector of stacks for the given model with available free units.
    ///  - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if the database query fails.
    #[instrument(level = "trace", skip_all, fields(%model, %free_units))]
    pub async fn get_stacks_for_model(
        &self,
        model: &str,
        free_units: i64,
        user_id: i64,
        is_confidential: bool,
    ) -> Result<Option<Stack>> {
        let mut query = String::from(
            r"
            WITH latest_key_rotation AS (
                SELECT max(key_rotation_counter) as max_krc FROM key_rotations
            ),
            valid_nodes AS (
                SELECT DISTINCT node_small_id
                FROM node_public_keys npk, latest_key_rotation
                WHERE npk.key_rotation_counter >= latest_key_rotation.max_krc
                GROUP BY node_small_id
                HAVING bool_and(npk.is_valid) = true
            ),
            selected_stack AS (
                SELECT stacks.stack_small_id
                FROM stacks
                INNER JOIN tasks ON tasks.task_small_id = stacks.task_small_id
                LEFT JOIN stack_settlement_tickets ON stack_settlement_tickets.stack_small_id = stacks.stack_small_id",
        );

        if is_confidential {
            query.push_str(
                r"
                INNER JOIN valid_nodes ON valid_nodes.node_small_id = stacks.node_small_id",
            );
        }

        query.push_str(
            r"
                WHERE tasks.model_name = $1
                AND stacks.num_compute_units - stacks.already_computed_units - stacks.locked_compute_units >= $2
                AND stacks.user_id = $3
                AND stacks.is_claimed = false
                AND stacks.is_locked = false
                AND stacks.in_settle_period = false
                AND (stack_settlement_tickets.is_claimed = false OR stack_settlement_tickets.is_claimed IS NULL)",
        );

        if is_confidential {
            query.push_str(
                r"
                AND tasks.security_level = 1",
            );
        }

        query.push_str(
            r"
                LIMIT 1
            )
            UPDATE stacks
            SET locked_compute_units = locked_compute_units + $2
            WHERE stack_small_id IN (SELECT stack_small_id FROM selected_stack)
            RETURNING stacks.*",
        );

        let stack = sqlx::query(&query)
            .bind(model)
            .bind(free_units)
            .bind(user_id)
            .fetch_optional(&self.db)
            .await?
            .map(|stack| Stack::from_row(&stack).map_err(AtomaStateManagerError::from))
            .transpose()?;
        Ok(stack)
    }

    /// Selects and updates an available stack for a specific task and user, reserving compute units.
    ///
    /// This method finds a suitable stack associated with the given `task_small_id` and `user_id`
    /// that has enough available compute units (`free_units`). It then atomically updates
    /// the selected stack by increasing its `already_computed_units` by the requested amount.
    ///
    /// # Arguments
    ///
    /// * `task_small_id` - The unique identifier of the task.
    /// * `free_units` - The number of compute units required and to be reserved.
    /// * `user_id` - The ID of the user requesting the stack.
    ///
    /// # Returns
    ///
    /// - `Result<Option<Stack>>`: A result containing either:
    ///  - `Ok(Some(Stack))`: The updated `Stack` object after reserving the compute units.
    ///  - `Ok(None)`: If no suitable stack (matching task, user, sufficient units, not claimed/locked/in settle period) is found.
    ///  - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if the database query fails.

    #[instrument(level = "trace", skip_all, fields(%task_small_id, %free_units, %user_id))]
    pub async fn get_stacks_for_task(
        &self,
        task_small_id: i64,
        free_units: i64,
        user_id: i64,
    ) -> Result<Option<Stack>> {
        let stack = sqlx::query(
            "
            WITH selected_stack AS (
                SELECT stack_small_id
                FROM stacks
                WHERE task_small_id = $1
                AND num_compute_units - already_computed_units - locked_compute_units >= $2
                AND user_id = $3
                AND is_claimed = false
                AND is_locked = false
                AND in_settle_period = false
                LIMIT 1
                FOR UPDATE
            )
            UPDATE stacks
            SET locked_compute_units = locked_compute_units + $2
            WHERE stack_small_id IN (SELECT stack_small_id FROM selected_stack)
            RETURNING *",
        )
        .bind(task_small_id)
        .bind(free_units)
        .bind(user_id)
        .fetch_optional(&self.db)
        .await?
        .map(|stack| Stack::from_row(&stack).map_err(AtomaStateManagerError::from))
        .transpose()?;
        Ok(stack)
    }

    /// Get tasks for model.
    ///
    /// This method fetches all tasks from the database that are associated with
    /// the given model through the `tasks` table.
    ///
    /// # Arguments
    ///
    /// * `model` - The model name for the task.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<Task>>`: A result containing either:
    ///  - `Ok(Vec<Task>)`: A vector of `Task` objects representing all tasks for the given model.
    ///  - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if the database query fails.
    #[instrument(level = "trace", skip_all, fields(%model))]
    pub async fn get_tasks_for_model(&self, model: &str) -> Result<Vec<Task>> {
        let tasks = sqlx::query("SELECT * FROM tasks WHERE model_name = $1 AND is_deprecated = $2")
            .bind(model)
            .bind(false)
            .fetch_all(&self.db)
            .await?;
        tasks
            .into_iter()
            .map(|task| Task::from_row(&task).map_err(AtomaStateManagerError::from))
            .collect()
    }

    /// Updates a stack as claimed and sets the user refund amount.
    ///
    /// This method updates the stack's status to claimed and sets the user refund amount.
    ///
    /// # Arguments
    ///
    /// * `stack_small_id` - The unique identifier of the stack to update.
    /// * `user_refund_amount` - The amount to refund to the user.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if the database query fails.
    #[instrument(level = "trace", skip_all, fields(%stack_small_id, %user_refund_amount))]
    pub async fn update_stack_claimed(
        &self,
        stack_small_id: i64,
        user_refund_amount: i64,
    ) -> Result<()> {
        sqlx::query("UPDATE stacks SET is_claimed = true, is_locked = true, user_refund_amount = $2 WHERE stack_small_id = $1")
            .bind(stack_small_id)
            .bind(user_refund_amount)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Gets the node with the cheapest price per one million compute units for a given model.
    ///
    /// This method queries the database to find the node subscription with the lowest price per one million compute units
    /// that matches the specified model and confidentiality requirements. It joins the tasks, node_subscriptions,
    /// and (optionally) node_public_keys tables to find valid subscriptions.
    ///
    /// # Arguments
    ///
    /// * `model` - The name of the model to search for (e.g., "gpt-4", "llama-2")
    /// * `is_confidential` - Whether to only return nodes that support confidential computing
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing:
    /// - `Ok(Some(CheapestNode))` - If a valid node subscription is found
    /// - `Ok(None)` - If no valid node subscription exists for the model
    /// - `Err(AtomaStateManagerError)` - If a database error occurs
    ///
    /// # Errors
    ///
    /// This function will return an error if the database query fails.
    ///
    /// The returned `CheapestNode` contains:
    /// - The task's small ID
    /// - The price per one million compute units
    /// - The maximum number of compute units supported
    ///
    /// # Security
    ///
    /// When `is_confidential` is true, this method:
    /// - Only returns nodes that have valid public keys
    /// - Only considers tasks with security level 2
    /// - Requires nodes to be registered in the node_public_keys table
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// # use atoma_state::AtomaState;
    /// # use sqlx::PgPool;
    ///
    /// async fn find_cheapest_node(state: &AtomaState) -> anyhow::Result<()> {
    ///     // Find cheapest non-confidential node for GPT-4
    ///     let regular_node = state.get_cheapest_node_for_model("gpt-4", false).await?;
    ///
    ///     // Find cheapest confidential node for GPT-4
    ///     let confidential_node = state.get_cheapest_node_for_model("gpt-4", true).await?;
    ///
    ///     Ok(())
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%model))]
    pub async fn get_cheapest_node_for_model(
        &self,
        model: &str,
        is_confidential: bool,
    ) -> Result<Option<CheapestNode>> {
        // TODO: benchmark this query performance
        let mut query = String::from(
            r"
            WITH latest_rotation AS (
                SELECT key_rotation_counter
                FROM key_rotations
                ORDER BY key_rotation_counter DESC
                LIMIT 1
            ),
            valid_nodes AS (
                SELECT DISTINCT npk.node_small_id
                FROM node_public_keys npk
                INNER JOIN latest_rotation ON latest_rotation.key_rotation_counter = npk.key_rotation_counter
                GROUP BY npk.node_small_id
                HAVING bool_and(npk.is_valid) = true
            )
            SELECT tasks.task_small_id, node_subscriptions.price_per_one_million_compute_units,
                node_subscriptions.max_num_compute_units, node_subscriptions.node_small_id
            FROM tasks
            INNER JOIN node_subscriptions ON tasks.task_small_id = node_subscriptions.task_small_id",
        );

        if is_confidential {
            query.push_str(
                r"
            INNER JOIN valid_nodes ON valid_nodes.node_small_id = node_subscriptions.node_small_id",
            );
        }

        query.push_str(
            r"
            WHERE tasks.is_deprecated = false
            AND tasks.model_name = $1
            AND node_subscriptions.valid = true",
        );

        if is_confidential {
            query.push_str(
                r"
            AND tasks.security_level = 1",
            );
        }

        query.push_str(
            r"
            ORDER BY node_subscriptions.price_per_one_million_compute_units
            LIMIT 1",
        );

        let node_settings = sqlx::query(&query)
            .bind(model)
            .fetch_optional(&self.db)
            .await?;
        Ok(node_settings
            .map(|node_settings| CheapestNode::from_row(&node_settings))
            .transpose()?)
    }

    /// Selects a node's public key for encryption based on model requirements and compute capacity.
    ///
    /// This method queries the database to find the cheapest valid node that:
    /// 1. Has a valid public key for encryption
    /// 2. Has an associated stack with sufficient remaining compute capacity
    /// 3. Supports the specified model with security level 1 (confidential computing)
    ///
    /// The nodes are ordered by each stac's price per one million compute units, so the most cost-effective stack meeting
    /// all requirements will be selected.
    ///
    /// # Arguments
    ///
    /// * `model` - The name of the model requiring encryption (e.g., "gpt-4", "llama-2")
    /// * `max_num_tokens` - The maximum number of compute units/tokens needed for the task
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing:
    /// - `Ok(Some(NodePublicKey))` - If a suitable node is found, returns its public key and ID
    /// - `Ok(None)` - If no suitable node is found
    /// - `Err(AtomaStateManagerError)` - If a database error occurs
    ///
    /// # Errors
    ///
    /// This function will return an error if the database query fails.
    ///
    /// # Database Query Details
    ///
    /// The method joins three tables:
    /// - `node_public_keys`: For encryption key information
    /// - `stacks`: For compute capacity and pricing
    /// - `tasks`: For model and security level verification
    ///
    /// The results are filtered to ensure:
    /// - The stack supports the requested model
    /// - The task requires security level 1 (confidential computing)
    /// - The stack has sufficient remaining compute units
    /// - The node's public key is valid
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// # use atoma_state::AtomaState;
    /// # use sqlx::PgPool;
    ///
    /// async fn encrypt_for_node(state: &AtomaState) -> anyhow::Result<()> {
    ///     // Find a node that can handle GPT-4 requests with up to1000 tokens
    ///     let node_key = state.select_node_public_key_for_encryption("gpt-4", 1000).await?;
    ///
    ///     if let Some(node_key) = node_key {
    ///         // Use the node's public key for encryption
    ///         // ...
    ///     }
    ///
    ///     Ok(())
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%model, %max_num_tokens))]
    pub async fn select_node_public_key_for_encryption(
        &self,
        model: &str,
        max_num_tokens: i64,
        user_id: i64,
    ) -> Result<Option<NodePublicKey>> {
        // NOTE: We don't inner join with stack_settlement_tickets because we want to allow,
        // as this method is dedicated for confidential compute requests/tasks.
        let node = sqlx::query(
            r"
            WITH latest_rotation AS (
                SELECT MAX(key_rotation_counter) as key_rotation_counter
                FROM key_rotations
            ),
            valid_nodes AS (
                SELECT DISTINCT npk.node_small_id, npk.public_key
                FROM node_public_keys npk
                INNER JOIN latest_rotation ON latest_rotation.key_rotation_counter = npk.key_rotation_counter
                GROUP BY npk.node_small_id, npk.public_key
                HAVING bool_and(npk.is_valid) = true
            ),
            selected_stack AS (
                SELECT vn.public_key, vn.node_small_id, s.stack_small_id
                FROM valid_nodes vn
                INNER JOIN stacks s ON s.selected_node_id = vn.node_small_id
                INNER JOIN tasks t ON t.task_small_id = s.task_small_id
                WHERE t.model_name = $1
                AND t.security_level = 1
                AND t.is_deprecated = false
                AND s.num_compute_units - s.already_computed_units - s.locked_compute_units >= $2
                AND s.is_claimed = false
                AND s.is_locked = false
                AND s.user_id = $3
                ORDER BY s.price_per_one_million_compute_units ASC
                LIMIT 1
            )
            UPDATE stacks
            SET locked_compute_units = stacks.locked_compute_units + $2
            FROM selected_stack
            WHERE stacks.stack_small_id = selected_stack.stack_small_id
            RETURNING selected_stack.public_key, selected_stack.node_small_id, selected_stack.stack_small_id
            ",
        )
        .bind(model)
        .bind(max_num_tokens)
        .bind(user_id)
        .fetch_optional(&self.db)
        .await?;
        node.map(|node| NodePublicKey::from_row(&node).map_err(AtomaStateManagerError::from))
            .transpose()
    }

    /// Retrieves a node's public key for encryption by its node ID.
    ///
    /// This method queries the database to find the public key associated with a specific node.
    /// Unlike `select_node_public_key_for_encryption`, this method looks up the key directly by
    /// node ID without considering model requirements or compute capacity.
    ///
    /// # Arguments
    ///
    /// * `node_small_id` - The unique identifier of the node whose public key is being requested
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing:
    /// - `Ok(Some(NodePublicKey))` - If a valid public key is found for the node
    /// - `Ok(None)` - If no public key exists for the specified node
    /// - `Err(AtomaStateManagerError)` - If a database error occurs
    ///
    /// # Errors
    ///
    /// This function will return an error if the database query fails.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// # use atoma_state::AtomaState;
    /// # use sqlx::PgPool;
    ///
    /// async fn get_node_key(state: &AtomaState) -> anyhow::Result<()> {
    ///     let node_id = 123;
    ///     let node_key = state.select_node_public_key_for_encryption_for_node(node_id).await?;
    ///
    ///     if let Some(key) = node_key {
    ///         // Use the node's public key for encryption
    ///         // ...
    ///     }
    ///
    ///     Ok(())
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%node_small_id))]
    pub async fn select_node_public_key_for_encryption_for_node(
        &self,
        node_small_id: i64,
    ) -> Result<Option<NodePublicKey>> {
        let node_public_key =
            sqlx::query(
                r"
                WITH latest_rotation AS (
                    SELECT key_rotation_counter
                    FROM key_rotations
                    ORDER BY key_rotation_counter DESC
                    LIMIT 1
                ),
                valid_node AS (
                    SELECT npk.node_small_id, npk.public_key
                    FROM node_public_keys npk
                    INNER JOIN latest_rotation ON latest_rotation.key_rotation_counter = npk.key_rotation_counter
                    WHERE npk.node_small_id = $1
                    GROUP BY npk.node_small_id, npk.public_key
                    HAVING bool_and(npk.is_valid) = true
                )
                SELECT node_small_id, public_key
                FROM valid_node
                "
            )
                .bind(node_small_id)
                .fetch_optional(&self.db)
                .await?;
        Ok(node_public_key
            .map(|node_public_key| NodePublicKey::from_row(&node_public_key))
            .transpose()?)
    }

    /// Retrieves all tasks from the database.
    ///
    /// This method fetches all task records from the `tasks` table in the database.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<Task>>`: A result containing either:
    ///   - `Ok(Vec<Task>)`: A vector of all `Task` objects in the database.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue converting the database rows into `Task` objects.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn list_all_tasks(state_manager: &AtomaStateManager) -> Result<Vec<Task>, AtomaStateManagerError> {
    ///     state_manager.get_all_tasks().await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn get_all_tasks(&self) -> Result<Vec<Task>> {
        let tasks = sqlx::query("SELECT * FROM tasks")
            .fetch_all(&self.db)
            .await?;
        tasks
            .into_iter()
            .map(|task| Task::from_row(&task).map_err(AtomaStateManagerError::from))
            .collect()
    }

    /// Inserts a new task into the database.
    ///
    /// This method takes a `Task` object and inserts its data into the `tasks` table
    /// in the database.
    ///
    /// # Arguments
    ///
    /// * `task` - A `Task` struct containing all the information about the task to be inserted.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's a constraint violation (e.g., duplicate `task_small_id` or `task_id`).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, Task};
    ///
    /// async fn add_task(state_manager: &mut AtomaStateManager, task: Task) -> Result<(), AtomaStateManagerError> {
    ///     state_manager.insert_new_task(task).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(task_id = %task.task_small_id))]
    pub async fn insert_new_task(&self, task: Task) -> Result<()> {
        sqlx::query(
            "INSERT INTO tasks (
                task_small_id, task_id, role, model_name, is_deprecated,
                valid_until_epoch, security_level, minimum_reputation_score
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(task.task_small_id)
        .bind(task.task_id)
        .bind(task.role)
        .bind(task.model_name)
        .bind(task.is_deprecated)
        .bind(task.valid_until_epoch)
        .bind(task.security_level)
        .bind(task.minimum_reputation_score)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Deprecates a task in the database based on its small ID.
    ///
    /// This method updates the `is_deprecated` field of a task to `TRUE` in the `tasks` table
    /// using the provided `task_small_id`.
    ///
    /// # Arguments
    ///
    /// * `task_small_id` - The unique small identifier for the task to be deprecated.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - The task with the specified `task_small_id` doesn't exist.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn deprecate_task(state_manager: &AtomaStateManager, task_small_id: i64) -> Result<(), AtomaStateManagerError> {
    ///     state_manager.deprecate_task(task_small_id).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%task_small_id))]
    pub async fn deprecate_task(&self, task_small_id: i64, epoch: i64) -> Result<()> {
        sqlx::query("UPDATE tasks SET is_deprecated = TRUE, deprecated_at_epoch = $1 WHERE task_small_id = $2")
            .bind(epoch)
            .bind(task_small_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Retrieves all tasks subscribed to by a specific node.
    ///
    /// This method fetches all tasks from the database that are associated with
    /// the given node through the `node_subscriptions` table.
    ///
    /// # Arguments
    ///
    /// * `node_small_id` - The unique identifier of the node whose subscribed tasks are to be fetched.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<Task>>`: A result containing either:
    ///   - `Ok(Vec<Task>)`: A vector of `Task` objects representing all tasks subscribed to by the node.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue converting the database rows into `Task` objects.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_node_tasks(state_manager: &AtomaStateManager, node_small_id: i64) -> Result<Vec<Task>, AtomaStateManagerError> {
    ///     state_manager.get_subscribed_tasks(node_small_id).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%node_small_id))]
    pub async fn get_subscribed_tasks(&self, node_small_id: i64) -> Result<Vec<Task>> {
        let tasks = sqlx::query(
            "SELECT tasks.* FROM tasks
            INNER JOIN node_subscriptions ON tasks.task_small_id = node_subscriptions.task_small_id
            WHERE node_subscriptions.node_small_id = $1",
        )
        .bind(node_small_id)
        .fetch_all(&self.db)
        .await?;
        tasks
            .into_iter()
            .map(|task| Task::from_row(&task).map_err(AtomaStateManagerError::from))
            .collect()
    }

    /// Retrieves all node subscriptions.
    ///
    /// This method fetches all subscription records from the `node_subscriptions` table.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<NodeSubscription>>`: A result containing either:
    ///   - `Ok(Vec<NodeSubscription>)`: A vector of `NodeSubscription` objects representing all found subscriptions.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue converting the database rows into `NodeSubscription` objects.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, NodeSubscription};
    ///
    /// async fn get_subscriptions(state_manager: &AtomaStateManager) -> Result<Vec<NodeSubscription>, AtomaStateManagerError> {
    ///     state_manager.get_all_node_subscriptions().await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn get_all_node_subscriptions(&self) -> Result<Vec<NodeSubscription>> {
        let subscriptions = sqlx::query("SELECT * FROM node_subscriptions")
            .fetch_all(&self.db)
            .await?;

        subscriptions
            .into_iter()
            .map(|subscription| {
                NodeSubscription::from_row(&subscription).map_err(AtomaStateManagerError::from)
            })
            .collect()
    }

    /// Retrieves all node subscriptions for a specific task.
    ///
    /// This method fetches all subscription records from the `node_subscriptions` table
    ///
    /// # Arguments
    ///
    /// * `task_small_id` - The unique identifier of the task to fetch subscriptions for.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<NodeSubscription>>`: A result containing either:
    ///   - `Ok(Vec<NodeSubscription>)`: A vector of `NodeSubscription` objects representing all found subscriptions.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    ///
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, NodeSubscription};
    ///
    /// async fn get_all_node_subscriptions_for_task(state_manager: &AtomaStateManager, task_small_id: i64) -> Result<Vec<NodeSubscription>, AtomaStateManagerError> {
    ///    state_manager.get_all_node_subscriptions_for_task(task_small_id).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(?task_small_id))]
    pub async fn get_all_node_subscriptions_for_task(
        &self,
        task_small_id: i64,
    ) -> Result<Vec<NodeSubscription>> {
        let subscriptions =
            sqlx::query("SELECT * FROM node_subscriptions WHERE task_small_id = $1")
                .bind(task_small_id)
                .fetch_all(&self.db)
                .await?;

        subscriptions
            .into_iter()
            .map(|subscription| {
                NodeSubscription::from_row(&subscription).map_err(AtomaStateManagerError::from)
            })
            .collect()
    }

    /// Retrieves the attestation for a specific node.
    /// This method fetches the attestation record from the `node_attestations` table
    /// for a given node identified by its small ID.
    ///
    /// # Arguments
    ///
    /// * `node_small_id` - The unique identifier of the node for which the attestation is requested
    ///
    /// # Returns
    ///
    /// - `Result<Option<NodeAttestation>>`: A result containing either:
    ///   - `Ok(Some(NodeAttestation))`: The `NodeAttestation` object if found.
    ///   - `Ok(None)`: If no attestation exists for the specified node.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue converting the database row into a `NodeAttestation` object.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, NodeAttestation};
    ///
    /// async fn get_node_attestation(state_manager: &AtomaStateManager, node_small_id: i64) -> Result<Option<NodeAttestation>, AtomaStateManagerError> {
    ///     state_manager.get_node_attestation(node_small_id).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%node_small_id))]
    pub async fn get_node_attestation(
        &self,
        node_small_id: i64,
    ) -> Result<Option<NodeAttestation>> {
        let attestation = sqlx::query("SELECT * FROM node_attestations WHERE node_small_id = $1")
            .bind(node_small_id)
            .fetch_optional(&self.db)
            .await?;
        attestation
            .map(|attestation| {
                NodeAttestation::from_row(&attestation).map_err(AtomaStateManagerError::from)
            })
            .transpose()
    }

    /// Updates or inserts a node attestation in the database.
    /// This method either updates an existing attestation for a node or inserts a new one
    /// if it does not already exist.
    ///
    /// # Arguments
    ///
    /// * `attestation` - The `NodeAttestation` object containing the node's small ID,
    ///   the attestation data, and its validity status.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue with the data being inserted or updated.
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, NodeAttestation};
    ///
    /// async fn update_node_attestation(state_manager: &AtomaStateManager, attestation: NodeAttestation) -> Result<(), AtomaStateManagerError> {
    ///     state_manager.update_node_attestation(attestation).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(node_small_id = %attestation.node_small_id))]
    pub async fn update_node_attestation(&self, attestation: NodeAttestation) -> Result<()> {
        sqlx::query(
            "INSERT INTO node_attestations (node_small_id, compressed_evidence)
             VALUES ($1, $2)
             ON CONFLICT (node_small_id) DO UPDATE SET
             compressed_evidence = EXCLUDED.compressed_evidence,
             updated_at = NOW()",
        )
        .bind(attestation.node_small_id)
        .bind(attestation.compressed_evidence)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Retrieves all stacks that are not settled.
    ///
    /// This method fetches all stack records from the `stacks` table that are not in the settle period.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<Stack>>`: A result containing either:
    ///   - `Ok(Vec<Stack>)`: A vector of `Stack` objects representing all stacks that are not in the settle period.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if the database query fails.
    ///
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_current_stacks(state_manager: &AtomaStateManager) -> Result<Vec<Stack>, AtomaStateManagerError> {
    ///    state_manager.get_current_stacks().await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn get_current_stacks(&self) -> Result<Vec<Stack>> {
        let stacks = sqlx::query("SELECT * FROM stacks WHERE in_settle_period = false")
            .fetch_all(&self.db)
            .await?;
        stacks
            .into_iter()
            .map(|stack| Stack::from_row(&stack).map_err(AtomaStateManagerError::from))
            .collect()
    }

    /// Get stacks created/settled for the last `last_hours` hours.
    ///
    /// This method fetches the stacks created/settled for the last `last_hours` hours from the `stats_stacks` table.
    ///
    /// # Arguments
    ///
    /// * `last_hours` - The number of hours to fetch the stacks stats.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<StatsStackResponse>>`: A result containing either:
    ///   - `Ok(Vec<StatsStackResponse>)`: The stacks stats for the last `last_hours` hours.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if the database query fails.
    ///
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_current_stacks(state_manager: &AtomaStateManager) -> Result<Vec<StatsStackResponse>, AtomaStateManagerError> {
    ///    state_manager.get_stats_stacks(5).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn get_stats_stacks(&self, last_hours: usize) -> Result<Vec<StatsStackResponse>> {
        let timestamp = Utc::now();
        let start_timestamp = timestamp
            .checked_sub_signed(chrono::Duration::hours(last_hours as i64))
            .ok_or(AtomaStateManagerError::InvalidTimestamp)?;
        let stats_stacks =
            sqlx::query("SELECT * FROM stats_stacks WHERE timestamp >= $1 ORDER BY timestamp ASC")
                .bind(start_timestamp)
                .fetch_all(&self.db)
                .await?;
        stats_stacks
            .into_iter()
            .map(|stack| StatsStackResponse::from_row(&stack).map_err(AtomaStateManagerError::from))
            .collect()
    }

    /// Get the distribution of nodes by country.
    /// This method fetches the distribution of nodes by country from the `nodes` table.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<NodeDistribution>>`: A result containing either:
    ///  - `Ok(Vec<NodeDistribution>)`: The distribution of nodes by country.
    /// - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_nodes_distribution(state_manager: &AtomaStateManager) -> Result<Vec<NodeDistribution>, AtomaStateManagerError> {
    ///    state_manager.get_nodes_distribution().await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn get_nodes_distribution(&self) -> Result<Vec<NodeDistribution>> {
        let nodes_distribution = sqlx::query(
            "SELECT country, COUNT(*) AS count FROM nodes GROUP BY country ORDER BY count DESC",
        )
        .fetch_all(&self.db)
        .await?;
        nodes_distribution
            .into_iter()
            .map(|node| NodeDistribution::from_row(&node).map_err(AtomaStateManagerError::from))
            .collect()
    }

    /// Register user with password.
    ///
    /// This method inserts a new entry into the `users` table to register a new user. In case the user already exists, it returns None.
    ///
    /// # Arguments
    ///
    /// * `email` - The email of the user.
    /// * `password_hash` - The password hash of the user.
    ///
    /// # Returns
    ///
    /// - `Result<Option<i64>>`: A result containing either:
    ///   - `Ok(Some(i64))`: The ID of the user if the user was successfully registered.
    ///   - `Ok(None)`: If the user already exists.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if the database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn register_user(state_manager: &AtomaStateManager, email: &str, password_hash: &str) -> Result<Option<i64>, AtomaStateManagerError> {
    ///    state_manager.register(email, password_hash).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn register(
        &self,
        user_profile: UserProfile,
        password_hash: &str,
        password_salt: &str,
    ) -> Result<Option<i64>> {
        let result = sqlx::query("INSERT INTO users (email, password_hash, password_salt) VALUES ($1, $2, $3) ON CONFLICT (email) DO NOTHING RETURNING id")
        .bind(user_profile.email)
        .bind(password_hash)
        .bind(password_salt)
        .fetch_optional(&self.db)
        .await?;
        Ok(result.map(|record| record.get("id")))
    }

    /// Get the password salt of a user.
    ///
    /// This method fetches the password salt of a user from the `users` table.
    ///
    /// # Arguments
    ///
    /// * `email` - The email of the user.
    ///
    /// # Returns
    ///
    /// - `Result<Option<String>>`: A result containing either:
    ///   - `Ok(Some(String))`: The password salt of the user.
    ///   - `Ok(None)`: If the user does not exist.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if the database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_password_salt(state_manager: &AtomaStateManager, email: &str) -> Result<Option<String>, AtomaStateManagerError> {
    ///   state_manager.get_password_salt(email).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn get_password_salt(&self, email: &str) -> Result<Option<String>> {
        let result = sqlx::query("SELECT password_salt FROM users WHERE email = $1")
            .bind(email)
            .fetch_optional(&self.db)
            .await?;
        Ok(result.map(|record| record.get("password_salt")))
    }

    /// Checks if a node is subscribed to a specific task.
    ///
    /// This method queries the `node_subscriptions` table to determine if there's
    /// an entry for the given node and task combination.
    ///
    /// # Arguments
    ///
    /// * `node_small_id` - The unique identifier of the node.
    /// * `task_small_id` - The unique identifier of the task.
    ///
    /// # Returns
    ///
    /// - `Result<bool>`: A result containing either:
    ///   - `Ok(true)` if the node is subscribed to the task.
    ///   - `Ok(false)` if the node is not subscribed to the task.
    ///   - `Err(AtomaStateManagerError)` if there's a database error.
    ///
    /// # Errors
    ///
    /// This function will return an error if the database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn check_subscription(state_manager: &AtomaStateManager, node_small_id: i64, task_small_id: i64) -> Result<bool, AtomaStateManagerError> {
    ///     state_manager.is_node_subscribed_to_task(node_small_id, task_small_id).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%node_small_id, %task_small_id))]
    pub async fn is_node_subscribed_to_task(
        &self,
        node_small_id: i64,
        task_small_id: i64,
    ) -> Result<bool> {
        let result = sqlx::query(
            "SELECT COUNT(*) FROM node_subscriptions WHERE node_small_id = $1 AND task_small_id = $2",
        )
        .bind(node_small_id)
        .bind(task_small_id)
        .fetch_one(&self.db)
        .await?;
        let count: i64 = result.get(0);
        Ok(count > 0)
    }

    /// Subscribes a node to a task with a specified price per one million compute units.
    ///
    /// This method inserts a new entry into the `node_subscriptions` table to
    /// establish a subscription relationship between a node and a task, along
    /// with the specified price per one million compute units for the subscription.
    ///
    /// # Arguments
    ///
    /// * `node_small_id` - The unique identifier of the node to be subscribed.
    /// * `task_small_id` - The unique identifier of the task to which the node is subscribing.
    /// * `price_per_one_million_compute_units` - The price per compute unit for the subscription.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's a constraint violation (e.g., duplicate subscription).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn subscribe_node_to_task(state_manager: &AtomaStateManager, node_small_id: i64, task_small_id: i64, price_per_one_million_compute_units: i64) -> Result<(), AtomaStateManagerError> {
    ///     state_manager.subscribe_node_to_task(node_small_id, task_small_id, price_per_one_million_compute_units).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(
            %node_small_id,
            %task_small_id,
            %price_per_one_million_compute_units,
            %max_num_compute_units
        )
    )]
    pub async fn subscribe_node_to_task(
        &self,
        node_small_id: i64,
        task_small_id: i64,
        price_per_one_million_compute_units: i64,
        max_num_compute_units: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO node_subscriptions
                (node_small_id, task_small_id, price_per_one_million_compute_units, max_num_compute_units, valid)
                VALUES ($1, $2, $3, $4, TRUE)",
        )
            .bind(node_small_id)
            .bind(task_small_id)
            .bind(price_per_one_million_compute_units)
            .bind(max_num_compute_units)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Retrieves the node subscription associated with a specific task ID.
    ///
    /// This method fetches the node subscription details from the `node_subscriptions` table
    /// based on the provided `task_small_id`.
    ///
    /// # Arguments
    ///
    /// * `task_small_id` - The unique small identifier of the task to retrieve the subscription for.
    ///
    /// # Returns
    ///
    /// - `Result<NodeSubscription>`: A result containing either:
    ///   - `Ok(NodeSubscription)`: A `NodeSubscription` object representing the subscription details.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue converting the database row into a `NodeSubscription` object.
    /// - No subscription is found for the given `task_small_id`.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, NodeSubscription};
    ///
    /// async fn get_subscription(state_manager: &AtomaStateManager, task_small_id: i64) -> Result<NodeSubscription, AtomaStateManagerError> {
    ///     state_manager.get_node_subscription_by_task_small_id(task_small_id).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(%task_small_id)
    )]
    pub async fn get_node_subscription_by_task_small_id(
        &self,
        task_small_id: i64,
    ) -> Result<NodeSubscription> {
        let subscription = sqlx::query("SELECT * FROM node_subscriptions WHERE task_small_id = $1")
            .bind(task_small_id)
            .fetch_one(&self.db)
            .await?;
        Ok(NodeSubscription::from_row(&subscription)?)
    }

    /// Updates an existing node subscription to a task with new price and compute unit values.
    ///
    /// This method updates an entry in the `node_subscriptions` table, modifying the
    /// price per one million compute units and the maximum number of compute units for an existing
    /// subscription between a node and a task.
    ///
    /// # Arguments
    ///
    /// * `node_small_id` - The unique identifier of the subscribed node.
    /// * `task_small_id` - The unique identifier of the task to which the node is subscribed.
    /// * `price_per_one_million_compute_units` - The new price per compute unit for the subscription.
    /// * `max_num_compute_units` - The new maximum number of compute units for the subscription.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - The specified node subscription doesn't exist.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn update_subscription(state_manager: &AtomaStateManager) -> Result<(), AtomaStateManagerError> {
    ///     state_manager.update_node_subscription(1, 2, 100, 1000).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(
            %node_small_id,
            %task_small_id,
            %price_per_one_million_compute_units,
            %max_num_compute_units
        )
    )]
    pub async fn update_node_subscription(
        &self,
        node_small_id: i64,
        task_small_id: i64,
        price_per_one_million_compute_units: i64,
        max_num_compute_units: i64,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE node_subscriptions SET price_per_one_million_compute_units = $1, max_num_compute_units = $2 WHERE node_small_id = $3 AND task_small_id = $4",
        )
            .bind(price_per_one_million_compute_units)
            .bind(max_num_compute_units)
            .bind(node_small_id)
            .bind(task_small_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Unsubscribes a node from a task.
    ///
    /// This method updates the `valid` field of the `node_subscriptions` table to `FALSE` for the specified node and task combination.
    ///
    /// # Arguments
    ///
    /// * `node_small_id` - The unique identifier of the node to be unsubscribed.
    /// * `task_small_id` - The unique identifier of the task from which the node is unsubscribed.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    /// # Errors
    ///
    /// This function will return an error if the database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn unsubscribe_node(state_manager: &AtomaStateManager, node_small_id: i64, task_small_id: i64) -> Result<(), AtomaStateManagerError> {
    ///     state_manager.unsubscribe_node_from_task(node_small_id, task_small_id).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(%node_small_id, %task_small_id)
    )]
    pub async fn unsubscribe_node_from_task(
        &self,
        node_small_id: i64,
        task_small_id: i64,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM node_subscriptions WHERE node_small_id = $1 AND task_small_id = $2",
        )
        .bind(node_small_id)
        .bind(task_small_id)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Retrieves the stack associated with a specific stack ID.
    ///
    /// This method fetches the stack details from the `stacks` table based on the provided `stack_id`.
    ///
    /// # Arguments
    ///
    /// * `stack_small_id` - The unique small identifier of the stack to retrieve.
    ///
    /// # Returns
    ///
    /// - `Result<Stack>`: A result containing either:
    ///   - `Ok(Stack)`: A `Stack` object representing the stack.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue converting the database rows into a `Stack` object.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_stack(state_manager: &AtomaStateManager, stack_small_id: i64) -> Result<Stack, AtomaStateManagerError> {
    ///     state_manager.get_stack(stack_id).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(%stack_small_id)
    )]
    pub async fn get_stack(&self, stack_small_id: i64) -> Result<Stack> {
        let stack = sqlx::query("SELECT * FROM stacks WHERE stack_small_id = $1")
            .bind(stack_small_id)
            .fetch_one(&self.db)
            .await?;
        Ok(Stack::from_row(&stack)?)
    }

    /// Retrieves multiple stacks from the database by their small IDs.
    ///
    /// This method efficiently fetches multiple stack records from the `stacks` table in a single query
    /// by using an IN clause with the provided stack IDs.
    ///
    /// # Arguments
    ///
    /// * `stack_small_ids` - A slice of stack IDs to retrieve from the database.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<Stack>>`: A result containing either:
    ///   - `Ok(Vec<Stack>)`: A vector of `Stack` objects corresponding to the requested IDs.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue converting the database rows into `Stack` objects.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, Stack};
    ///
    /// async fn get_multiple_stacks(state_manager: &AtomaStateManager) -> Result<Vec<Stack>, AtomaStateManagerError> {
    ///     let stack_ids = &[1, 2, 3]; // IDs of stacks to retrieve
    ///     state_manager.get_stacks(stack_ids).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(?stack_small_ids)
    )]
    pub async fn get_stacks(&self, stack_small_ids: &[i64]) -> Result<Vec<Stack>> {
        let mut query_builder = build_query_with_in(
            "SELECT * FROM stacks",
            "stack_small_id",
            stack_small_ids,
            None,
        );
        let stacks = query_builder.build().fetch_all(&self.db).await?;
        stacks
            .into_iter()
            .map(|stack| Stack::from_row(&stack).map_err(AtomaStateManagerError::from))
            .collect()
    }

    /// Retrieves all stacks associated with the given node IDs.
    ///
    /// This method fetches all stack records from the database that are associated with any
    /// of the provided node IDs in the `node_small_ids` array.
    ///
    /// # Arguments
    ///
    /// * `node_small_ids` - A slice of node IDs to fetch stacks for.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<Stack>>`: A result containing either:
    ///   - `Ok(Vec<Stack>)`: A vector of `Stack` objects representing all stacks found.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue converting the database rows into `Stack` objects.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, Stack};
    ///
    /// async fn get_stacks(state_manager: &AtomaStateManager) -> Result<Vec<Stack>, AtomaStateManagerError> {
    ///     let node_ids = &[1, 2, 3];
    ///     state_manager.get_all_stacks(node_ids).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(?node_small_ids)
    )]
    pub async fn get_stacks_by_node_small_ids(&self, node_small_ids: &[i64]) -> Result<Vec<Stack>> {
        let mut query_builder = build_query_with_in(
            "SELECT * FROM stacks",
            "selected_node_id",
            node_small_ids,
            None,
        );

        let stacks = query_builder.build().fetch_all(&self.db).await?;

        stacks
            .into_iter()
            .map(|stack| Stack::from_row(&stack).map_err(AtomaStateManagerError::from))
            .collect()
    }

    /// Retrieves all stacks associated with a specific node ID.
    ///
    /// This method fetches all stack records from the `stacks` table that are associated
    /// with the provided `node_small_id`.
    ///
    /// # Arguments
    ///
    /// * `node_small_id` - The unique identifier of the node whose stacks should be retrieved.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<Stack>>`: A result containing either:
    ///   - `Ok(Vec<Stack>)`: A vector of `Stack` objects associated with the given node ID.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue converting the database rows into `Stack` objects.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, Stack};
    ///
    /// async fn get_node_stacks(state_manager: &AtomaStateManager, node_small_id: i64) -> Result<Vec<Stack>, AtomaStateManagerError> {
    ///     state_manager.get_stack_by_id(node_small_id).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(%node_small_id)
    )]
    pub async fn get_stack_by_id(&self, node_small_id: i64) -> Result<Vec<Stack>> {
        let stacks = sqlx::query("SELECT * FROM stacks WHERE selected_node_id = $1")
            .bind(node_small_id)
            .fetch_all(&self.db)
            .await?;
        stacks
            .into_iter()
            .map(|stack| Stack::from_row(&stack).map_err(AtomaStateManagerError::from))
            .collect()
    }

    /// Retrieves all stacks associated with a specific user ID.
    ///
    /// This method fetches all stack records from the `stacks` table that are associated
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user whose stacks should be retrieved.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<Stack>>`: A result containing either:
    ///  - `Ok(Vec<Stack>)`: A vector of `Stack` objects associated with the given user ID.
    /// - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue converting the database rows into `Stack` objects.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, Stack};
    ///
    /// async fn get_user_stacks(state_manager: &AtomaStateManager, user_id: i64) -> Result<Vec<Stack>, AtomaStateManagerError> {
    ///    state_manager.get_stacks_by_user_id(user_id).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(%user_id)
    )]
    pub async fn get_stacks_by_user_id(&self, user_id: i64) -> Result<Vec<(Stack, DateTime<Utc>)>> {
        let stacks = sqlx::query("SELECT * FROM stacks WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(&self.db)
            .await?;
        stacks
            .into_iter()
            .map(|row| {
                Stack::from_row(&row)
                    .map_err(AtomaStateManagerError::from)
                    .map(|stack| (stack, row.get("acquired_timestamp")))
            })
            .collect()
    }

    /// Retrieves stacks that are almost filled beyond a specified fraction threshold.
    ///
    /// This method fetches all stacks from the database where:
    /// 1. The stack belongs to one of the specified nodes (`node_small_ids`)
    /// 2. The number of already computed units exceeds the specified fraction of total compute units
    ///    (i.e., `already_computed_units > num_compute_units * fraction`)
    ///
    /// # Arguments
    ///
    /// * `node_small_ids` - A slice of node IDs to check for almost filled stacks.
    /// * `fraction` - A floating-point value between 0 and 1 representing the threshold fraction.
    ///                 For example, 0.8 means stacks that are more than 80% filled.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<Stack>>`: A result containing either:
    ///   - `Ok(Vec<Stack>)`: A vector of `Stack` objects that meet the filling criteria.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue converting the database rows into `Stack` objects.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_filled_stacks(state_manager: &AtomaStateManager) -> Result<Vec<Stack>, AtomaStateManagerError> {
    ///     let node_ids = &[1, 2, 3];  // Check stacks for these nodes
    ///     let threshold = 0.8;        // Look for stacks that are 80% or more filled
    ///
    ///     state_manager.get_almost_filled_stacks(node_ids, threshold).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(?node_small_ids, %fraction)
    )]
    pub async fn get_almost_filled_stacks(
        &self,
        node_small_ids: &[i64],
        fraction: f64,
    ) -> Result<Vec<Stack>> {
        let mut query_builder = build_query_with_in(
            "SELECT * FROM stacks",
            "selected_node_id",
            node_small_ids,
            Some("CAST(already_computed_units AS FLOAT) / CAST(num_compute_units AS FLOAT) > "),
        );

        // Add the fraction value directly in the SQL
        query_builder.push(fraction.to_string());

        let stacks = query_builder.build().fetch_all(&self.db).await?;

        stacks
            .into_iter()
            .map(|stack| Stack::from_row(&stack).map_err(AtomaStateManagerError::from))
            .collect()
    }

    /// Retrieves and updates an available stack with the specified number of compute units.
    ///
    /// This method attempts to reserve a specified number of compute units from a stack
    /// owned by the given public key. It performs this operation atomically using a database
    /// transaction to ensure consistency.
    ///
    /// # Arguments
    ///
    /// * `stack_small_id` - The unique identifier of the stack.
    /// * `public_key` - The public key of the stack owner.
    /// * `num_compute_units` - The number of compute units to reserve.
    ///
    /// # Returns
    ///
    /// - `Result<Option<Stack>>`: A result containing either:
    ///   - `Ok(Some(Stack))`: If the stack was successfully updated, returns the updated stack.
    ///   - `Ok(None)`: If the stack couldn't be updated (e.g., insufficient compute units or stack in settle period).
    ///   - `Err(AtomaStateManagerError)`: If there was an error during the database operation.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database transaction fails to begin, execute, or commit.
    /// - There's an issue with the SQL query execution.
    ///
    /// # Details
    ///
    /// The function performs the following steps:
    /// 1. Begins a database transaction.
    /// 2. Attempts to update the stack by increasing the `already_computed_units` field.
    /// 3. If the update is successful (i.e., the stack has enough available compute units),
    ///    it fetches the updated stack information.
    /// 4. Commits the transaction.
    ///
    /// The update will only succeed if:
    /// - The stack belongs to the specified owner (public key).
    /// - The stack has enough remaining compute units.
    /// - The stack is not in the settle period.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, Stack};
    ///
    /// async fn reserve_compute_units(state_manager: &AtomaStateManager) -> Result<Option<Stack>, AtomaStateManagerError> {
    ///     let stack_small_id = 1;
    ///     let public_key = "owner_public_key";
    ///     let num_compute_units = 100;
    ///
    ///     state_manager.get_available_stack_with_compute_units(stack_small_id, public_key, num_compute_units).await
    /// }
    /// ```
    ///
    /// This function is particularly useful for atomically reserving compute units from a stack,
    /// ensuring that the operation is thread-safe and consistent even under concurrent access.
    #[instrument(
        level = "trace",
        skip_all,
        fields(
            %stack_small_id,
            %public_key,
            %num_compute_units
        )
    )]
    pub async fn get_available_stack_with_compute_units(
        &self,
        stack_small_id: i64,
        public_key: &str,
        num_compute_units: i64,
    ) -> Result<Option<Stack>> {
        // Single query that updates and returns the modified row
        let maybe_stack = sqlx::query_as::<_, Stack>(
            r"
            UPDATE stacks
            SET locked_compute_units = locked_compute_units + $1
            WHERE stack_small_id = $2
            AND owner = $3
            AND num_compute_units - already_computed_units - locked_compute_units>= $1
            AND in_settle_period = false
            RETURNING *
            ",
        )
        .bind(num_compute_units)
        .bind(stack_small_id)
        .bind(public_key)
        .fetch_optional(&self.db)
        .await?;

        Ok(maybe_stack)
    }

    /// Locks (reserves) a specified number of compute units for a stack.
    ///
    /// This method atomically updates the `already_computed_units` field in the `stacks` table
    /// by adding the specified number of compute units to the current value. This effectively
    /// "locks" these compute units for use by preventing other processes from using the same units.
    ///
    /// # Arguments
    ///
    /// * `stack_small_id` - The unique small identifier of the stack to lock compute units for.
    /// * `available_compute_units` - The number of compute units to lock/reserve.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - The specified stack is not found (`AtomaStateManagerError::StackNotFound`).
    /// - The specified stack does not have enough compute units to lock.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn lock_units(state_manager: &AtomaStateManager) -> Result<(), AtomaStateManagerError> {
    ///     let stack_small_id = 1;
    ///     let units_to_lock = 100;
    ///
    ///     state_manager.lock_compute_units_for_stack(stack_small_id, units_to_lock).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(
            %stack_small_id,
            %available_compute_units
        )
    )]
    pub async fn lock_compute_units(
        &self,
        stack_small_id: i64,
        available_compute_units: i64,
    ) -> Result<()> {
        let result = sqlx::query(
            "UPDATE stacks
            SET locked_compute_units = locked_compute_units + $1
            WHERE stack_small_id = $2
            AND already_computed_units + locked_compute_units + $1 <= num_compute_units",
        )
        .bind(available_compute_units)
        .bind(stack_small_id)
        .execute(&self.db)
        .await?;
        if result.rows_affected() == 0 {
            return Err(AtomaStateManagerError::StackNotFound);
        }
        Ok(())
    }

    /// Inserts a new stack into the database.
    ///
    /// This method inserts a new entry into the `stacks` table with the provided stack details.
    ///
    /// # Arguments
    ///
    /// * `stack` - The `Stack` object to be inserted into the database.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue converting the provided `Stack` object into a database row.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn insert_stack(state_manager: &AtomaStateManager, stack: Stack) -> Result<(), AtomaStateManagerError> {
    ///     state_manager.insert_new_stack(stack).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(stack_small_id = %stack.stack_small_id,
            stack_id = %stack.stack_id,
            task_small_id = %stack.task_small_id,
            selected_node_id = %stack.selected_node_id,
            num_compute_units = %stack.num_compute_units,
            price = %stack.price_per_one_million_compute_units)
        )]
    pub async fn insert_new_stack(
        &self,
        stack: Stack,
        user_id: i64,
        acquired_timestamp: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO stacks (
                owner,
                stack_small_id,
                stack_id,
                task_small_id,
                selected_node_id,
                num_compute_units,
                price_per_one_million_compute_units,
                already_computed_units,
                locked_compute_units,
                in_settle_period,
                num_total_messages,
                user_id,
                acquired_timestamp
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13
            )",
        )
        .bind(stack.owner)
        .bind(stack.stack_small_id)
        .bind(stack.stack_id)
        .bind(stack.task_small_id)
        .bind(stack.selected_node_id)
        .bind(stack.num_compute_units)
        .bind(stack.price_per_one_million_compute_units)
        .bind(stack.already_computed_units)
        .bind(stack.locked_compute_units)
        .bind(stack.in_settle_period)
        .bind(stack.num_total_messages)
        .bind(user_id)
        .bind(acquired_timestamp)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Locks a stack
    ///
    /// This method locks a stack by setting the `is_locked` field to true for the specified `stack_small_id`.
    ///
    /// # Arguments
    ///
    /// * `stack_small_id` - The unique small identifier of the stack to lock.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn lock_stack(state_manager: &AtomaStateManager, stack_small_id: i64) -> Result<(), AtomaStateManagerError> {
    ///     state_manager.lock_stack(stack_small_id).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(%stack_small_id)
    )]
    pub async fn lock_stack(&self, stack_small_id: i64) -> Result<()> {
        sqlx::query("UPDATE stacks SET is_locked = true WHERE stack_small_id = $1")
            .bind(stack_small_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Updates the number of tokens already computed for a stack.
    ///
    /// This method updates the `already_computed_units` field in the `stacks` table
    /// for the specified `stack_small_id`.
    ///
    /// # Arguments
    ///
    /// * `stack_small_id` - The unique small identifier of the stack to update.
    /// * `estimated_total_tokens` - The estimated total number of tokens.
    /// * `total_tokens` - The total number of tokens.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn update_stack_num_tokens(state_manager: &AtomaStateManager, stack_small_id: i64, estimated_total_tokens: i64, total_tokens: i64) -> Result<(), AtomaStateManagerError> {
    ///     state_manager.update_stack_num_tokens(stack_small_id, estimated_total_tokens, total_tokens).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(%stack_small_id, %estimated_total_tokens, %total_tokens)
    )]
    pub async fn update_stack_num_tokens(
        &self,
        stack_small_id: i64,
        estimated_total_tokens: i64,
        total_tokens: i64,
    ) -> Result<()> {
        let result = sqlx::query(
            "UPDATE stacks
                SET already_computed_units = already_computed_units + $2,
                    locked_compute_units = locked_compute_units - $1,
                    num_total_messages = num_total_messages + $4
                WHERE stack_small_id = $3
           ",
        )
        .bind(estimated_total_tokens)
        .bind(total_tokens)
        .bind(stack_small_id)
        .bind(i64::from(total_tokens > 0)) // If total amount is greater than 0 then the request was successful
        .execute(&self.db)
        .await?;

        if result.rows_affected() == 0 {
            return Err(AtomaStateManagerError::StackNotFound);
        }

        Ok(())
    }

    /// Retrieves the stack settlement ticket for a given stack.
    ///
    /// This method fetches the settlement ticket details from the `stack_settlement_tickets` table
    /// based on the provided `stack_small_id`.
    ///
    /// # Arguments
    ///
    /// * `stack_small_id` - The unique small identifier of the stack whose settlement ticket is to be retrieved.
    ///
    /// # Returns
    ///
    /// - `Result<StackSettlementTicket>`: A result containing either:
    ///   - `Ok(StackSettlementTicket)`: A `StackSettlementTicket` object representing the stack settlement ticket.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue converting the database row into a `StackSettlementTicket` object.
    /// - No settlement ticket is found for the given `stack_small_id`.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, StackSettlementTicket};
    ///
    /// async fn get_settlement_ticket(state_manager: &AtomaStateManager, stack_small_id: i64) -> Result<StackSettlementTicket, AtomaStateManagerError> {
    ///     state_manager.get_stack_settlement_ticket(stack_small_id).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(%stack_small_id)
    )]
    pub async fn get_stack_settlement_ticket(
        &self,
        stack_small_id: i64,
    ) -> Result<StackSettlementTicket> {
        let stack_settlement_ticket =
            sqlx::query("SELECT * FROM stack_settlement_tickets WHERE stack_small_id = $1")
                .bind(stack_small_id)
                .fetch_one(&self.db)
                .await?;
        Ok(StackSettlementTicket::from_row(&stack_settlement_ticket)?)
    }

    /// Retrieves multiple stack settlement tickets from the database.
    ///
    /// This method fetches multiple settlement tickets from the `stack_settlement_tickets` table
    /// based on the provided `stack_small_ids`.
    ///
    /// # Arguments
    ///
    /// * `stack_small_ids` - A slice of stack IDs whose settlement tickets should be retrieved.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<StackSettlementTicket>>`: A result containing a vector of `StackSettlementTicket` objects.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, StackSettlementTicket};
    ///
    /// async fn get_settlement_tickets(state_manager: &AtomaStateManager, stack_small_ids: &[i64]) -> Result<Vec<StackSettlementTicket>, AtomaStateManagerError> {
    ///     state_manager.get_stack_settlement_tickets(stack_small_ids).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(?stack_small_ids)
    )]
    pub async fn get_stack_settlement_tickets(
        &self,
        stack_small_ids: &[i64],
    ) -> Result<Vec<StackSettlementTicket>> {
        let mut query_builder = build_query_with_in(
            "SELECT * FROM stack_settlement_tickets",
            "stack_small_id",
            stack_small_ids,
            None,
        );

        let stack_settlement_tickets = query_builder.build().fetch_all(&self.db).await?;

        stack_settlement_tickets
            .into_iter()
            .map(|row| StackSettlementTicket::from_row(&row).map_err(AtomaStateManagerError::from))
            .collect()
    }

    /// Inserts a new stack settlement ticket into the database.
    ///
    /// This method inserts a new entry into the `stack_settlement_tickets` table with the provided stack settlement ticket details.
    ///
    /// # Arguments
    ///
    /// * `stack_settlement_ticket` - The `StackSettlementTicket` object to be inserted into the database.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue converting the provided `StackSettlementTicket` object into a database row.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, StackSettlementTicket};
    ///
    /// async fn insert_settlement_ticket(state_manager: &AtomaStateManager, stack_settlement_ticket: StackSettlementTicket) -> Result<(), AtomaStateManagerError> {
    ///     state_manager.insert_new_stack_settlement_ticket(stack_settlement_ticket).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(stack_small_id = %stack_settlement_ticket.stack_small_id,
            selected_node_id = %stack_settlement_ticket.selected_node_id,
            num_claimed_compute_units = %stack_settlement_ticket.num_claimed_compute_units,
            requested_attestation_nodes = %stack_settlement_ticket.requested_attestation_nodes)
    )]
    pub async fn insert_new_stack_settlement_ticket(
        &self,
        stack_settlement_ticket: StackSettlementTicket,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let mut tx = self.db.begin().await?;
        sqlx::query(
            "INSERT INTO stack_settlement_tickets
                (
                    stack_small_id,
                    selected_node_id,
                    num_claimed_compute_units,
                    requested_attestation_nodes,
                    committed_stack_proofs,
                    stack_merkle_leaves,
                    dispute_settled_at_epoch,
                    already_attested_nodes,
                    is_in_dispute,
                    user_refund_amount,
                    is_claimed)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(stack_settlement_ticket.stack_small_id)
        .bind(stack_settlement_ticket.selected_node_id)
        .bind(stack_settlement_ticket.num_claimed_compute_units)
        .bind(stack_settlement_ticket.requested_attestation_nodes)
        .bind(stack_settlement_ticket.committed_stack_proofs)
        .bind(stack_settlement_ticket.stack_merkle_leaves)
        .bind(stack_settlement_ticket.dispute_settled_at_epoch)
        .bind(stack_settlement_ticket.already_attested_nodes)
        .bind(stack_settlement_ticket.is_in_dispute)
        .bind(stack_settlement_ticket.user_refund_amount)
        .bind(stack_settlement_ticket.is_claimed)
        .execute(&mut *tx)
        .await?;

        let timestamp = timestamp
            .with_second(0)
            .and_then(|t| t.with_minute(0))
            .and_then(|t| t.with_nanosecond(0))
            .ok_or(AtomaStateManagerError::InvalidTimestamp)?;
        // Also update the stack to set in_settle_period to true
        sqlx::query(
            "INSERT into stats_stacks (timestamp,settled_num_compute_units) VALUES ($1,$2)
             ON CONFLICT (timestamp)
             DO UPDATE SET
                settled_num_compute_units = stats_stacks.settled_num_compute_units + EXCLUDED.settled_num_compute_units"
        )
        .bind(timestamp)
        .bind(stack_settlement_ticket.num_claimed_compute_units)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Updates a stack settlement ticket with attestation commitments.
    ///
    /// This method updates the `stack_settlement_tickets` table with new attestation information
    /// for a specific stack. It updates the committed stack proof, stack Merkle leaf, and adds
    /// a new attestation node to the list of already attested nodes.
    ///
    /// # Arguments
    ///
    /// * `stack_small_id` - The unique small identifier of the stack to update.
    /// * `committed_stack_proof` - The new committed stack proof as a byte vector.
    /// * `stack_merkle_leaf` - The new stack Merkle leaf as a byte vector.
    /// * `attestation_node_id` - The ID of the node providing the attestation.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - The specified stack settlement ticket doesn't exist.
    /// - The attestation node is not found in the list of requested attestation nodes.
    /// - The provided Merkle leaf has an invalid length.
    /// - There's an issue updating the JSON array of already attested nodes.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn update_settlement_ticket(state_manager: &AtomaStateManager) -> Result<(), AtomaStateManagerError> {
    ///     let stack_small_id = 1;
    ///     let committed_stack_proof = vec![1, 2, 3, 4];
    ///     let stack_merkle_leaf = vec![5, 6, 7, 8];
    ///     let attestation_node_id = 42;
    ///
    ///     state_manager.update_stack_settlement_ticket_with_attestation_commitments(
    ///         stack_small_id,
    ///         committed_stack_proof,
    ///         stack_merkle_leaf,
    ///         attestation_node_id
    ///     ).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%stack_small_id, %attestation_node_id))]
    pub async fn update_stack_settlement_ticket_with_attestation_commitments(
        &self,
        stack_small_id: i64,
        committed_stack_proof: Vec<u8>,
        stack_merkle_leaf: Vec<u8>,
        attestation_node_id: i64,
    ) -> Result<()> {
        let mut tx = self.db.begin().await?;

        let row = sqlx::query(
            "SELECT committed_stack_proofs, stack_merkle_leaves, requested_attestation_nodes
             FROM stack_settlement_tickets
             WHERE stack_small_id = $1",
        )
        .bind(stack_small_id)
        .fetch_one(&mut *tx)
        .await?;

        let mut committed_stack_proofs: Vec<u8> = row.get("committed_stack_proofs");
        let mut current_merkle_leaves: Vec<u8> = row.get("stack_merkle_leaves");
        let requested_nodes: String = row.get("requested_attestation_nodes");
        let requested_nodes: Vec<i64> = serde_json::from_str(&requested_nodes)?;

        // Find the index of the attestation_node_id
        let index = requested_nodes
            .iter()
            .position(|&id| id == attestation_node_id)
            .ok_or_else(|| AtomaStateManagerError::AttestationNodeNotFound(attestation_node_id))?;

        // Update the corresponding 32-byte range in the stack_merkle_leaves
        let start = (index + 1) * 32;
        let end = start + 32;
        if end > current_merkle_leaves.len() {
            return Err(AtomaStateManagerError::InvalidMerkleLeafLength);
        }
        if end > committed_stack_proofs.len() {
            return Err(AtomaStateManagerError::InvalidCommittedStackProofLength);
        }

        current_merkle_leaves[start..end].copy_from_slice(&stack_merkle_leaf[..32]);
        committed_stack_proofs[start..end].copy_from_slice(&committed_stack_proof[..32]);

        sqlx::query(
            "UPDATE stack_settlement_tickets
             SET committed_stack_proofs = $1,
                 stack_merkle_leaves = $2,
                 already_attested_nodes = CASE
                     WHEN already_attested_nodes IS NULL THEN json_array($3)
                     ELSE json_insert(already_attested_nodes, '$[#]', $3)
                 END
             WHERE stack_small_id = $4",
        )
        .bind(committed_stack_proofs)
        .bind(current_merkle_leaves)
        .bind(attestation_node_id)
        .bind(stack_small_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Settles a stack settlement ticket by updating the dispute settled at epoch.
    ///
    /// This method updates the `stack_settlement_tickets` table, setting the `dispute_settled_at_epoch` field.
    ///
    /// # Arguments
    ///
    /// * `stack_small_id` - The unique small identifier of the stack settlement ticket to update.
    /// * `dispute_settled_at_epoch` - The epoch at which the dispute was settled.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - The specified stack settlement ticket doesn't exist.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn settle_ticket(state_manager: &AtomaStateManager) -> Result<(), AtomaStateManagerError> {
    ///     let stack_small_id = 1;
    ///     let dispute_settled_at_epoch = 10;
    ///
    ///     state_manager.settle_stack_settlement_ticket(stack_small_id, dispute_settled_at_epoch).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%stack_small_id, %dispute_settled_at_epoch))]
    pub async fn settle_stack_settlement_ticket(
        &self,
        stack_small_id: i64,
        dispute_settled_at_epoch: i64,
    ) -> Result<()> {
        sqlx::query("UPDATE stack_settlement_tickets SET dispute_settled_at_epoch = $1 WHERE stack_small_id = $2")
            .bind(dispute_settled_at_epoch)
            .bind(stack_small_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Updates a stack settlement ticket to mark it as claimed and set the user refund amount.
    ///
    /// This method updates the `stack_settlement_tickets` table, setting the `is_claimed` flag to true
    /// and updating the `user_refund_amount` for a specific stack settlement ticket.
    ///
    /// # Arguments
    ///
    /// * `stack_small_id` - The unique small identifier of the stack settlement ticket to update.
    /// * `user_refund_amount` - The amount to be refunded to the user.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - The specified stack settlement ticket doesn't exist.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn claim_settlement_ticket(state_manager: &AtomaStateManager) -> Result<(), AtomaStateManagerError> {
    ///     let stack_small_id = 1;
    ///     let user_refund_amount = 1000;
    ///
    ///     state_manager.update_stack_settlement_ticket_with_claim(stack_small_id, user_refund_amount).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%stack_small_id, %user_refund_amount))]
    pub async fn update_stack_settlement_ticket_with_claim(
        &self,
        stack_small_id: i64,
        user_refund_amount: i64,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE stack_settlement_tickets
                SET user_refund_amount = $1,
                    is_claimed = true
                WHERE stack_small_id = $2",
        )
        .bind(user_refund_amount)
        .bind(stack_small_id)
        .execute(&self.db)
        .await?;

        Ok(())
    }

    /// Retrieves all stacks that have been claimed for the specified node IDs.
    ///
    /// This method fetches all stack records from the `stacks` table where the `selected_node_id`
    /// matches any of the provided node IDs and the stack is marked as claimed (`is_claimed = true`).
    ///
    /// # Arguments
    ///
    /// * `node_small_ids` - A slice of node IDs to fetch claimed stacks for.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<Stack>>`: A result containing either:
    ///   - `Ok(Vec<Stack>)`: A vector of `Stack` objects representing all claimed stacks found.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue converting the database rows into `Stack` objects.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, Stack};
    ///
    /// async fn get_claimed_stacks(state_manager: &AtomaStateManager) -> Result<Vec<Stack>, AtomaStateManagerError> {
    ///     let node_ids = &[1, 2, 3]; // IDs of nodes to fetch claimed stacks for
    ///     state_manager.get_claimed_stacks(node_ids).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(?node_small_ids))]
    pub async fn get_claimed_stacks(
        &self,
        node_small_ids: &[i64],
    ) -> Result<Vec<StackSettlementTicket>> {
        let mut query_builder = build_query_with_in(
            "SELECT * FROM stack_settlement_tickets",
            "selected_node_id",
            node_small_ids,
            Some("is_claimed = true"),
        );

        let stacks = query_builder.build().fetch_all(&self.db).await?;

        stacks
            .into_iter()
            .map(|row| StackSettlementTicket::from_row(&row).map_err(AtomaStateManagerError::from))
            .collect()
    }

    /// Retrieves all stack attestation disputes for a given stack and attestation node.
    ///
    /// This method fetches all disputes from the `stack_attestation_disputes` table
    /// that match the provided `stack_small_id` and `attestation_node_id`.
    ///
    /// # Arguments
    ///
    /// * `stack_small_id` - The unique small identifier of the stack.
    /// * `attestation_node_id` - The ID of the attestation node.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<StackAttestationDispute>>`: A result containing either:
    ///   - `Ok(Vec<StackAttestationDispute>)`: A vector of `StackAttestationDispute` objects representing all disputes found.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue converting the database rows into `StackAttestationDispute` objects.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, StackAttestationDispute};
    ///
    /// async fn get_disputes(state_manager: &AtomaStateManager) -> Result<Vec<StackAttestationDispute>, AtomaStateManagerError> {
    ///     let stack_small_id = 1;
    ///     let attestation_node_id = 42;
    ///     state_manager.get_stack_attestation_disputes(stack_small_id, attestation_node_id).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%stack_small_id, %attestation_node_id))]
    pub async fn get_stack_attestation_disputes(
        &self,
        stack_small_id: i64,
        attestation_node_id: i64,
    ) -> Result<Vec<StackAttestationDispute>> {
        let disputes = sqlx::query(
            "SELECT * FROM stack_attestation_disputes
                WHERE stack_small_id = $1 AND attestation_node_id = $2",
        )
        .bind(stack_small_id)
        .bind(attestation_node_id)
        .fetch_all(&self.db)
        .await?;

        disputes
            .into_iter()
            .map(|row| {
                StackAttestationDispute::from_row(&row).map_err(AtomaStateManagerError::from)
            })
            .collect()
    }

    /// Retrieves all attestation disputes filed against the specified nodes.
    ///
    /// This method fetches all disputes from the `stack_attestation_disputes` table where the
    /// specified nodes are the original nodes being disputed against (i.e., where they are
    /// listed as `original_node_id`).
    ///
    /// # Arguments
    ///
    /// * `node_small_ids` - A slice of node IDs to check for disputes against.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<StackAttestationDispute>>`: A result containing either:
    ///   - `Ok(Vec<StackAttestationDispute>)`: A vector of all disputes found against the specified nodes.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue converting the database rows into `StackAttestationDispute` objects.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, StackAttestationDispute};
    ///
    /// async fn get_disputes(state_manager: &AtomaStateManager) -> Result<Vec<StackAttestationDispute>, AtomaStateManagerError> {
    ///     let node_ids = &[1, 2, 3]; // IDs of nodes to check for disputes against
    ///     state_manager.get_against_attestation_disputes(node_ids).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(?node_small_ids))]
    pub async fn get_against_attestation_disputes(
        &self,
        node_small_ids: &[i64],
    ) -> Result<Vec<StackAttestationDispute>> {
        let mut query_builder = build_query_with_in(
            "SELECT * FROM stack_attestation_disputes",
            "original_node_id",
            node_small_ids,
            None,
        );

        let disputes = query_builder.build().fetch_all(&self.db).await?;

        disputes
            .into_iter()
            .map(|row| {
                StackAttestationDispute::from_row(&row).map_err(AtomaStateManagerError::from)
            })
            .collect()
    }

    /// Retrieves all attestation disputes where the specified nodes are the attestation providers.
    ///
    /// This method fetches all disputes from the `stack_attestation_disputes` table where the
    /// specified nodes are the attestation providers (i.e., where they are listed as `attestation_node_id`).
    ///
    /// # Arguments
    ///
    /// * `node_small_ids` - A slice of node IDs to check for disputes where they are attestation providers.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<StackAttestationDispute>>`: A result containing either:
    ///   - `Ok(Vec<StackAttestationDispute>)`: A vector of all disputes where the specified nodes are attestation providers.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails or if there's an issue parsing the results.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue converting the database rows into `StackAttestationDispute` objects.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, StackAttestationDispute};
    ///
    /// async fn get_disputes(state_manager: &AtomaStateManager) -> Result<Vec<StackAttestationDispute>, AtomaStateManagerError> {
    ///     let node_ids = &[1, 2, 3]; // IDs of nodes to check for disputes as attestation providers
    ///     state_manager.get_own_attestation_disputes(node_ids).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(?node_small_ids))]
    pub async fn get_own_attestation_disputes(
        &self,
        node_small_ids: &[i64],
    ) -> Result<Vec<StackAttestationDispute>> {
        let mut query_builder = build_query_with_in(
            "SELECT * FROM stack_attestation_disputes",
            "attestation_node_id",
            node_small_ids,
            None,
        );

        let disputes = query_builder.build().fetch_all(&self.db).await?;

        disputes
            .into_iter()
            .map(|row| {
                StackAttestationDispute::from_row(&row).map_err(AtomaStateManagerError::from)
            })
            .collect()
    }

    /// Inserts a new stack attestation dispute into the database.
    ///
    /// This method adds a new entry to the `stack_attestation_disputes` table with the provided dispute information.
    ///
    /// # Arguments
    ///
    /// * `stack_attestation_dispute` - A `StackAttestationDispute` struct containing all the information about the dispute to be inserted.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's a constraint violation (e.g., duplicate primary key).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, StackAttestationDispute};
    ///
    /// async fn add_dispute(state_manager: &AtomaStateManager, dispute: StackAttestationDispute) -> Result<(), AtomaStateManagerError> {
    ///     state_manager.insert_stack_attestation_dispute(dispute).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(stack_small_id = %stack_attestation_dispute.stack_small_id,
            attestation_node_id = %stack_attestation_dispute.attestation_node_id,
            original_node_id = %stack_attestation_dispute.original_node_id)
    )]
    pub async fn insert_stack_attestation_dispute(
        &self,
        stack_attestation_dispute: StackAttestationDispute,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO stack_attestation_disputes
                (stack_small_id, attestation_commitment, attestation_node_id, original_node_id, original_commitment)
                VALUES ($1, $2, $3, $4, $5)",
        )
            .bind(stack_attestation_dispute.stack_small_id)
            .bind(stack_attestation_dispute.attestation_commitment)
            .bind(stack_attestation_dispute.attestation_node_id)
            .bind(stack_attestation_dispute.original_node_id)
            .bind(stack_attestation_dispute.original_commitment)
            .execute(&self.db)
            .await?;

        Ok(())
    }

    /// Stores the public address of a node in the database.
    ///
    /// This method updates the public address of a node in the `nodes` table.
    ///
    /// # Arguments
    ///
    /// * `small_id` - The unique small identifier of the node.
    /// * `address` - The public address of the node.
    /// * `country` - The country of the node.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn update_address(state_manager: &AtomaStateManager, small_id: i64, address: String, country:String) -> Result<(), AtomaStateManagerError> {
    ///    state_manager.update_node_public_address(small_id, address, country).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%small_id, %address))]
    pub async fn update_node_public_address(
        &self,
        small_id: i64,
        address: String,
        country: String,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE nodes
             SET public_address = $2, country = $3
             WHERE node_small_id = $1
             AND (public_address IS DISTINCT FROM $2 OR country IS DISTINCT FROM $3)",
        )
        .bind(small_id)
        .bind(address)
        .bind(country)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Retrieves all node public addresses from the database.
    ///
    /// This method fetches all public addresses of nodes from the `nodes` table.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<String>>`: A result containing either:
    ///   - `Ok(Vec<String>)`: A vector of all node public addresses.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    #[instrument(level = "trace", skip_all)]
    pub async fn retrieve_node_public_addresses(&self) -> Result<Vec<String>> {
        let addresses = sqlx::query_scalar(
            "SELECT public_address
             FROM nodes
             WHERE public_address IS NOT NULL",
        )
        .fetch_all(&self.db)
        .await?;
        Ok(addresses)
    }

    /// Retrieves the public address of a node from the database.
    ///
    /// This method fetches the public address of a node from the `nodes` table.
    ///
    /// # Arguments
    ///
    /// * `small_id` - The unique small identifier of the node.
    ///
    /// # Returns
    ///
    /// - `Result<Option<String>>`: A result containing either:
    ///   - `Ok(Some(String))`: The public address of the node.
    ///   - `Ok(None)`: If the node is not found.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_address(state_manager: &AtomaStateManager, small_id: i64) -> Result<Option<String>, AtomaStateManagerError> {
    ///    state_manager.get_node_public_address(small_id).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%small_id))]
    pub async fn get_node_public_address(&self, small_id: i64) -> Result<Option<String>> {
        let address =
            sqlx::query_scalar("SELECT public_address FROM nodes WHERE node_small_id = $1")
                .bind(small_id)
                .fetch_optional(&self.db)
                .await?;
        Ok(address)
    }

    /// Retrieves the public address and small ID of the node associated with a specific stack.
    ///
    /// This method performs a JOIN between the `stacks` and `nodes` tables to fetch the node's
    /// public address and small ID based on the stack's selected node.
    ///
    /// # Arguments
    ///
    /// * `stack_small_id` - The unique small identifier of the stack.
    ///
    /// # Returns
    ///
    /// - `Result<(Option<String>, i64)>`: A result containing either:
    ///   - `Ok((Option<String>, i64))`: A tuple containing:
    ///     - The node's public address (if set) or None
    ///     - The node's small ID
    ///   - `Err(AtomaStateManagerError)`: An error if:
    ///     - The database query fails
    ///     - The specified stack is not found
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute
    /// - The specified stack does not exist (`AtomaStateManagerError::StackNotFound`)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_node_info(state_manager: &AtomaStateManager) -> Result<(), AtomaStateManagerError> {
    ///     let stack_small_id = 1;
    ///     let (public_address, node_small_id) = state_manager
    ///         .get_node_public_url_and_small_id(stack_small_id)
    ///         .await?;
    ///
    ///     println!("Node {} has public address: {:?}", node_small_id, public_address);
    ///     Ok(())
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%stack_small_id))]
    pub async fn get_node_public_url_and_small_id(
        &self,
        stack_small_id: i64,
    ) -> Result<(Option<String>, i64)> {
        let result = sqlx::query_as::<_, (Option<String>, i64)>(
            "SELECT n.public_address, n.node_small_id
             FROM stacks s
             JOIN nodes n ON s.selected_node_id = n.node_small_id
             WHERE s.stack_small_id = $1",
        )
        .bind(stack_small_id)
        .fetch_optional(&self.db)
        .await?
        .ok_or(AtomaStateManagerError::StackNotFound)?;

        Ok(result)
    }

    /// Retrieves the sui address of a node from the database.
    ///
    /// This method fetches the sui address of a node from the `nodes` table.
    ///
    /// # Arguments
    ///
    /// * `small_id` - The unique small identifier of the node.
    ///
    /// # Returns
    ///
    /// - `Result<Option<String>>`: A result containing either:
    ///   - `Ok(Some(String))`: The sui address of the node.
    ///   - `Ok(None)`: If the node does not have a sui address.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_address(state_manager: &AtomaStateManager, small_id: i64) -> Result<Option<String>, AtomaStateManagerError> {
    ///    state_manager.get_node_sui_address(small_id).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%small_id))]
    pub async fn get_node_sui_address(&self, small_id: i64) -> Result<Option<String>> {
        let address = sqlx::query_scalar("SELECT sui_address FROM nodes WHERE node_small_id = $1")
            .bind(small_id)
            .fetch_optional(&self.db)
            .await?;
        Ok(address)
    }

    /// Records or updates a key rotation event for a specific epoch in the database.
    ///
    /// This method tracks cryptographic key rotation events by maintaining a counter for each epoch.
    /// It uses an "upsert" operation, meaning it will either:
    /// - Insert a new record if no rotation exists for the given epoch
    /// - Update the existing record if a rotation was already recorded for that epoch
    ///
    /// # Arguments
    ///
    /// * `epoch` - The epoch number when the key rotation occurred
    /// * `key_rotation_counter` - The cumulative number of key rotations that have occurred
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError))
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute
    /// - There's a connection issue with the database
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn rotate_keys(state_manager: &AtomaStateManager) -> Result<(), AtomaStateManagerError> {
    ///     let current_epoch = 100;
    ///     let rotation_counter = 5;
    ///
    ///     state_manager.insert_new_key_rotation(current_epoch, rotation_counter).await
    /// }
    /// ```
    ///
    /// # Security Considerations
    ///
    /// This method is part of the system's cryptographic key management infrastructure and helps:
    /// - Maintain an audit trail of key rotation events
    /// - Verify compliance with key rotation policies
    /// - Track the frequency of key rotations per epoch
    #[instrument(level = "trace", skip_all, fields(%epoch, %key_rotation_counter))]
    pub async fn insert_new_key_rotation(
        &self,
        epoch: i64,
        key_rotation_counter: i64,
        nonce: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO key_rotations (epoch, key_rotation_counter, nonce) VALUES ($1, $2, $3)
            ON CONFLICT (epoch)
            DO UPDATE SET key_rotation_counter = EXCLUDED.key_rotation_counter,
                          nonce = EXCLUDED.nonce",
        )
        .bind(epoch)
        .bind(key_rotation_counter)
        .bind(nonce)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Retrieves the nonce for the latest key rotation from the `key_rotations` table.
    ///
    /// This method queries the `key_rotations` table to get the nonce value for the most recent key rotation.
    ///
    /// # Returns
    ///
    /// - `Result<i64>`: A result containing the nonce value for the most recent key rotation.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute
    /// - There's a connection issue with the database
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_nonce(state_manager: &AtomaStateManager) -> Result<i64> {
    ///     let nonce = state_manager.get_contract_key_rotation_nonce().await?;
    ///     Ok(nonce)
    /// }
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn get_contract_key_rotation_nonce(&self) -> Result<i64> {
        let nonce =
            sqlx::query_scalar("SELECT nonce FROM key_rotations ORDER BY epoch DESC LIMIT 1")
                .fetch_one(&self.db)
                .await?;
        Ok(nonce)
    }

    /// Updates or inserts a node's public key and associated information in the database.
    ///
    /// This method updates the `node_public_keys` table with new information for a specific node. If an entry
    /// for the node already exists, it updates the existing record. Otherwise, it creates a new record.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The unique small identifier of the node.
    /// * `epoch` - The current epoch number when the update occurs.
    /// * `node_badge_id` - The badge identifier associated with the node.
    /// * `new_public_key` - The new public key to be stored for the node.
    /// * `evidence_bytes` - The TEE evidence bytes (containing both Nvidia GPU and NvSwitch remote attestations and certificate chains).
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (`Ok(())`) or failure (`Err(AtomaStateManagerError)`).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute
    /// - There's a connection issue with the database
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn update_key(state_manager: &AtomaStateManager) -> Result<(), AtomaStateManagerError> {
    ///     let node_id = 1;
    ///     let epoch = 100;
    ///     let node_badge_id = "badge123".to_string();
    ///     let new_public_key = "pk_abc123...".to_string();
    ///     let attestation_bytes = vec![1, 2, 3, 4];
    ///
    ///     state_manager.update_node_public_key(
    ///         node_id,
    ///         epoch,
    ///         node_badge_id,
    ///         new_public_key,
    ///         attestation_bytes
    ///     ).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%node_id, %epoch))]
    #[allow(clippy::too_many_arguments)]
    pub async fn update_node_public_key(
        &self,
        node_id: i64,
        epoch: i64,
        key_rotation_counter: i64,
        new_public_key: Vec<u8>,
        evidence_bytes: Vec<u8>,
        device_type: i64,
        is_valid: bool,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO node_public_keys (
                node_small_id,
                epoch,
                key_rotation_counter,
                public_key,
                evidence_bytes,
                device_type,
                is_valid
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (node_small_id, device_type)
            DO UPDATE SET
                epoch = EXCLUDED.epoch,
                key_rotation_counter = EXCLUDED.key_rotation_counter,
                public_key = EXCLUDED.public_key,
                evidence_bytes = EXCLUDED.evidence_bytes,
                is_valid = EXCLUDED.is_valid",
        )
        .bind(node_id)
        .bind(epoch)
        .bind(key_rotation_counter)
        .bind(new_public_key)
        .bind(evidence_bytes)
        .bind(device_type)
        .bind(is_valid)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Insert new node into the database.
    ///
    /// This method inserts a new node into the `nodes` table.
    ///
    /// # Arguments
    ///
    /// * `node_small_id` - The unique small identifier of the node.
    /// * `sui_address` - The sui address of the node.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if the database query fails.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn insert_node(state_manager: &AtomaStateManager) -> Result<(), AtomaStateManagerError> {
    ///     let node_small_id = 1;
    ///     let node_id = "node123".to_string();
    ///     let sui_address = "0x1234567890".to_string();
    ///
    ///     state_manager.insert_new_node(node_small_id, node_id, sui_address).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn insert_new_node(
        &self,
        node_small_id: i64,
        node_id: String,
        sui_address: String,
    ) -> Result<()> {
        sqlx::query("INSERT INTO nodes (node_small_id, node_id, sui_address) VALUES ($1, $2, $3)")
            .bind(node_small_id)
            .bind(node_id)
            .bind(sui_address)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Registers or updates a node's public URL in the database.
    ///
    /// This method updates the `nodes` table with a node's public URL and timestamp. Since nodes must first
    /// register on the network before registering their public URL, this method implements a retry mechanism
    /// to handle race conditions where the node registration event hasn't been processed yet.
    ///
    /// # Arguments
    ///
    /// * `node_small_id` - The unique small identifier of the node.
    /// * `public_url` - The public URL where the node can be reached.
    /// * `timestamp` - The Unix timestamp when this registration/update occurred.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (`Ok(())`) or failure (`Err(AtomaStateManagerError)`).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - After 3 retries, the node is still not found in the database (`AtomaStateManagerError::NodeNotFound`).
    ///
    /// # Retries
    ///
    /// The method will retry the update operation up to 3 times with a 500ms delay between attempts if the
    /// node is not found. This helps handle race conditions where the node registration event hasn't been
    /// processed yet.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn register_url(state_manager: &AtomaStateManager) -> Result<(), AtomaStateManagerError> {
    ///     let node_small_id = 1;
    ///     let public_url = "https://node1.example.com".to_string();
    ///     let timestamp = chrono::Utc::now().timestamp();
    ///
    ///     state_manager.register_node_public_url(node_small_id, public_url, timestamp).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn register_node_public_url(
        &self,
        node_small_id: i64,
        public_url: String,
        timestamp: i64,
        country: String,
    ) -> Result<()> {
        // NOTE: We expect that the `nodes` already contains the entry for the current `node_small_id`
        // as each is supposed to first register on the network and only then register the public url,
        // with its available registration metadata (i.e. the `node_small_id`). The only exception
        // is the case when the node registration event was not received yet. In this case, we should
        // retry after some short delay.
        //
        // It might the (likely malicious) case that a node is publishing an url with a previous timestamp
        // to overwrite the previous url. In this case, all the retries will fail, but this is probably
        // fine.
        const NUM_RETRIES: u32 = 3;
        const BASE_DELAY: Duration = Duration::from_millis(100);
        const MAX_DELAY: Duration = Duration::from_millis(500);

        validation::validate_timestamp(timestamp)?;
        validation::validate_country(&country)?;
        validation::validate_url(&public_url)?;

        for retry in 0..NUM_RETRIES {
            let result = sqlx::query(
                "UPDATE nodes
                    SET public_address = $2,
                        timestamp = $3,
                        country = $4
                    WHERE node_small_id = $1
                    AND (timestamp IS NULL OR timestamp < $3)
                    AND (public_address IS DISTINCT FROM $2 OR country IS DISTINCT FROM $4)",
            )
            .bind(node_small_id)
            .bind(&public_url)
            .bind(timestamp)
            .bind(&country)
            .execute(&self.db)
            .await?;

            if result.rows_affected() > 0 {
                return Ok(());
            }

            if retry < NUM_RETRIES - 1 {
                // Calculate exponential backoff with jitter
                let backoff = BASE_DELAY * 2u32.pow(retry);
                let with_jitter = std::cmp::min(
                    MAX_DELAY,
                    backoff + Duration::from_millis(fastrand::u64(0..=100)),
                );
                tokio::time::sleep(with_jitter).await;
            }
        }

        Err(AtomaStateManagerError::NodeNotFound)
    }

    /// Verifies the ownership of a node's small ID by checking the Sui address.
    ///
    /// This method fetches the node's small ID from the database and checks if the provided Sui address
    /// matches the address stored in the database.
    ///
    /// # Arguments
    ///
    /// * `node_small_id` - The small ID of the node to verify.
    /// * `sui_address` - The Sui address to verify against the node's small ID.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if the database query fails to execute.
    #[tracing::instrument(
        level = "trace",
        skip_all,
        fields(node_small_id = %node_small_id, sui_address = %sui_address)
    )]
    pub async fn verify_node_small_id_ownership(
        &self,
        node_small_id: i64,
        sui_address: String,
    ) -> Result<()> {
        let exists = sqlx::query(
            "SELECT EXISTS(SELECT 1 FROM nodes WHERE node_small_id = $1 AND public_address = $2)",
        )
        .bind(node_small_id)
        .bind(sui_address)
        .fetch_one(&self.db)
        .await?
        .get::<bool, _>(0);

        if !exists {
            return Err(AtomaStateManagerError::NodeSmallIdOwnershipVerificationFailed);
        }

        Ok(())
    }

    /// Get the user_id by email and password.
    ///
    /// This method queries the `users` table to get the user_id by email and password.
    ///
    /// # Arguments
    ///
    /// * `email` - The email of the user.
    /// * `hashed_password` - The hashed password of the user.
    ///
    /// # Returns
    ///
    /// - `Result<Option<i64>>`: A result containing either:
    ///   - `Ok(Some(i64))`: The user_id of the user.
    ///   - `Ok(None)`: If the user is not found.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_user_id(state_manager: &AtomaStateManager, email: &str, hashed_password: &str) -> Result<Option<i64>, AtomaStateManagerError> {
    ///    state_manager.get_user_id_by_email_password(email, hashed_password).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn get_user_id_by_email_password(
        &self,
        email: &str,
        hashed_password: &str,
    ) -> Result<Option<i64>> {
        let user = sqlx::query("SELECT id FROM users WHERE email = $1 AND password_hash = $2")
            .bind(email)
            .bind(hashed_password)
            .fetch_optional(&self.db)
            .await?;

        Ok(user.map(|user| user.get("id")))
    }

    /// Get the id of the user by email (register if not in the table yet).
    ///
    /// This method queries the `users` table to get the user_id by email. If the user is not found, it will insert the user into the table.
    ///
    /// # Arguments
    ///
    /// * `email` - The email of the user.
    ///
    /// # Returns
    ///
    /// - `Result<i64>`: A result containing either:
    ///   - `Ok(i64)`: The user_id of the user.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn oauth(state_manager: &AtomaStateManager, email: &str) -> Result<i64> {
    ///    state_manager.oauth(email).await
    /// }
    /// ```
    pub async fn oauth(&self, email: &str, password_salt: &str) -> Result<i64> {
        let user = sqlx::query("INSERT INTO users (email, password_salt) VALUES ($1, $2) ON CONFLICT (email) DO UPDATE SET email = EXCLUDED.email RETURNING id")
                .bind(email)
                .bind(password_salt)
                .fetch_one(&self.db).await?;

        Ok(user.get("id"))
    }

    /// Check if the refresh_token_hash is valid for the user.
    ///
    /// This method checks if the refresh token hash is valid for the user by querying the `refresh_tokens` table.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    /// * `refresh_token_hash` - The refresh token hash to check.
    ///
    /// # Returns
    ///
    /// - `Result<bool>`: A result indicating whether the refresh token is valid for the user.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn is_token_valid(state_manager: &AtomaStateManager, user_id: i64, refresh_token_hash: &str) -> Result<bool> {
    ///   state_manager.is_refresh_token_valid(user_id, refresh_token_hash).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn is_refresh_token_valid(
        &self,
        user_id: i64,
        refresh_token_hash: &str,
    ) -> Result<bool> {
        let is_valid = sqlx::query(
            "SELECT EXISTS(SELECT 1 FROM refresh_tokens WHERE user_id = $1 AND token_hash = $2)",
        )
        .bind(user_id)
        .bind(refresh_token_hash)
        .fetch_one(&self.db)
        .await?;

        Ok(is_valid.get::<bool, _>(0))
    }

    /// Stores refresh token hash for the user.
    ///
    /// This method inserts a new refresh token hash into the `refresh_tokens` table for the specified user.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    /// * `refresh_token` - The refresh token to store.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn store_token(state_manager: &AtomaStateManager, user_id: i64, refresh_token_hash: &str) -> Result<(), AtomaStateManagerError> {
    ///    state_manager.store_refresh_token(user_id, refresh_token_hash).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn store_refresh_token(&self, user_id: i64, refresh_token_hash: &str) -> Result<()> {
        sqlx::query("INSERT INTO refresh_tokens (user_id, token_hash) VALUES ($1, $2)")
            .bind(user_id)
            .bind(refresh_token_hash)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Delete a refresh token hash for a user.
    ///
    /// This method deletes a refresh token hash from the `refresh_tokens` table for the specified user.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    /// * `refresh_token_hash` - The refresh token hash to delete.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn delete_token(state_manager: &AtomaStateManager, user_id: i64, refresh_token_hash: &str) -> Result<(), AtomaStateManagerError> {
    ///   state_manager.delete_refresh_token(user_id, refresh_token_hash).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn delete_refresh_token(&self, user_id: i64, refresh_token_hash: &str) -> Result<()> {
        sqlx::query("DELETE FROM refresh_tokens WHERE user_id = $1 AND token_hash = $2")
            .bind(user_id)
            .bind(refresh_token_hash)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Delete a api_token for a user.
    ///
    /// This method deletes a api token from the `api_tokens` table for the specified user.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    /// * `api_token` - The api token to delete.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn delete_token(state_manager: &AtomaStateManager, user_id: i64, api_token: &str) -> Result<(), AtomaStateManagerError> {
    ///    state_manager.delete_api_token(user_id, api_token).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn delete_api_token(&self, user_id: i64, api_token_id: i64) -> Result<()> {
        sqlx::query("DELETE FROM api_tokens WHERE user_id = $1 AND id = $2")
            .bind(user_id)
            .bind(api_token_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Checks if the api token is valid for the user.
    ///
    /// This method checks if the api token is valid for the user by querying the `api_tokens` table.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    /// * `api_token` - The api token to check.
    ///
    /// # Returns
    ///
    /// - `Result<bool>`: A result indicating whether the api token is valid for the user.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn is_token_valid(state_manager: &AtomaStateManager, user_id: i64, api_token: &str) -> Result<bool> {
    ///    state_manager.is_api_token_valid(user_id, api_token).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn is_api_token_valid(&self, api_token: &str) -> Result<i64> {
        let is_valid = sqlx::query(
            "UPDATE api_tokens SET last_used_timestamp = now() WHERE token = $1 RETURNING user_id",
        )
        .bind(api_token)
        .fetch_one(&self.db)
        .await?;

        Ok(is_valid.get::<i64, _>(0))
    }

    /// Stores a new api token for a user.
    ///
    /// This method inserts a new api token into the `api_tokens` table for the specified user.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    /// * `api_token` - The api token to store.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn store_token(state_manager: &AtomaStateManager, user_id: i64, api_token: &str) -> Result<(), AtomaStateManagerError> {
    ///    state_manager.store_api_token(user_id, api_token).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn store_api_token(&self, user_id: i64, api_token: &str, name: &str) -> Result<()> {
        sqlx::query("INSERT INTO api_tokens (user_id, token, name) VALUES ($1, $2, $3)")
            .bind(user_id)
            .bind(api_token)
            .bind(name)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Retrieves all API tokens for a user.
    ///
    /// This method fetches all API tokens from the `api_tokens` table for the specified user.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<TokenResponse>>`: A result containing either:
    ///   - `Ok(Vec<TokenResponse>)`: A list of API tokens for the user.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails.
    ///
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_tokens(state_manager: &AtomaStateManager, user_id: i64) -> Result<Vec<String>, AtomaStateManagerError> {
    ///    state_manager.get_api_tokens_for_user(user_id).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn get_api_tokens_for_user(&self, user_id: i64) -> Result<Vec<TokenResponse>> {
        let tokens = sqlx::query(
            "SELECT id, RIGHT(token,4) as token_last_4, last_used_timestamp, creation_timestamp as created_at, name FROM api_tokens WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_all(&self.db)
        .await?;

        tokens
            .into_iter()
            .map(|token| TokenResponse::from_row(&token).map_err(AtomaStateManagerError::from))
            .collect()
    }

    /// Get compute units processed for the last `last_hours` hours.
    ///
    /// This method fetches the compute units processed for the last `last_hours` hours from the `stats_compute_units_processed` table.
    ///
    /// # Arguments
    ///
    /// * `last_hours` - The number of hours to fetch the compute units processed for.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<ComputedUnitsProcessedResponse>>`: A result containing either:
    ///   - `Ok(Vec<ComputedUnitsProcessedResponse>)`: The compute units processed for the last `last_hours` hours.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_compute_units_processed(state_manager: &AtomaStateManager, last_hours: usize) -> Result<Vec<ComputedUnitsProcessedResponse>, AtomaStateManagerError> {
    ///    state_manager.get_compute_units_processed(last_hours).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn get_compute_units_processed(
        &self,
        last_hours: usize,
    ) -> Result<Vec<ComputedUnitsProcessedResponse>> {
        let timestamp = Utc::now();
        let start_timestamp = timestamp
            .checked_sub_signed(chrono::Duration::hours(last_hours as i64))
            .ok_or(AtomaStateManagerError::InvalidTimestamp)?;
        let performances_per_hour = sqlx::query("SELECT timestamp, model_name, amount, requests, time FROM stats_compute_units_processed WHERE timestamp >= $1 ORDER BY timestamp ASC, model_name ASC")
                        .bind(start_timestamp)
                        .fetch_all(&self.db)
                        .await?;
        performances_per_hour
            .into_iter()
            .map(|performance| {
                ComputedUnitsProcessedResponse::from_row(&performance)
                    .map_err(AtomaStateManagerError::from)
            })
            .collect()
    }

    /// Get balance for a user.
    ///
    /// This method fetches the balance for a user from the `crypto_balances` table.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    ///
    /// # Returns
    ///
    /// - `Result<i64>`: A result containing either:
    ///   - `Ok(i64)`: The balance for the user.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    ///
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_crypto_balance(state_manager: &AtomaStateManager, user_id: i64) -> Result<i64, AtomaStateManagerError> {
    ///     state_manager.get_crypto_balance_for_user(user_id).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn get_crypto_balance_for_user(&self, user_id: i64) -> Result<i64> {
        let balance = sqlx::query("SELECT usdc_balance FROM crypto_balances WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(&self.db)
            .await?
            .map_or(0, |row| row.get::<i64, _>("usdc_balance"));

        Ok(balance)
    }

    /// Get the user profile by user_id.
    ///
    /// This method fetches the user profile by user_id from the `users` table.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    ///
    /// # Returns
    ///
    /// - `Result<UserProfile>`: A result containing either:
    ///   - `Ok(UserProfile)`: The user profile for the user_id.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_profile(state_manager: &AtomaStateManager, user_id: i64) -> Result<UserProfile, AtomaStateManagerError> {
    ///     state_manager.get_user_profile(user_id).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub async fn get_user_profile(&self, user_id: i64) -> Result<UserProfile> {
        let user = sqlx::query("SELECT email FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&self.db)
            .await?;

        UserProfile::from_row(&user).map_err(AtomaStateManagerError::from)
    }

    /// Get latency performance for the last `last_hours` hours.
    ///
    /// This method fetches the latency performance for the last `last_hours` hours from the `stats_latency` table.
    ///
    /// # Arguments
    ///
    /// * `last_hours` - The number of hours to fetch the latency performance for.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<LatencyResponse>>`: A result containing either:
    ///   - `Ok(Vec<LatencyResponse>)`: The latency performance for the last `last_hours` hours.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_latency_performance(state_manager: &AtomaStateManager, last_hours: usize) -> Result<Vec<LatencyResponse>, AtomaStateManagerError> {
    ///   state_manager.get_latency_performance(last_hours).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn get_latency_performance(&self, last_hours: usize) -> Result<Vec<LatencyResponse>> {
        let timestamp = Utc::now();
        let start_timestamp = timestamp
            .checked_sub_signed(chrono::Duration::hours(last_hours as i64))
            .ok_or(AtomaStateManagerError::InvalidTimestamp)?;
        let performances_per_hour = sqlx::query(
            "SELECT timestamp, latency, requests FROM stats_latency WHERE timestamp >= $1 ORDER BY timestamp ASC",
        )
        .bind(start_timestamp)
        .fetch_all(&self.db)
        .await?;
        performances_per_hour
            .into_iter()
            .map(|performance| {
                LatencyResponse::from_row(&performance).map_err(AtomaStateManagerError::from)
            })
            .collect()
    }

    /// Add compute units processed to the database.
    ///
    /// This method inserts the compute units processed into the `stats_compute_units_processed` table.
    ///
    /// # Arguments
    ///
    /// * `timestamp` - The timestamp of the data.
    /// * `compute_units_processed` - The number of compute units processed.
    /// * `time` - The time taken to process the compute units.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    /// use chrono::{DateTime, Utc};
    ///
    /// async fn add_compute_units_processed(state_manager: &AtomaStateManager, timestamp: DateTime<Utc>, compute_units_processed: i64, time: f64) -> Result<(), AtomaStateManagerError> {
    ///   state_manager.add_compute_units_processed(timestamp, compute_units_processed, time).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn add_compute_units_processed(
        &self,
        timestamp: DateTime<Utc>,
        model_name: String,
        compute_units_processed: i64,
        time: f64,
    ) -> Result<()> {
        // We want the table to gather data in hourly intervals
        let timestamp = timestamp
            .with_second(0)
            .and_then(|t| t.with_minute(0))
            .and_then(|t| t.with_nanosecond(0))
            .ok_or(AtomaStateManagerError::InvalidTimestamp)?;
        sqlx::query(
            "INSERT INTO stats_compute_units_processed (timestamp, model_name, amount, time) VALUES ($1, $2, $3, $4)
                 ON CONFLICT (timestamp, model_name) DO UPDATE SET
                    amount = stats_compute_units_processed.amount + EXCLUDED.amount,
                    time = stats_compute_units_processed.time + EXCLUDED.time,
                    requests = stats_compute_units_processed.requests + 1",
        )
        .bind(timestamp)
        .bind(model_name)
        .bind(compute_units_processed)
        .bind(time)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Add latency to the database.
    ///
    /// This method inserts the latency into the `stats_latency` table. This measure the time from the request to first generated token.
    ///
    /// # Arguments
    ///
    /// * `timestamp` - The timestamp of the data.
    /// * `latency` - The latency of the data.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    /// use chrono::{DateTime, Utc};
    ///
    /// async fn add_latency(state_manager: &AtomaStateManager, timestamp: DateTime<Utc>, latency: f64) -> Result<(), AtomaStateManagerError> {
    ///    state_manager.add_latency(timestamp, latency).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn add_latency(&self, timestamp: DateTime<Utc>, latency: f64) -> Result<()> {
        // We want the table to gather data in hourly intervals
        let timestamp = timestamp
            .with_second(0)
            .and_then(|t| t.with_minute(0))
            .and_then(|t| t.with_nanosecond(0))
            .ok_or(AtomaStateManagerError::InvalidTimestamp)?;
        sqlx::query(
            "INSERT INTO stats_latency (timestamp, latency) VALUES ($1, $2)
                 ON CONFLICT (timestamp) DO UPDATE SET
                    latency = stats_latency.latency + EXCLUDED.latency,
                    requests = stats_latency.requests + 1",
        )
        .bind(timestamp)
        .bind(latency)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Records statistics about a new stack in the database.
    ///
    /// This method inserts or updates hourly statistics about stack compute units in the `stats_stacks` table.
    /// The timestamp is rounded down to the nearest hour, and if an entry already exists for that hour,
    /// the compute units are added to the existing total.
    ///
    /// # Arguments
    ///
    /// * `stack` - The `Stack` object containing information about the new stack.
    /// * `timestamp` - The timestamp when the stack was created.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - The timestamp cannot be normalized to an hour boundary.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, Stack};
    /// use chrono::{DateTime, Utc};
    ///
    /// async fn record_stack_stats(state_manager: &AtomaStateManager, stack: Stack) -> Result<(), AtomaStateManagerError> {
    ///     let timestamp = Utc::now();
    ///     state_manager.new_stats_stack(stack, timestamp).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn new_stats_stack(&self, stack: Stack, timestamp: DateTime<Utc>) -> Result<()> {
        sqlx::query(
            "INSERT into stats_stacks (timestamp,num_compute_units) VALUES ($1,$2)
                 ON CONFLICT (timestamp)
                 DO UPDATE SET
                    num_compute_units = stats_stacks.num_compute_units + EXCLUDED.num_compute_units",
        )
        .bind(Utc::now())
        .bind(stack.num_compute_units)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Updates the sui address for the user.
    ///
    /// This method updates the `sui_address` field for the user in the `users` table.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    /// * `sui_address` - The SUI address to store for the user.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    ///
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn store_sui_address(state_manager: &AtomaStateManager, user_id: i64, sui_address: String) -> Result<(), AtomaStateManagerError> {
    ///    state_manager.store_sui_address(user_id, sui_address).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn update_sui_address(&self, user_id: i64, sui_address: String) -> Result<()> {
        sqlx::query("UPDATE users SET sui_address = $1 WHERE id = $2")
            .bind(sui_address)
            .bind(user_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Retrieves the SUI address for the user.
    ///
    /// This method retrieves the SUI address for the user from the `users` table.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    ///
    /// # Returns
    ///
    /// - `Result<Option<String>>`: A result containing either:
    ///  - `Ok(Some(String))`: The SUI address of the user.
    /// - `Ok(None)`: If the user is not found.
    /// - `Err(AtomaStateManagerError)`: An error if the database query fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    ///
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_sui_address(state_manager: &AtomaStateManager, user_id: i64) -> Result<Option<String>, AtomaStateManagerError> {
    ///    state_manager.get_sui_address(user_id).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn get_sui_address(&self, user_id: i64) -> Result<Option<String>> {
        let sui_address =
            sqlx::query_scalar::<_, Option<String>>("SELECT sui_address FROM users WHERE id = $1")
                .bind(user_id)
                .fetch_optional(&self.db)
                .await?;

        Ok(sui_address.flatten())
    }

    /// Confirms that the user is associated with the wallet.
    ///
    /// This method confirms that users has this wallet associated with them.
    ///
    /// # Arguments
    ///
    /// * `sui_address` - The sui_address of the user.
    /// * `user_id` - The unique identifier of the user.
    ///
    /// # Returns
    ///
    /// - `Result<bool>`: A result containing either:
    /// - `Ok(true)`: If the user is associated with the wallet.
    /// - `Ok(false)`: If the user is not associated with the wallet.
    /// - `Err(AtomaStateManagerError)`: An error if the database query fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    ///
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn confirm_user(state_manager: &AtomaStateManager, sui_address: String, user_id: i64) -> Result<Option<i64>, AtomaStateManagerError> {
    ///    state_manager.confirm_user(sui_address, user_id).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn confirm_user(&self, sui_address: String, user_id: i64) -> Result<bool> {
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM users WHERE sui_address = $1 and id = $2)",
        )
        .bind(sui_address)
        .bind(user_id)
        .fetch_one(&self.db)
        .await?;
        Ok(exists)
    }

    /// Update the balance for the user.
    ///
    /// This method updates the `usdc_balance` field for the user in the `users` table.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    /// * `balance` - The new balance to store for the user.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    ///
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn top_up_crypto_balance(state_manager: &AtomaStateManager, user_id: i64, balance: i64) -> Result<(), AtomaStateManagerError> {
    ///    state_manager.top_up_crypto_balance(user_id, balance).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn top_up_crypto_balance(&self, user_id: i64, balance: i64) -> Result<()> {
        sqlx::query(
            "INSERT INTO crypto_balances (user_id, usdc_balance)
                         VALUES ($1, $2)
                         ON CONFLICT (user_id)
                         DO UPDATE SET
                            usdc_balance = crypto_balances.usdc_balance + EXCLUDED.usdc_balance",
        )
        .bind(user_id)
        .bind(balance)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Deduct from the usdc balance for the user.
    ///
    /// This method withdraws the balance from the user in the `users` table.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    /// * `balance` - The balance to withdraw from the user.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    ///
    /// - The database query fails to execute (that could mean the balance is not available)
    #[instrument(level = "trace", skip(self))]
    pub async fn deduct_from_usdc(&self, user_id: i64, balance: i64) -> Result<()> {
        let result = sqlx::query("UPDATE crypto_balances SET usdc_balance = usdc_balance - $2 WHERE user_id = $1 AND usdc_balance >= $2")
            .bind(user_id)
            .bind(balance)
            .execute(&self.db)
            .await?;
        if result.rows_affected() != 1 {
            return Err(AtomaStateManagerError::InsufficientBalance);
        }
        Ok(())
    }

    /// Refunds a USDC payment.
    ///
    /// This method refunds a USDC payment to the user in the `crypto_balances` table.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    /// * `amount` - The amount to refund.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    ///
    /// - The database query fails to execute.
    #[instrument(level = "trace", skip(self))]
    pub async fn refund_usdc(&self, user_id: i64, amount: i64) -> Result<()> {
        sqlx::query(
            "UPDATE crypto_balances SET usdc_balance = usdc_balance + $2 WHERE user_id = $1",
        )
        .bind(user_id)
        .bind(amount)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Insert a new usdc payment digest.
    ///
    /// This method inserts a new usdc payment digest into the `usdc_payment_digests` table. It fails if the digest already exists.
    ///
    /// # Arguments
    ///
    /// * `digest` - The digest to insert.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    ///
    /// - The database query fails to execute. Including if the digest already exists.
    #[instrument(level = "trace", skip(self))]
    pub async fn insert_new_usdc_payment_digest(&self, digest: String) -> Result<()> {
        sqlx::query("INSERT INTO usdc_payment_digests (digest) VALUES ($1)")
            .bind(digest)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Gets the zk_salt for the user.
    ///
    /// This method fetches the zk_salt for the user from the `users` table.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    ///
    /// # Returns
    ///
    /// - `Result<Option<String>>`: A result containing either:
    ///   - `Ok(Some(String))`: The zk_salt for the user.
    ///   - `Ok(None)`: If the user is not found.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    ///
    /// - The database query fails to execute.
    #[instrument(level = "trace", skip(self))]
    pub async fn get_zk_salt(&self, user_id: i64) -> Result<Option<String>> {
        let zk_salt =
            sqlx::query_scalar::<_, Option<String>>("SELECT zk_salt FROM users WHERE id = $1")
                .bind(user_id)
                .fetch_optional(&self.db)
                .await?;

        Ok(zk_salt.flatten())
    }

    /// Sets the zk_salt for the user.
    ///
    /// This method updates the `zk_salt` field for the user in the `users` table.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    /// * `zk_salt` - The new zk_salt to store for the user.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    ///
    /// - The database query fails to execute.
    pub async fn set_zk_salt(&self, user_id: i64, zk_salt: &str) -> Result<()> {
        sqlx::query("UPDATE users SET zk_salt = $1 WHERE id = $2")
            .bind(zk_salt)
            .bind(user_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Locks the fiat balance for a user.
    ///
    /// This method locks the fiat balance for a user in the `fiat_balance` table.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    /// * `amount` - The amount to lock.
    ///
    /// # Returns
    ///
    /// - `Result<bool>`: A result indicating whether the lock was successful.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    ///
    /// - The database query fails to execute.
    #[instrument(level = "trace", skip(self))]
    pub async fn lock_user_fiat_balance(
        &self,
        user_id: i64,
        input_amount: i64,
        output_amount: i64,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE fiat_balances 
             SET overcharged_unsettled_input_amount = overcharged_unsettled_input_amount + $2,
                 overcharged_unsettled_completions_amount = overcharged_unsettled_completions_amount + $3
             WHERE user_id = $1 AND 
                   usd_balance >= already_debited_completions_amount + already_debited_input_amount + overcharged_unsettled_completions_amount + overcharged_unsettled_input_amount + $2 + $3",
        )
        .bind(user_id)
        .bind(input_amount)
        .bind(output_amount)
        .execute(&self.db)
        .await?;

        Ok(result.rows_affected() == 1)
    }

    /// Updates the usd amount already computed for a user.
    ///
    /// This method updates the `already_debited_amount` field in the `fiat_balance` table
    /// for the specified `user_id`.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user to update.
    /// * `estimated_amount` - The estimated amount.
    /// * `total_amount` - The total amount.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    #[instrument(level = "trace", skip(self))]
    pub async fn update_real_amount_fiat_balance(
        &self,
        user_id: i64,
        estimated_input_amount: i64,
        input_amount: i64,
        estimated_output_amount: i64,
        output_amount: i64,
    ) -> Result<()> {
        let result = sqlx::query(
            "UPDATE fiat_balances 
                SET 
                    overcharged_unsettled_input_amount = overcharged_unsettled_input_amount - $2,
                    already_debited_input_amount = already_debited_input_amount + $3,
                    overcharged_unsettled_completions_amount = overcharged_unsettled_completions_amount - $4,
                    already_debited_completions_amount = already_debited_completions_amount + $5,
                    num_requests = num_requests + $6
                WHERE user_id = $1",
        )
        .bind(user_id)
        .bind(estimated_input_amount)
        .bind(input_amount)
        .bind(estimated_output_amount)
        .bind(output_amount)
        .bind(i64::from(output_amount > 0)) // If total amount is greater than 0 then the request was successful
        .execute(&self.db)
        .await?;

        if result.rows_affected() == 0 {
            return Err(AtomaStateManagerError::InsufficientBalance);
        }

        Ok(())
    }

    /// Updates the usage per day for a user and model.
    ///
    /// This method updates the `usage_per_day` table for the specified user and model.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    /// * `model` - The name of the model.
    /// * `input_amount` - The input amount for the model.
    /// * `input_tokens` - The input tokens for the model.
    /// * `output_amount` - The output amount for the model.
    /// * `output_tokens` - The output tokens for the model.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    /// use chrono::Utc;
    /// async fn update_usage_per_day(state_manager: &AtomaStateManager, user_id: i64, model: String, input_amount: i64, input_tokens: i64, output_amount: i64, output_tokens: i64) -> Result<(), AtomaStateManagerError> {
    ///     state_manager.update_per_day_table(user_id, model, input_amount, input_tokens, output_amount, output_tokens).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn update_per_day_table(
        &self,
        user_id: i64,
        model: String,
        input_amount: i64,
        input_tokens: i64,
        output_amount: i64,
        output_tokens: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO usage_per_day (user_id, model, input_amount, input_tokens, output_amount, output_tokens)
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (user_id, model, date) DO UPDATE SET
                    input_amount = usage_per_day.input_amount + EXCLUDED.input_amount,
                    input_tokens = usage_per_day.input_tokens + EXCLUDED.input_tokens,
                    output_amount = usage_per_day.output_amount + EXCLUDED.output_amount,
                    output_tokens = usage_per_day.output_tokens + EXCLUDED.output_tokens",
        )
        .bind(user_id)
        .bind(model)
        .bind(input_amount)
        .bind(input_tokens)
        .bind(output_amount)
        .bind(output_tokens)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Updates the usage per model for a user.
    ///
    /// This method updates the `usage_per_model` table for the specified user and model.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    /// * `model_name` - The name of the model.
    /// * `total_tokens` - The total tokens used by the user for the model.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn update_usage(state_manager: &AtomaStateManager, user_id: i64, model_name: String, total_tokens: i64) -> Result<(), AtomaStateManagerError> {
    ///     state_manager.update_usage_per_model(user_id, model_name, total_tokens).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn update_usage_per_model(
        &self,
        user_id: i64,
        model_name: String,
        input_tokens: i64,
        output_tokens: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO usage_per_model (user_id, model, total_input_tokens, total_output_tokens)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (user_id, model) DO UPDATE SET
                    total_input_tokens = usage_per_model.total_input_tokens + EXCLUDED.total_input_tokens,
                    total_output_tokens = usage_per_model.total_output_tokens + EXCLUDED.total_output_tokens",
        )
        .bind(user_id)
        .bind(model_name)
        .bind(input_tokens)
        .bind(output_tokens)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Sets the price for a user for a specific model.
    ///
    /// This method inserts or updates the price for a user for a specific model in the `user_model_prices` table.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    /// * `model_name` - The name of the model.
    /// * `price_per_one_million_input_compute_units` - The price per one million input compute units.
    /// * `price_per_one_million_output_compute_units` - The price per one million output compute units.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - The user_id or model_name is invalid.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    /// async fn set_custom_pricing(state_manager: &AtomaStateManager, user_id: i64, model_name: String, price_per_one_million_input_compute_units: i64, price_per_one_million_output_compute_units: i64) -> Result<(), AtomaStateManagerError> {
    ///     state_manager.set_custom_pricing(user_id, model_name, price_per_one_million_input_compute_units, price_per_one_million_output_compute_units).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn set_custom_pricing(
        &self,
        user_id: i64,
        model: &str,
        price_per_one_million_input_compute_units: i64,
        price_per_one_million_output_compute_units: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO user_model_prices (user_id, model_name, price_per_one_million_input_compute_units,price_per_one_million_output_compute_units) VALUES ($1, $2, $3, $4)"
        )
        .bind(user_id)
        .bind(model)
        .bind(price_per_one_million_input_compute_units)
        .bind(price_per_one_million_output_compute_units)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Gets the price for a user for a specific model.
    ///
    /// This method retrieves the price for a user for a specific model from the `user_model_prices` table.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The unique identifier of the user.
    /// * `model_name` - The name of the model.
    ///
    /// # Returns
    ///
    /// - `Result<Option<Pricing>>`: A result containing either:
    ///   - `Ok(Some(Pricing))`: The pricing information for the user and model.
    ///   - `Ok(None)`: If no pricing information is found for the user and model.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - The user_id or model_name is invalid.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::{AtomaStateManager, Pricing};
    ///
    /// async fn get_custom_pricing(state_manager: &AtomaStateManager, user_id: i64, model: &str) -> Result<Option<Pricing>, AtomaStateManagerError> {
    ///     state_manager.get_custom_pricing(user_id, model).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn get_custom_pricing(&self, user_id: i64, model: &str) -> Result<Option<Pricing>> {
        let pricing = sqlx::query(
            "SELECT price_per_one_million_input_compute_units, price_per_one_million_output_compute_units FROM user_model_prices WHERE user_id = $1 AND model_name = $2 ORDER BY creation_timestamp DESC LIMIT 1",
        )
        .bind(user_id)
        .bind(model)
        .fetch_optional(&self.db)
        .await?;

        pricing
            .map(|pricing| Pricing::from_row(&pricing).map_err(AtomaStateManagerError::from))
            .transpose()
    }
}

pub mod validation {
    use tracing::{error, instrument};

    use crate::AtomaStateManagerError;

    /// Validates that the timestamp is not in the future.
    ///
    /// # Arguments
    ///
    /// * `timestamp` - The timestamp to validate.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    ///
    /// - The timestamp is in the future.
    #[instrument(level = "debug")]
    pub fn validate_timestamp(timestamp: i64) -> Result<(), AtomaStateManagerError> {
        // Max timestamp drift in milliseconds, to account for clock drift between nodes
        const MAX_TIMESTAMP_DRIFT: i64 = 250; // 250 milliseconds
                                              // Validate timestamp is not in the future
        let current_time = chrono::Utc::now().timestamp_millis();
        if timestamp > (current_time + MAX_TIMESTAMP_DRIFT) / 1000 {
            error!("Timestamp is in the future: {timestamp}");
            return Err(AtomaStateManagerError::InvalidTimestamp);
        }
        Ok(())
    }

    /// Validates that the country is a valid ISO 3166-1 alpha-2 code.
    ///
    /// # Arguments
    ///
    /// * `country` - The country to validate.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    ///
    /// - The country is not a valid ISO 3166-1 alpha-2 code.
    #[instrument(level = "debug")]
    pub fn validate_country(country: &str) -> Result<(), AtomaStateManagerError> {
        if isocountry::CountryCode::for_alpha2(country).is_err() {
            error!("Country is not a valid ISO 3166-1 alpha-2 code: {country}");
            return Err(AtomaStateManagerError::InvalidCountry(country.to_string()));
        }
        Ok(())
    }

    /// Validates that the url is valid.
    ///
    /// # Arguments
    ///
    /// * `url` - The url to validate.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    ///
    /// - The url is not valid.
    #[instrument(level = "debug")]
    pub fn validate_url(url: &str) -> Result<(), AtomaStateManagerError> {
        if url::Url::parse(url).is_err() {
            error!("URL is not valid: {url}");
            return Err(AtomaStateManagerError::InvalidUrl(url.to_string()));
        }
        Ok(())
    }
}
