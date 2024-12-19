use crate::build_query_with_in;
use crate::handlers::{handle_atoma_event, handle_state_manager_event};
use crate::types::{
    AtomaAtomaStateManagerEvent, CheapestNode, ComputedUnitsProcessedResponse, LatencyResponse,
    NodeDistribution, NodePublicKey, NodeSubscription, Stack, StackAttestationDispute,
    StackSettlementTicket, StatsStackResponse, Task,
};

use atoma_sui::events::AtomaEvent;
use chrono::{DateTime, Timelike, Utc};
use flume::Receiver as FlumeReceiver;
use sqlx::PgPool;
use sqlx::{FromRow, Row};
use thiserror::Error;
use tokio::sync::watch::Receiver;
use tracing::instrument;

pub(crate) type Result<T> = std::result::Result<T, AtomaStateManagerError>;

/// AtomaStateManager is a wrapper around a Postgres connection pool, responsible for managing the state of the Atoma system.
///
/// It provides an interface to interact with the Postgres database, handling operations
/// related to tasks, node subscriptions, stacks, and various other system components.
pub struct AtomaStateManager {
    /// The Postgres connection pool used for database operations.
    pub state: AtomaState,
    /// Receiver channel from the SuiEventSubscriber
    pub event_subscriber_receiver: FlumeReceiver<AtomaEvent>,
    /// Atoma service receiver
    pub state_manager_receiver: FlumeReceiver<AtomaAtomaStateManagerEvent>,
    /// Sui address
    pub sui_address: String,
}

impl AtomaStateManager {
    /// Constructor
    pub fn new(
        db: PgPool,
        event_subscriber_receiver: FlumeReceiver<AtomaEvent>,
        state_manager_receiver: FlumeReceiver<AtomaAtomaStateManagerEvent>,
        sui_address: String,
    ) -> Self {
        Self {
            state: AtomaState::new(db),
            event_subscriber_receiver,
            state_manager_receiver,
            sui_address,
        }
    }

    /// Creates a new `AtomaStateManager` instance from a database URL.
    ///
    /// This method establishes a connection to the Postgres database using the provided URL,
    /// creates all necessary tables in the database, and returns a new `AtomaStateManager` instance.
    pub async fn new_from_url(
        database_url: &str,
        event_subscriber_receiver: FlumeReceiver<AtomaEvent>,
        state_manager_receiver: FlumeReceiver<AtomaAtomaStateManagerEvent>,
        sui_address: String,
    ) -> Result<Self> {
        let db = PgPool::connect(database_url).await?;
        // run migrations
        sqlx::migrate!("./src/migrations").run(&db).await?;
        Ok(Self {
            state: AtomaState::new(db),
            event_subscriber_receiver,
            state_manager_receiver,
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
    #[instrument(level = "trace", skip_all)]
    pub async fn run(self, mut shutdown_signal: Receiver<bool>) -> Result<()> {
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
                                continue;
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
                                continue;
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
                            continue;
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
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    /// Creates a new `AtomaState` instance from a database URL.
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
        // TODO: filter also by security level and other constraints
        let mut query = String::from(
            r#"
            WITH selected_stack AS (
                SELECT stacks.stack_small_id
                FROM stacks
                INNER JOIN tasks ON tasks.task_small_id = stacks.task_small_id"#,
        );

        if is_confidential {
            query.push_str(r#"
                INNER JOIN node_public_keys ON node_public_keys.node_small_id = stacks.node_small_id"#);
        }

        query.push_str(
            r#"
                WHERE tasks.model_name = $1
                AND stacks.num_compute_units - stacks.already_computed_units >= $2
                AND stacks.user_id = $3"#,
        );

        if is_confidential {
            query.push_str(
                r#"
                AND tasks.security_level = 2
                AND node_public_keys.is_valid = true"#,
            );
        }

        query.push_str(
            r#"
                LIMIT 1
            )
            UPDATE stacks
            SET already_computed_units = already_computed_units + $2
            WHERE stack_small_id IN (SELECT stack_small_id FROM selected_stack)
            RETURNING stacks.*"#,
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

    /// Gets the node with the cheapest price per compute unit for a given model.
    ///
    /// This method queries the database to find the node subscription with the lowest price per compute unit
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
    /// The returned `CheapestNode` contains:
    /// - The task's small ID
    /// - The price per compute unit
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
        let mut query = String::from(
            r#"
            SELECT tasks.task_small_id, price_per_compute_unit, max_num_compute_units, node_subscriptions.node_small_id
            FROM tasks
            INNER JOIN node_subscriptions ON tasks.task_small_id = node_subscriptions.task_small_id"#,
        );

        if is_confidential {
            query.push_str(r#"
            INNER JOIN node_public_keys ON node_public_keys.node_small_id = node_subscriptions.node_small_id"#);
        }

        query.push_str(
            r#"
            WHERE tasks.is_deprecated = false
            AND tasks.model_name = $1
            AND node_subscriptions.valid = true"#,
        );

        if is_confidential {
            query.push_str(
                r#"
            AND tasks.security_level = 2
            AND node_public_keys.is_valid = true"#,
            );
        }

        query.push_str(
            r#"
            ORDER BY node_subscriptions.price_per_compute_unit 
            LIMIT 1"#,
        );

        let node_settings = sqlx::query(&query)
            .bind(model)
            .bind(false)
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
    /// 3. Supports the specified model with security level 2 (confidential computing)
    ///
    /// The nodes are ordered by stack price, so the most cost-effective stack meeting
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
    /// # Database Query Details
    ///
    /// The method joins three tables:
    /// - `node_public_keys`: For encryption key information
    /// - `stacks`: For compute capacity and pricing
    /// - `tasks`: For model and security level verification
    ///
    /// The results are filtered to ensure:
    /// - The stack supports the requested model
    /// - The task requires security level 2 (confidential computing)
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
    ) -> Result<Option<NodePublicKey>> {
        let node = sqlx::query(
            r#"
            SELECT node_public_keys.public_key, node_public_keys.node_small_id
                FROM node_public_keys
                INNER JOIN stacks ON stacks.selected_node_id = node_public_keys.node_small_id
                INNER JOIN tasks ON tasks.task_small_id = stacks.task_small_id
                WHERE tasks.model_name = $1
                AND tasks.security_level = 2
                AND stacks.num_compute_units - stacks.already_computed_units >= $2
                AND node_public_keys.is_valid = true
                ORDER BY stacks.price ASC
                LIMIT 1
            "#,
        )
        .bind(model)
        .bind(max_num_tokens)
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
            sqlx::query(r#"SELECT node_small_id, public_key FROM node_public_keys WHERE node_small_id = $1 and is_valid = true"#)
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
    /// * `username` - The username of the user.
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
    /// async fn register_user(state_manager: &AtomaStateManager, username: &str, password_hash: &str) -> Result<Option<i64>, AtomaStateManagerError> {
    ///    state_manager.register(username, password_hash).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn register(&self, username: &str, password_hash: &str) -> Result<Option<i64>> {
        let result = sqlx::query("INSERT INTO users (username, password_hash) VALUES ($1, $2) ON CONFLICT (username) DO NOTHING RETURNING id")
        .bind(username)
        .bind(password_hash)
        .fetch_optional(&self.db)
        .await?;
        Ok(result.map(|record| record.get("id")))
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

    /// Subscribes a node to a task with a specified price per compute unit.
    ///
    /// This method inserts a new entry into the `node_subscriptions` table to
    /// establish a subscription relationship between a node and a task, along
    /// with the specified price per compute unit for the subscription.
    ///
    /// # Arguments
    ///
    /// * `node_small_id` - The unique identifier of the node to be subscribed.
    /// * `task_small_id` - The unique identifier of the task to which the node is subscribing.
    /// * `price_per_compute_unit` - The price per compute unit for the subscription.
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
    /// async fn subscribe_node_to_task(state_manager: &AtomaStateManager, node_small_id: i64, task_small_id: i64, price_per_compute_unit: i64) -> Result<(), AtomaStateManagerError> {
    ///     state_manager.subscribe_node_to_task(node_small_id, task_small_id, price_per_compute_unit).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(
            %node_small_id,
            %task_small_id,
            %price_per_compute_unit,
            %max_num_compute_units
        )
    )]
    pub async fn subscribe_node_to_task(
        &self,
        node_small_id: i64,
        task_small_id: i64,
        price_per_compute_unit: i64,
        max_num_compute_units: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO node_subscriptions 
                (node_small_id, task_small_id, price_per_compute_unit, max_num_compute_units, valid) 
                VALUES ($1, $2, $3, $4, TRUE)",
        )
            .bind(node_small_id)
            .bind(task_small_id)
            .bind(price_per_compute_unit)
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
    /// price per compute unit and the maximum number of compute units for an existing
    /// subscription between a node and a task.
    ///
    /// # Arguments
    ///
    /// * `node_small_id` - The unique identifier of the subscribed node.
    /// * `task_small_id` - The unique identifier of the task to which the node is subscribed.
    /// * `price_per_compute_unit` - The new price per compute unit for the subscription.
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
            %price_per_compute_unit,
            %max_num_compute_units
        )
    )]
    pub async fn update_node_subscription(
        &self,
        node_small_id: i64,
        task_small_id: i64,
        price_per_compute_unit: i64,
        max_num_compute_units: i64,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE node_subscriptions SET price_per_compute_unit = $1, max_num_compute_units = $2 WHERE node_small_id = $3 AND task_small_id = $4",
        )
            .bind(price_per_compute_unit)
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
            r#"
            UPDATE stacks
            SET already_computed_units = already_computed_units + $1
            WHERE stack_small_id = $2
            AND owner = $3
            AND num_compute_units - already_computed_units >= $1
            AND in_settle_period = false
            RETURNING *
            "#,
        )
        .bind(num_compute_units)
        .bind(stack_small_id)
        .bind(public_key)
        .fetch_optional(&self.db)
        .await?;

        Ok(maybe_stack)
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
            price = %stack.price)
    )]
    pub async fn insert_new_stack(&self, stack: Stack, user_id: i64) -> Result<()> {
        sqlx::query(
            "INSERT INTO stacks 
                (owner, stack_small_id, stack_id, task_small_id, selected_node_id, num_compute_units, price, already_computed_units, in_settle_period, total_hash, num_total_messages, user_id) 
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
            .bind(stack.owner)
            .bind(stack.stack_small_id)
            .bind(stack.stack_id)
            .bind(stack.task_small_id)
            .bind(stack.selected_node_id)
            .bind(stack.num_compute_units)
            .bind(stack.price)
            .bind(stack.already_computed_units)
            .bind(stack.in_settle_period)
            .bind(stack.total_hash)
            .bind(stack.num_total_messages)
            .bind(user_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Updates the number of compute units already computed for a stack.
    ///
    /// This method updates the `already_computed_units` field in the `stacks` table
    /// for the specified `stack_small_id`.
    ///
    /// # Arguments
    ///
    /// * `stack_small_id` - The unique small identifier of the stack to update.
    /// * `already_computed_units` - The number of compute units already computed.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
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
    /// async fn update_computed_units(state_manager: &AtomaStateManager, stack_small_id: i64, already_computed_units: i64) -> Result<(), AtomaStateManagerError> {
    ///     state_manager.update_computed_units_for_stack(stack_small_id, already_computed_units).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(%stack_small_id, %already_computed_units)
    )]
    pub async fn update_computed_units_for_stack(
        &self,
        stack_small_id: i64,
        already_computed_units: i64,
    ) -> Result<()> {
        sqlx::query("UPDATE stacks SET already_computed_units = $1 WHERE stack_small_id = $2")
            .bind(already_computed_units)
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
            SET already_computed_units = already_computed_units - ($1 - $2) 
            WHERE stack_small_id = $3",
        )
        .bind(estimated_total_tokens)
        .bind(total_tokens)
        .bind(stack_small_id)
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

    /// Updates the total hash and increments the total number of messages for a stack.
    ///
    /// This method updates the `total_hash` field in the `stacks` table by appending a new hash
    /// to the existing hash and increments the `num_total_messages` field by 1 for the specified `stack_small_id`.
    ///
    /// # Arguments
    ///
    /// * `stack_small_id` - The unique small identifier of the stack to update.
    /// * `new_hash` - A 32-byte array representing the new hash to append to the existing total hash.
    ///
    /// # Returns
    ///
    /// - `Result<()>`: A result indicating success (Ok(())) or failure (Err(AtomaStateManagerError)).
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database transaction fails to begin, execute, or commit.
    /// - The specified stack is not found.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn update_hash(state_manager: &AtomaStateManager) -> Result<(), AtomaStateManagerError> {
    ///     let stack_small_id = 1;
    ///     let new_hash = [0u8; 32]; // Example hash
    ///
    ///     state_manager.update_stack_total_hash(stack_small_id, new_hash).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(%stack_small_id, ?new_hash)
    )]
    pub async fn update_stack_total_hash(
        &self,
        stack_small_id: i64,
        new_hash: [u8; 32],
    ) -> Result<()> {
        let rows_affected = sqlx::query(
            "UPDATE stacks 
            SET total_hash = total_hash || $1,
                num_total_messages = num_total_messages + 1
            WHERE stack_small_id = $2",
        )
        .bind(&new_hash[..])
        .bind(stack_small_id)
        .execute(&self.db)
        .await?
        .rows_affected();

        if rows_affected == 0 {
            return Err(AtomaStateManagerError::StackNotFound);
        }

        Ok(())
    }

    /// Retrieves the total hash for a specific stack.
    ///
    /// This method fetches the `total_hash` field from the `stacks` table for the given `stack_small_id`.
    ///
    /// # Arguments
    ///
    /// * `stack_small_id` - The unique small identifier of the stack whose total hash is to be retrieved.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<u8>>`: A result containing either:
    ///   - `Ok(Vec<u8>)`: A byte vector representing the total hash of the stack.
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
    /// async fn get_total_hash(state_manager: &AtomaStateManager, stack_small_id: i64) -> Result<Vec<u8>, AtomaStateManagerError> {
    ///     state_manager.get_stack_total_hash(stack_small_id).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(%stack_small_id)
    )]
    pub async fn get_stack_total_hash(&self, stack_small_id: i64) -> Result<Vec<u8>> {
        let total_hash = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT total_hash FROM stacks WHERE stack_small_id = $1",
        )
        .bind(stack_small_id)
        .fetch_one(&self.db)
        .await?;
        Ok(total_hash)
    }

    /// Retrieves the total hashes for multiple stacks in a single query.
    ///
    /// This method efficiently fetches the `total_hash` field from the `stacks` table for all
    /// provided stack IDs in a single database query.
    ///
    /// # Arguments
    ///
    /// * `stack_small_ids` - A slice of stack IDs whose total hashes should be retrieved.
    ///
    /// # Returns
    ///
    /// - `Result<Vec<Vec<u8>>>`: A result containing either:
    ///   - `Ok(Vec<Vec<u8>>)`: A vector of byte vectors, where each inner vector represents
    ///     the total hash of a stack. The order corresponds to the order of results returned
    ///     by the database query.
    ///   - `Err(AtomaStateManagerError)`: An error if the database query fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The database query fails to execute.
    /// - There's an issue retrieving the hash data from the result rows.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use atoma_node::atoma_state::AtomaStateManager;
    ///
    /// async fn get_hashes(state_manager: &AtomaStateManager) -> Result<Vec<Vec<u8>>, AtomaStateManagerError> {
    ///     let stack_ids = &[1, 2, 3]; // IDs of stacks to fetch hashes for
    ///     state_manager.get_all_total_hashes(stack_ids).await
    /// }
    /// ```
    #[instrument(
        level = "trace",
        skip_all,
        fields(?stack_small_ids)
    )]
    pub async fn get_all_total_hashes(&self, stack_small_ids: &[i64]) -> Result<Vec<Vec<u8>>> {
        let mut query_builder = build_query_with_in(
            "SELECT total_hash FROM stacks",
            "stack_small_id",
            stack_small_ids,
            None,
        );

        Ok(query_builder
            .build()
            .fetch_all(&self.db)
            .await?
            .iter()
            .map(|row| row.get("total_hash"))
            .collect())
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
        sqlx::query("UPDATE nodes SET public_address = $2, country = $3 WHERE node_small_id = $1")
            .bind(small_id)
            .bind(address)
            .bind(country)
            .execute(&self.db)
            .await?;
        Ok(())
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

    /// Updates the throughput performance of a node.
    ///
    /// This method updates the `node_throughput_performance` table with the throughput performance of a node.
    ///
    /// # Arguments
    ///
    /// * `node_small_id` - The unique small identifier of the node.
    /// * `input_tokens` - The number of input tokens processed by the node.
    /// * `output_tokens` - The number of output tokens processed by the node.
    /// * `time` - The time taken to process the tokens.
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
    /// async fn update_performance(state_manager: &AtomaStateManager, node_small_id: i64, input_tokens: i64, output_tokens: i64, time: f64) -> Result<(), AtomaStateManagerError> {
    ///    state_manager.update_node_throughput_performance(node_small_id, input_tokens, output_tokens, time).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%node_small_id, %input_tokens, %output_tokens, %time))]
    pub async fn update_node_throughput_performance(
        &self,
        node_small_id: i64,
        input_tokens: i64,
        output_tokens: i64,
        time: f64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO node_throughput_performance 
                (node_small_id, queries, input_tokens, output_tokens, time) 
                VALUES ($1, $2, $3, $4, $5) 
                ON CONFLICT (node_small_id)
                DO UPDATE SET queries = node_throughput_performance.queries + EXCLUDED.queries, 
                              input_tokens = node_throughput_performance.input_tokens + EXCLUDED.input_tokens,
                              output_tokens = node_throughput_performance.output_tokens + EXCLUDED.output_tokens,
                              time = node_throughput_performance.time + EXCLUDED.time",
        )
        .bind(node_small_id)
        .bind(1)
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(time)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Updates the prefill performance of a node.
    ///
    /// This method updates the `node_prefill_performance` table with the prefill performance of a node.
    ///
    /// # Arguments
    ///
    /// * `node_small_id` - The unique small identifier of the node.
    /// * `input_tokens` - The number of input tokens processed by the node.
    /// * `output_tokens` - The number of output tokens processed by the node.
    /// * `time` - The time taken to process the tokens.
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
    /// async fn update_performance(state_manager: &AtomaStateManager, node_small_id: i64, input_tokens: i64, output_tokens: i64, time: f64) -> Result<(), AtomaStateManagerError> {
    ///    state_manager.update_node_prefill_performance(node_small_id, input_tokens, output_tokens, time).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%node_small_id, %tokens, %time))]
    pub async fn update_node_prefill_performance(
        &self,
        node_small_id: i64,
        tokens: i64,
        time: f64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO node_prefill_performance 
                (node_small_id, queries, tokens, time) 
                VALUES ($1, $2, $3, $4) 
                ON CONFLICT (node_small_id)
                DO UPDATE SET queries = node_prefill_performance.queries + EXCLUDED.queries, 
                              tokens = node_prefill_performance.tokens + EXCLUDED.tokens,
                              time = node_prefill_performance.time + EXCLUDED.time",
        )
        .bind(node_small_id)
        .bind(1)
        .bind(tokens)
        .bind(time)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Updates the decode performance of a node.
    ///
    /// This method updates the `node_decode_performance` table with the decode performance of a node.
    ///
    /// # Arguments
    ///
    /// * `node_small_id` - The unique small identifier of the node.
    /// * `input_tokens` - The number of input tokens processed by the node.
    /// * `output_tokens` - The number of output tokens processed by the node.
    /// * `time` - The time taken to process the tokens.
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
    /// async fn update_performance(state_manager: &AtomaStateManager, node_small_id: i64, input_tokens: i64, output_tokens: i64, time: f64) -> Result<(), AtomaStateManagerError> {
    ///    state_manager.update_node_decode_performance(node_small_id, input_tokens, output_tokens, time).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%node_small_id, %tokens, %time))]
    pub async fn update_node_decode_performance(
        &self,
        node_small_id: i64,
        tokens: i64,
        time: f64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO node_decode_performance 
                (node_small_id, queries, tokens, time) 
                VALUES ($1, $2, $3, $4) 
                ON CONFLICT (node_small_id)
                DO UPDATE SET queries = node_decode_performance.queries + EXCLUDED.queries, 
                              tokens = node_decode_performance.tokens + EXCLUDED.tokens,
                              time = node_decode_performance.time + EXCLUDED.time",
        )
        .bind(node_small_id)
        .bind(1)
        .bind(tokens)
        .bind(time)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Updates the latency performance of a node.
    ///
    /// This method updates the `node_latency_performance` table with the latency performance of a node.
    ///
    /// # Arguments
    ///
    /// * `node_small_id` - The unique small identifier of the node.
    /// * `latency` - The latency of the node.
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
    /// async fn update_latency_performance(state_manager: &AtomaStateManager, node_small_id: i64, latency: f64) -> Result<(), AtomaStateManagerError> {
    ///    state_manager.update_node_latency_performance(node_small_id, latency).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%node_small_id, %latency))]
    pub async fn update_node_latency_performance(
        &self,
        node_small_id: i64,
        latency: f64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO node_latency_performance 
                (node_small_id, queries, latency) 
                VALUES ($1, $2, $3) 
                ON CONFLICT (node_small_id)
                DO UPDATE SET queries = node_latency_performance.queries + EXCLUDED.queries,
                              latency = node_latency_performance.latency + EXCLUDED.latency",
        )
        .bind(node_small_id)
        .bind(1)
        .bind(latency)
        .execute(&self.db)
        .await?;
        Ok(())
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
    /// * `tee_remote_attestation_bytes` - The TEE (Trusted Execution Environment) remote attestation data as bytes.
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
    pub async fn update_node_public_key(
        &self,
        node_id: i64,
        epoch: i64,
        new_public_key: Vec<u8>,
        tee_remote_attestation_bytes: Vec<u8>,
        is_valid: bool,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO node_public_keys (node_small_id, epoch, public_key, tee_remote_attestation_bytes, is_valid) VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT (node_small_id)
                DO UPDATE SET epoch = $2, 
                              public_key = $3, 
                              tee_remote_attestation_bytes = $4,
                              is_valid = $5",
        )
        .bind(node_id)
        .bind(epoch)
        .bind(new_public_key)
        .bind(tee_remote_attestation_bytes)
        .bind(is_valid)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Retrieves the X25519 public key for a selected node.
    ///
    /// This method retrieves the X25519 public key for a specific node from the `node_public_keys` table.
    ///
    /// # Arguments
    ///
    /// * `selected_node_id` - The unique small identifier of the node.
    ///
    /// # Returns
    ///
    /// - `Result<Option<String>>`: A result containing the X25519 public key if found, or None if not found.
    #[instrument(level = "trace", skip_all, fields(%selected_node_id))]
    pub async fn get_selected_node_x25519_public_key(
        &self,
        selected_node_id: i64,
    ) -> Result<Option<Vec<u8>>> {
        let public_key =
            sqlx::query("SELECT public_key FROM node_public_keys WHERE node_small_id = $1")
                .bind(selected_node_id)
                .fetch_optional(&self.db)
                .await?;
        Ok(public_key.map(|row| row.get::<Vec<u8>, _>("public_key")))
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
    pub async fn insert_new_node(&self, node_small_id: i64, sui_address: String) -> Result<()> {
        sqlx::query("INSERT INTO nodes (node_small_id, sui_address) VALUES ($1, $2)")
            .bind(node_small_id)
            .bind(sui_address)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Get the user_id by username and password.
    ///
    /// This method queries the `users` table to get the user_id by username and password.
    ///
    /// # Arguments
    ///
    /// * `username` - The username of the user.
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
    /// async fn get_user_id(state_manager: &AtomaStateManager, username: &str, hashed_password: &str) -> Result<Option<i64>, AtomaStateManagerError> {
    ///    state_manager.get_user_id_by_username_password(username, hashed_password).await
    /// }
    /// ```
    #[instrument(level = "trace", skip(self))]
    pub async fn get_user_id_by_username_password(
        &self,
        username: &str,
        hashed_password: &str,
    ) -> Result<Option<i64>> {
        let user = sqlx::query("SELECT id FROM users WHERE username = $1 AND password_hash = $2")
            .bind(username)
            .bind(hashed_password)
            .fetch_optional(&self.db)
            .await?;

        Ok(user.map(|user| user.get("id")))
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
    pub async fn delete_api_token(&self, user_id: i64, api_token: &str) -> Result<()> {
        sqlx::query("DELETE FROM api_tokens WHERE user_id = $1 AND token = $2")
            .bind(user_id)
            .bind(api_token)
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
        let is_valid = sqlx::query("SELECT user_id FROM api_tokens WHERE token = $1")
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
    pub async fn store_api_token(&self, user_id: i64, api_token: &str) -> Result<()> {
        sqlx::query("INSERT INTO api_tokens (user_id, token) VALUES ($1, $2)")
            .bind(user_id)
            .bind(api_token)
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
    /// - `Result<Vec<String>>`: A result containing either:
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
    pub async fn get_api_tokens_for_user(&self, user_id: i64) -> Result<Vec<String>> {
        let tokens = sqlx::query("SELECT token FROM api_tokens WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(&self.db)
            .await?;

        Ok(tokens.into_iter().map(|row| row.get("token")).collect())
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
        let timestamp = timestamp
            .with_second(0)
            .and_then(|t| t.with_minute(0))
            .and_then(|t| t.with_nanosecond(0))
            .ok_or(AtomaStateManagerError::InvalidTimestamp)?;
        sqlx::query(
            "INSERT into stats_stacks (timestamp,num_compute_units) VALUES ($1,$2)
                 ON CONFLICT (timestamp) 
                 DO UPDATE SET 
                    num_compute_units = stats_stacks.num_compute_units + EXCLUDED.num_compute_units",
        )
        .bind(timestamp)
        .bind(stack.num_compute_units)
        .execute(&self.db)
        .await?;
        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum AtomaStateManagerError {
    #[error("Failed to connect to the database: {0}")]
    DatabaseConnectionError(#[from] sqlx::Error),
    #[error("Database url is malformed")]
    DatabaseUrlError,
    #[error("Stack not found")]
    StackNotFound,
    #[error("Attestation node not found: {0}")]
    AttestationNodeNotFound(i64),
    #[error("Invalid Merkle leaf length")]
    InvalidMerkleLeafLength,
    #[error("Invalid committed stack proof length")]
    InvalidCommittedStackProofLength,
    #[error("Failed to parse JSON: {0}")]
    JsonParseError(#[from] serde_json::Error),
    #[error("Failed to retrieve existing total hash for stack: `{0}`")]
    FailedToRetrieveExistingTotalHash(i64),
    #[error("Failed to send result to channel")]
    ChannelSendError,
    #[error("Invalid timestamp")]
    InvalidTimestamp,
    #[error("Failed to run migrations")]
    FailedToRunMigrations(#[from] sqlx::migrate::MigrateError),
    #[error("Failed to verify quote: `{0}`")]
    FailedToVerifyQuote(String),
    #[error("Failed to parse quote: `{0}`")]
    FailedToParseQuote(String),
    #[error("Unix time went backwards: `{0}`")]
    UnixTimeWentBackwards(String),
    #[error("Failed to retrieve collateral: `{0}`")]
    FailedToRetrieveCollateral(String),
    #[error("Failed to retrieve fmspc: `{0}`")]
    FailedToRetrieveFmspc(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    const POSTGRES_TEST_DB_URL: &str = "postgres://atoma:atoma@localhost:5432/atoma";

    async fn setup_test_db() -> AtomaState {
        AtomaState::new_from_url(POSTGRES_TEST_DB_URL)
            .await
            .unwrap()
    }

    async fn truncate_tables(db: &sqlx::PgPool) {
        // List all your tables here
        sqlx::query(
            "TRUNCATE TABLE 
                tasks,
                node_subscriptions,
                stacks,
                nodes,
                stack_settlement_tickets,
                stack_attestation_disputes,
                node_public_keys,
                users
            CASCADE",
        )
        .execute(db)
        .await
        .expect("Failed to truncate tables");
    }

    /// Helper function to create a test task
    async fn create_test_task(
        pool: &sqlx::PgPool,
        task_small_id: i64,
        model_name: &str,
        security_level: i32,
    ) -> sqlx::Result<()> {
        sqlx::query(
            "INSERT INTO tasks (task_small_id, task_id, role, model_name, is_deprecated, valid_until_epoch, security_level, minimum_reputation_score)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
        )
        .bind(task_small_id)
        .bind(Uuid::new_v4().to_string())
        .bind(1)
        .bind(model_name)
        .bind(false)
        .bind(1000i64)
        .bind(security_level)
        .bind(0i64)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Helper function to create a test node subscription
    async fn create_test_node_subscription(
        pool: &sqlx::PgPool,
        node_small_id: i64,
        task_small_id: i64,
        price: i64,
        max_units: i64,
    ) -> sqlx::Result<()> {
        sqlx::query(
            "INSERT INTO node_subscriptions (node_small_id, task_small_id, price_per_compute_unit, max_num_compute_units, valid)
             VALUES ($1, $2, $3, $4, $5)"
        )
        .bind(node_small_id)
        .bind(task_small_id)
        .bind(price)
        .bind(max_units)
        .bind(true)
        .execute(pool)
        .await?;
        Ok(())
    }

    async fn create_test_node(pool: &sqlx::PgPool, node_small_id: i64) -> sqlx::Result<()> {
        sqlx::query(
            "INSERT INTO nodes (node_small_id, sui_address, public_address, country) VALUES ($1, $2, $3, $4)"
        )
        .bind(node_small_id)
        .bind(Uuid::new_v4().to_string())
        .bind("test_public_address")
        .bind("test_country")
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Helper function to create a test node public key
    async fn create_test_node_public_key(
        pool: &sqlx::PgPool,
        node_small_id: i64,
        is_valid: bool,
    ) -> sqlx::Result<()> {
        sqlx::query(
            "INSERT INTO node_public_keys (node_small_id, public_key, is_valid, epoch, tee_remote_attestation_bytes)
             VALUES ($1, $2, $3, $4, $5)"
        )
        .bind(node_small_id)
        .bind(vec![0u8; 32]) // dummy public key
        .bind(is_valid)
        .bind(1i64)
        .bind(vec![0u8; 32]) // dummy attestation bytes
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Helper function to create a test stack
    async fn create_test_stack(
        pool: &sqlx::PgPool,
        task_small_id: i64,
        stack_small_id: i64,
        node_small_id: i64,
        price: i64,
        num_compute_units: i64,
        user_id: i64,
    ) -> sqlx::Result<()> {
        sqlx::query("INSERT INTO users (id, username, password_hash) VALUES ($1, $2, $3)")
            .bind(user_id)
            .bind(format!("test_user_{}", user_id)) // Create unique username
            .bind("test_password_hash") // Default password hash
            .execute(pool)
            .await?;
        sqlx::query(
            "INSERT INTO stacks (
                stack_small_id,
                owner,
                stack_id,
                task_small_id,
                selected_node_id,
                num_compute_units,
                price,
                already_computed_units,
                in_settle_period,
                total_hash,
                num_total_messages,
                user_id
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(stack_small_id)
        .bind("test_owner") // Default test owner
        .bind(Uuid::new_v4().to_string()) // Generate unique stack_id
        .bind(task_small_id)
        .bind(node_small_id)
        .bind(num_compute_units)
        .bind(price)
        .bind(0i64) // Default already_computed_units
        .bind(false) // Default in_settle_period
        .bind(vec![0u8; 32]) // Default total_hash (32 bytes of zeros)
        .bind(0i64) // Default num_total_messages
        .bind(user_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_get_cheapest_node_basic() {
        let state = setup_test_db().await;
        truncate_tables(&state.db).await;

        // Create test data
        create_test_task(&state.db, 1, "gpt-4", 1).await.unwrap();
        create_test_node(&state.db, 1).await.unwrap();
        create_test_node_subscription(&state.db, 1, 1, 100, 1000)
            .await
            .unwrap();
        create_test_stack(&state.db, 1, 1, 1, 100, 1, 1000)
            .await
            .unwrap();

        // Test basic functionality
        let result = state
            .get_cheapest_node_for_model("gpt-4", false)
            .await
            .unwrap();
        assert!(result.is_some());
        let node = result.unwrap();
        assert_eq!(node.task_small_id, 1);
        assert_eq!(node.price_per_compute_unit, 100);
        assert_eq!(node.max_num_compute_units, 1000);
        assert_eq!(node.node_small_id, 1);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_get_cheapest_node_multiple_prices() {
        let state = setup_test_db().await;
        truncate_tables(&state.db).await;

        // Create test data with multiple nodes at different prices
        create_test_task(&state.db, 1, "gpt-4", 1).await.unwrap();
        create_test_node(&state.db, 1).await.unwrap();
        create_test_node_subscription(&state.db, 1, 1, 100, 1000)
            .await
            .unwrap();
        create_test_stack(&state.db, 1, 1, 1, 100, 1000, 1)
            .await
            .unwrap();
        create_test_node(&state.db, 2).await.unwrap();
        create_test_node_subscription(&state.db, 2, 1, 50, 1000)
            .await
            .unwrap();
        create_test_stack(&state.db, 1, 2, 2, 50, 1000, 2)
            .await
            .unwrap();
        create_test_node(&state.db, 3).await.unwrap();
        create_test_node_subscription(&state.db, 3, 1, 150, 1000)
            .await
            .unwrap();
        create_test_stack(&state.db, 1, 3, 3, 150, 1000, 3)
            .await
            .unwrap();
        // Should return the cheapest node
        let result = state
            .get_cheapest_node_for_model("gpt-4", false)
            .await
            .unwrap();
        assert!(result.is_some());
        let node = result.unwrap();
        assert_eq!(node.price_per_compute_unit, 50);
        assert_eq!(node.node_small_id, 2);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_get_cheapest_node_confidential() {
        let state = setup_test_db().await;
        truncate_tables(&state.db).await;

        // Create test data for confidential computing
        create_test_task(&state.db, 1, "gpt-4", 2).await.unwrap();
        create_test_node(&state.db, 1).await.unwrap();
        create_test_node_subscription(&state.db, 1, 1, 100, 1000)
            .await
            .unwrap();
        create_test_node_public_key(&state.db, 1, true)
            .await
            .unwrap();
        create_test_stack(&state.db, 1, 1, 1, 100, 1, 1000)
            .await
            .unwrap();
        // Test confidential computing requirements
        let result = state
            .get_cheapest_node_for_model("gpt-4", true)
            .await
            .unwrap();
        assert!(result.is_some());
        let node = result.unwrap();
        assert_eq!(node.task_small_id, 1);
        assert_eq!(node.node_small_id, 1);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_get_cheapest_node_invalid_public_key() {
        let state = setup_test_db().await;
        truncate_tables(&state.db).await;

        // Create test data with invalid public key
        create_test_task(&state.db, 1, "gpt-4", 2).await.unwrap();
        create_test_node(&state.db, 1).await.unwrap();
        create_test_node_subscription(&state.db, 1, 1, 100, 1000)
            .await
            .unwrap();
        create_test_node_public_key(&state.db, 1, false)
            .await
            .unwrap();
        create_test_stack(&state.db, 1, 1, 1, 100, 1, 1000)
            .await
            .unwrap();

        // Should return None when public key is invalid
        let result = state
            .get_cheapest_node_for_model("gpt-4", true)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_get_cheapest_node_deprecated_task() {
        let state = setup_test_db().await;
        truncate_tables(&state.db).await;

        // Create deprecated task
        sqlx::query(
            "INSERT INTO tasks (task_small_id, task_id, role, model_name, is_deprecated, valid_until_epoch, security_level, minimum_reputation_score)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
        )
        .bind(1i64)
        .bind(Uuid::new_v4().to_string())
        .bind(1)
        .bind("gpt-4")
        .bind(true) // is_deprecated = true
        .bind(1000i64)
        .bind(1i32)
        .bind(0i64)
        .execute(&state.db)
        .await
        .unwrap();

        create_test_node(&state.db, 1).await.unwrap();
        create_test_node_subscription(&state.db, 1, 1, 100, 1000)
            .await
            .unwrap();
        create_test_stack(&state.db, 1, 1, 1, 100, 1, 1000)
            .await
            .unwrap();
        // Should return None for deprecated task
        let result = state
            .get_cheapest_node_for_model("gpt-4", false)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_get_cheapest_node_invalid_subscription() {
        let state = setup_test_db().await;
        truncate_tables(&state.db).await;

        // Create test data with invalid subscription
        create_test_task(&state.db, 1, "gpt-4", 1).await.unwrap();
        create_test_node(&state.db, 1).await.unwrap();

        // Create invalid subscription (valid = false)
        sqlx::query(
            "INSERT INTO node_subscriptions (node_small_id, task_small_id, price_per_compute_unit, max_num_compute_units, valid)
             VALUES ($1, $2, $3, $4, $5)"
        )
        .bind(1i64)
        .bind(1i64)
        .bind(100i64)
        .bind(1000i64)
        .bind(false)
        .execute(&state.db)
        .await
        .unwrap();

        create_test_stack(&state.db, 1, 1, 1, 100, 1000, 1)
            .await
            .unwrap();

        // Should return None for invalid subscription
        let result = state
            .get_cheapest_node_for_model("gpt-4", false)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_get_cheapest_node_nonexistent_model() {
        let state = setup_test_db().await;
        truncate_tables(&state.db).await;

        // Test with non-existent model
        let result = state
            .get_cheapest_node_for_model("nonexistent-model", false)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_get_cheapest_node_mixed_security_levels() {
        let state = setup_test_db().await;
        truncate_tables(&state.db).await;

        // Create tasks with different security levels
        create_test_task(&state.db, 1, "gpt-4", 1).await.unwrap();
        create_test_task(&state.db, 2, "gpt-4", 2).await.unwrap();

        create_test_node(&state.db, 1).await.unwrap();
        create_test_node(&state.db, 2).await.unwrap();

        create_test_node_subscription(&state.db, 1, 1, 100, 1000)
            .await
            .unwrap();
        create_test_node_subscription(&state.db, 2, 2, 50, 1000)
            .await
            .unwrap();
        create_test_node_public_key(&state.db, 2, true)
            .await
            .unwrap();
        create_test_stack(&state.db, 1, 1, 1, 100, 1000, 1)
            .await
            .unwrap();
        create_test_stack(&state.db, 1, 2, 1, 50, 1000, 2)
            .await
            .unwrap();

        // Test non-confidential query (should return cheapest regardless of security level)
        let result = state
            .get_cheapest_node_for_model("gpt-4", false)
            .await
            .unwrap();
        assert!(result.is_some());
        let node = result.unwrap();
        assert_eq!(node.price_per_compute_unit, 50);

        // Test confidential query (should only return security level 2)
        let result = state
            .get_cheapest_node_for_model("gpt-4", true)
            .await
            .unwrap();
        assert!(result.is_some());
        let node = result.unwrap();
        assert_eq!(node.task_small_id, 2);
    }

    async fn setup_test_environment() -> Result<AtomaState> {
        let state = setup_test_db().await;
        truncate_tables(&state.db).await;

        // Create base task with security level 2 (confidential)
        create_test_task(&state.db, 1, "gpt-4", 2).await?;

        Ok(state)
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_basic_selection() -> Result<()> {
        let state = setup_test_environment().await?;

        // Setup single valid node
        create_test_node(&state.db, 1).await?;
        create_test_node_subscription(&state.db, 1, 1, 100, 1000).await?;
        create_test_node_public_key(&state.db, 1, true).await?;
        create_test_stack(&state.db, 1, 1, 1, 100, 1000, 1)
            .await
            .unwrap();
        let result = state
            .select_node_public_key_for_encryption("gpt-4", 800)
            .await?;

        assert!(result.is_some());
        assert_eq!(result.unwrap().node_small_id, 1);

        Ok(())
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_price_based_selection() -> Result<()> {
        let state = setup_test_environment().await?;

        // Setup nodes with different prices
        for (node_id, price) in [(1, 200), (2, 100), (3, 300)] {
            create_test_node(&state.db, node_id).await?;
            create_test_node_subscription(&state.db, node_id, 1, price, 1000).await?;
            create_test_node_public_key(&state.db, node_id, true).await?;
            create_test_stack(&state.db, 1, node_id, node_id, price, 1000, node_id)
                .await
                .unwrap();
        }

        let result = state
            .select_node_public_key_for_encryption("gpt-4", 800)
            .await?;

        assert!(result.is_some());
        assert_eq!(
            result.unwrap().node_small_id,
            2,
            "Should select cheapest node"
        );

        Ok(())
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_compute_capacity_requirements() -> Result<()> {
        let state = setup_test_environment().await?;

        // Setup nodes with different compute capacities
        create_test_node(&state.db, 1).await?;
        create_test_node_subscription(&state.db, 1, 1, 100, 500).await?; // Insufficient capacity
        create_test_node_public_key(&state.db, 1, true).await?;
        create_test_stack(&state.db, 1, 1, 1, 100, 500, 1)
            .await
            .unwrap();

        let result = state
            .select_node_public_key_for_encryption("gpt-4", 800)
            .await?;
        assert!(
            result.is_none(),
            "Should not select node with insufficient capacity"
        );

        // Add node with sufficient capacity
        create_test_node(&state.db, 2).await?;
        create_test_node_subscription(&state.db, 2, 1, 100, 1000).await?;
        create_test_node_public_key(&state.db, 2, true).await?;
        create_test_stack(&state.db, 1, 2, 2, 100, 1000, 2)
            .await
            .unwrap();

        let result = state
            .select_node_public_key_for_encryption("gpt-4", 800)
            .await?;
        assert_eq!(
            result.unwrap().node_small_id,
            2,
            "Should select node with sufficient capacity"
        );

        Ok(())
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_invalid_configurations() -> Result<()> {
        let state = setup_test_environment().await?;

        // Setup node with invalid public key
        create_test_node(&state.db, 1).await?;
        create_test_node_subscription(&state.db, 1, 1, 100, 1000).await?;
        create_test_node_public_key(&state.db, 1, false).await?;
        create_test_stack(&state.db, 1, 1, 1, 100, 1000, 1)
            .await
            .unwrap();

        let result = state
            .select_node_public_key_for_encryption("gpt-4", 800)
            .await?;
        assert!(
            result.is_none(),
            "Should not select node with invalid public key"
        );

        // Test non-existent model
        let result = state
            .select_node_public_key_for_encryption("nonexistent-model", 800)
            .await?;
        assert!(
            result.is_none(),
            "Should handle non-existent model gracefully"
        );

        Ok(())
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_security_level_requirement() -> Result<()> {
        let state = setup_test_environment().await?;

        // Create task with security level 1 (non-confidential)
        create_test_task(&state.db, 2, "gpt-4", 1).await?;

        // Setup valid node subscribed to non-confidential task
        create_test_node(&state.db, 1).await?;
        create_test_node_subscription(&state.db, 1, 2, 100, 1000).await?;
        create_test_node_public_key(&state.db, 1, true).await?;
        create_test_stack(&state.db, 2, 1, 1, 100, 1000, 1)
            .await
            .unwrap();
        let result = state
            .select_node_public_key_for_encryption("gpt-4", 800)
            .await?;
        assert!(
            result.is_none(),
            "Should not select node for non-confidential task"
        );

        Ok(())
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_edge_cases() -> Result<()> {
        let state = setup_test_environment().await?;

        create_test_node(&state.db, 1).await?;
        create_test_node_subscription(&state.db, 1, 1, 100, 1000).await?;
        create_test_node_public_key(&state.db, 1, true).await?;
        create_test_stack(&state.db, 1, 1, 1, 100, 1000, 1)
            .await
            .unwrap();

        // Test edge cases
        let test_cases = vec![
            (0, true, "zero tokens"),
            (1000, true, "exact capacity match"),
            (1001, false, "just over capacity"),
            (-1, true, "negative tokens"),
        ];

        for (tokens, should_succeed, case) in test_cases {
            let result = state
                .select_node_public_key_for_encryption("gpt-4", tokens)
                .await?;
            assert_eq!(
                result.is_some(),
                should_succeed,
                "Failed edge case: {} with {} tokens",
                case,
                tokens
            );
        }

        Ok(())
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_concurrent_access() -> Result<()> {
        let state = setup_test_environment().await?;

        create_test_node(&state.db, 1).await?;
        create_test_node_subscription(&state.db, 1, 1, 100, 1000).await?;
        create_test_node_public_key(&state.db, 1, true).await?;
        create_test_stack(&state.db, 1, 1, 1, 100, 1000, 1)
            .await
            .unwrap();
        let futures: Vec<_> = (0..5)
            .map(|_| state.select_node_public_key_for_encryption("gpt-4", 800))
            .collect();

        let results = futures::future::join_all(futures).await;

        for result in results {
            let node = result?;
            assert!(node.is_some(), "Concurrent access failed to retrieve node");
            assert_eq!(node.unwrap().node_small_id, 1);
        }

        Ok(())
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_select_node_public_key_for_encryption_for_node() -> Result<()> {
        let state = setup_test_db().await;
        truncate_tables(&state.db).await;

        // Create test node
        create_test_node(&state.db, 1).await?;

        // Insert test node public key
        sqlx::query(
            r#"INSERT INTO node_public_keys (node_small_id, epoch, public_key, tee_remote_attestation_bytes, is_valid) 
               VALUES ($1, $2, $3, $4, $5)"#,
        )
        .bind(1i64)
        .bind(1i64)
        .bind(vec![1u8, 2, 3, 4]) // Example public key bytes
        .bind(vec![1u8, 2, 3, 4]) // Example tee remote attestation bytes
        .bind(true)
        .execute(&state.db)
        .await?;

        // Test successful retrieval
        let result = state
            .select_node_public_key_for_encryption_for_node(1)
            .await?;
        assert!(result.is_some());
        let node_key = result.unwrap();
        assert_eq!(node_key.node_small_id, 1);
        assert_eq!(node_key.public_key, vec![1, 2, 3, 4]);

        // Test non-existent node
        let result = state
            .select_node_public_key_for_encryption_for_node(999)
            .await?;
        assert!(result.is_none());

        Ok(())
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_select_node_public_key_for_encryption_for_node_multiple_keys() -> Result<()> {
        let state = setup_test_db().await;
        truncate_tables(&state.db).await;

        // Insert multiple test node public keys
        for i in 1..=3 {
            // Create test node
            create_test_node(&state.db, i).await?;
            sqlx::query(
                r#"INSERT INTO node_public_keys (node_small_id, epoch, public_key, tee_remote_attestation_bytes, is_valid) 
                   VALUES ($1, $2, $3, $4, $5)"#,
            )
            .bind(i)
            .bind(i)
            .bind(vec![i as u8; 4]) // Different key for each node
            .bind(vec![i as u8; 4]) // Example tee remote attestation bytes
            .bind(true)
            .execute(&state.db)
            .await?;
        }

        // Test retrieval of each key
        for i in 1..=3 {
            let result = state
                .select_node_public_key_for_encryption_for_node(i)
                .await?;
            assert!(result.is_some());
            let node_key = result.unwrap();
            assert_eq!(node_key.node_small_id, i);
            assert_eq!(node_key.public_key, vec![i as u8; 4]);
        }

        Ok(())
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_select_node_public_key_for_encryption_for_node_invalid_data() -> Result<()> {
        let state = setup_test_db().await;
        truncate_tables(&state.db).await;

        // Create test node
        create_test_node(&state.db, 1).await?;

        // Test with negative node_small_id
        let result = state
            .select_node_public_key_for_encryption_for_node(-1)
            .await?;
        assert!(result.is_none());

        // Insert invalid public key (empty)
        sqlx::query(
            r#"INSERT INTO node_public_keys (node_small_id, epoch, public_key, tee_remote_attestation_bytes, is_valid) 
               VALUES ($1, $2, $3, $4, $5)"#,
        )
        .bind(1i64)
        .bind(1i64)
        .bind(Vec::<u8>::new()) // Empty public key
        .bind(vec![1u8, 2, 3, 4]) // Example tee remote attestation bytes
        .bind(true)
        .execute(&state.db)
        .await?;

        // Should still return the key, even if empty
        let result = state
            .select_node_public_key_for_encryption_for_node(1)
            .await?;
        assert!(result.is_some());
        let node_key = result.unwrap();
        assert_eq!(node_key.node_small_id, 1);
        assert!(node_key.public_key.is_empty());

        Ok(())
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_select_node_public_key_for_encryption_for_node_concurrent_access() -> Result<()> {
        let state = setup_test_db().await;
        truncate_tables(&state.db).await;

        // Create test node
        create_test_node(&state.db, 1).await?;

        // Insert test data
        sqlx::query(
            r#"INSERT INTO node_public_keys (node_small_id, epoch, public_key, tee_remote_attestation_bytes, is_valid) 
               VALUES ($1, $2, $3, $4, $5)"#,
        )
        .bind(1i64)
        .bind(1i64)
        .bind(vec![1u8, 2, 3, 4]) // Example public key bytes
        .bind(vec![1u8, 2, 3, 4]) // Example tee remote attestation bytes
        .bind(true)
        .execute(&state.db)
        .await?;

        // Create multiple concurrent requests
        let futures: Vec<_> = (0..10)
            .map(|_| {
                let state = state.clone();
                tokio::spawn(async move {
                    state
                        .select_node_public_key_for_encryption_for_node(1)
                        .await
                })
            })
            .collect();

        // All requests should complete successfully
        for future in futures {
            let result = future.await.unwrap().unwrap();
            assert!(result.is_some());
            let node_key = result.unwrap();
            assert_eq!(node_key.node_small_id, 1);
            assert_eq!(node_key.public_key, vec![1, 2, 3, 4]);
        }

        Ok(())
    }
}
