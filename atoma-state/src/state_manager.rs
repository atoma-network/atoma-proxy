use crate::build_query_with_in;
use crate::handlers::{handle_atoma_event, handle_state_manager_event};
use crate::types::{
    AtomaAtomaStateManagerEvent, NodeSubscription, Stack, StackAttestationDispute,
    StackSettlementTicket, Task,
};

use atoma_sui::events::AtomaEvent;
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
}

impl AtomaStateManager {
    /// Constructor
    pub fn new(
        db: PgPool,
        event_subscriber_receiver: FlumeReceiver<AtomaEvent>,
        state_manager_receiver: FlumeReceiver<AtomaAtomaStateManagerEvent>,
    ) -> Self {
        Self {
            state: AtomaState::new(db),
            event_subscriber_receiver,
            state_manager_receiver,
        }
    }

    /// Creates a new `AtomaStateManager` instance from a database URL.
    ///
    /// This method establishes a connection to the Postgres database using the provided URL,
    /// creates all necessary tables in the database, and returns a new `AtomaStateManager` instance.
    pub async fn new_from_url(
        database_url: String,
        event_subscriber_receiver: FlumeReceiver<AtomaEvent>,
        state_manager_receiver: FlumeReceiver<AtomaAtomaStateManagerEvent>,
    ) -> Result<Self> {
        let db = PgPool::connect(&database_url).await?;
        queries::create_all_tables(&db).await?;
        Ok(Self {
            state: AtomaState::new(db),
            event_subscriber_receiver,
            state_manager_receiver,
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
                            handle_atoma_event(atoma_event, &self).await?;
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
                            handle_state_manager_event(&self, state_manager_event).await?;
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
    pub async fn new_from_url(database_url: String) -> Result<Self> {
        let db = PgPool::connect(&database_url).await?;
        queries::create_all_tables(&db).await?;
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
    pub async fn get_stacks_for_model(&self, model: &str, free_units: i64) -> Result<Vec<Stack>> {
        // TODO: filter also by security level and other constraints
        let tasks = sqlx::query(
            "WITH selected_stack AS (
                SELECT stacks.stack_small_id
                FROM stacks
                INNER JOIN tasks ON tasks.task_small_id = stacks.task_small_id
                WHERE tasks.model_name = $1
                AND stacks.num_compute_units - stacks.already_computed_units >= $2
                LIMIT 1
            )
            UPDATE stacks
            SET already_computed_units = already_computed_units + $2
            WHERE stack_small_id IN (SELECT stack_small_id FROM selected_stack)
            RETURNING stacks.*",
        )
        .bind(model)
        .bind(free_units)
        .fetch_all(&self.db)
        .await?;
        tasks
            .into_iter()
            .map(|task| Stack::from_row(&task).map_err(AtomaStateManagerError::from))
            .collect()
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

    /// Retrieves all node subscriptions for a given set of node IDs.
    ///
    /// This method fetches all subscription records from the `node_subscriptions` table
    /// that match any of the provided node IDs.
    ///
    /// # Arguments
    ///
    /// * `node_small_ids` - A slice of node IDs to fetch subscriptions for.
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
    ///     let node_ids = vec![1, 2, 3];
    ///     state_manager.get_all_node_subscriptions(&node_ids).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(?node_small_ids))]
    pub async fn get_all_node_subscriptions(
        &self,
        node_small_ids: &[i64],
    ) -> Result<Vec<NodeSubscription>> {
        let mut query_builder = build_query_with_in(
            "SELECT * FROM node_subscriptions",
            "node_small_id",
            node_small_ids,
            None,
        );

        let subscriptions = query_builder.build().fetch_all(&self.db).await?;

        subscriptions
            .into_iter()
            .map(|subscription| {
                NodeSubscription::from_row(&subscription).map_err(AtomaStateManagerError::from)
            })
            .collect()
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
        sqlx::query("UPDATE node_subscriptions SET valid = FALSE WHERE node_small_id = $1 AND task_small_id = $2")
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
    pub async fn insert_new_stack(&self, stack: Stack) -> Result<()> {
        sqlx::query(
            "INSERT INTO stacks 
                (owner, stack_small_id, stack_id, task_small_id, selected_node_id, num_compute_units, price, already_computed_units, in_settle_period, total_hash, num_total_messages) 
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
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

        // Also update the stack to set in_settle_period to true
        sqlx::query("UPDATE stacks SET in_settle_period = true WHERE stack_small_id = $1")
            .bind(stack_settlement_ticket.stack_small_id)
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
    /// This method inserts or updates the public address of a node in the `node_public_addresses` table.
    ///
    /// # Arguments
    ///
    /// * `small_id` - The unique small identifier of the node.
    /// * `address` - The public address of the node.
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
    /// async fn update_address(state_manager: &AtomaStateManager, small_id: i64, address: String) -> Result<(), AtomaStateManagerError> {
    ///    state_manager.update_node_public_address(small_id, address).await
    /// }
    /// ```
    #[instrument(level = "trace", skip_all, fields(%small_id, %address))]
    pub async fn update_node_public_address(&self, small_id: i64, address: String) -> Result<()> {
        sqlx::query(
            "INSERT INTO node_public_addresses 
                    (node_small_id, public_address) 
                    VALUES ($1, $2) 
                    ON CONFLICT (node_small_id)
                    DO UPDATE SET public_address = EXCLUDED.public_address",
        )
        .bind(small_id)
        .bind(address)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Retrieves the public address of a node from the database.
    ///
    /// This method fetches the public address of a node from the `node_public_addresses` table.
    ///
    /// # Arguments
    ///
    /// * `small_id` - The unique small identifier of the node.
    ///
    /// # Returns
    ///
    /// - `Result<Option<String>>`: A result containing either:
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
        let address = sqlx::query_scalar(
            "SELECT public_address FROM node_public_addresses WHERE node_small_id = $1",
        )
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
}

pub(crate) mod queries {
    use super::*;

    /// Generates the SQL query to create the `tasks` table.
    ///
    /// This table stores information about tasks in the system.
    ///
    /// # Table Structure
    /// - `task_small_id`: INTEGER PRIMARY KEY - The unique identifier for the task.
    /// - `task_id`: TEXT UNIQUE NOT NULL - A unique text identifier for the task.
    /// - `role`: INTEGER NOT NULL - The role associated with the task.
    /// - `model_name`: TEXT - The name of the model used for the task (nullable).
    /// - `is_deprecated`: BOOLEAN NOT NULL - Indicates whether the task is deprecated.
    /// - `valid_until_epoch`: INTEGER - The epoch until which the task is valid (nullable).
    /// - `deprecated_at_epoch`: INTEGER - The epoch when the task was deprecated (nullable).
    /// - `optimizations`: TEXT NOT NULL - A string representing task optimizations.
    /// - `security_level`: INTEGER NOT NULL - The security level of the task.
    /// - `task_metrics_compute_unit`: INTEGER NOT NULL - The compute unit metric for the task.
    /// - `task_metrics_time_unit`: INTEGER - The time unit metric for the task (nullable).
    /// - `task_metrics_value`: INTEGER - The value metric for the task (nullable).
    /// - `minimum_reputation_score`: INTEGER - The minimum reputation score required (nullable).
    ///
    /// # Primary Key
    /// The table uses `task_small_id` as the primary key.
    ///
    /// # Unique Constraint
    /// The `task_id` field has a unique constraint to ensure no duplicate task IDs.
    ///
    /// # Returns
    /// A `String` containing the SQL query to create the `tasks` table.
    #[instrument(level = "trace", skip_all)]
    pub(crate) async fn create_tasks(db: &PgPool) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS tasks (
                task_small_id BIGINT PRIMARY KEY,
                task_id TEXT UNIQUE NOT NULL,
                role SMALLINT NOT NULL,
                model_name TEXT,
                is_deprecated BOOLEAN NOT NULL,
                valid_until_epoch BIGINT,
                deprecated_at_epoch BIGINT,
                security_level INTEGER NOT NULL,
                minimum_reputation_score SMALLINT
            )",
        )
        .execute(db)
        .await?;
        Ok(())
    }

    /// Generates the SQL query to create the `node_subscriptions` table.
    ///
    /// This table stores information about node subscriptions to tasks.
    ///
    /// # Table Structure
    /// - `task_small_id`: BIGINT NOT NULL - The ID of the task being subscribed to.
    /// - `node_small_id`: BIGINT NOT NULL - The ID of the node subscribing to the task.
    /// - `price_per_compute_unit`: INTEGER NOT NULL - The price per compute unit for this subscription.
    ///
    /// # Primary Key
    /// The table uses a composite primary key of (task_small_id, node_small_id).
    ///
    /// # Foreign Key
    /// - `task_small_id` references the `tasks` table.
    ///
    /// # Returns
    /// A `String` containing the SQL query to create the `node_subscriptions` table.
    #[instrument(level = "trace", skip_all)]
    pub(crate) async fn create_subscribed_tasks(db: &PgPool) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS node_subscriptions (
            task_small_id BIGINT NOT NULL,
            node_small_id BIGINT NOT NULL,
            price_per_compute_unit BIGINT NOT NULL,
            max_num_compute_units BIGINT NOT NULL,
            valid BOOLEAN NOT NULL,
            PRIMARY KEY (task_small_id, node_small_id),
            FOREIGN KEY (task_small_id) REFERENCES tasks (task_small_id)
        );",
        )
        .execute(db)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_node_subscriptions_task_small_id_node_small_id ON node_subscriptions (task_small_id, node_small_id);").execute(db).await?;
        Ok(())
    }

    /// Generates the SQL query to create the `stacks` table.
    ///
    /// This table stores information about stacks in the system.
    ///
    /// # Table Structure
    /// - `stack_small_id`: INTEGER PRIMARY KEY - The unique identifier for the stack.
    /// - `stack_id`: TEXT UNIQUE NOT NULL - A unique text identifier for the stack.
    /// - `selected_node_id`: INTEGER NOT NULL - The ID of the node selected for this stack.
    /// - `num_compute_units`: INTEGER NOT NULL - The number of compute units allocated to this stack.
    /// - `price`: INTEGER NOT NULL - The price associated with this stack.
    ///
    /// # Primary Key
    /// The table uses `stack_small_id` as the primary key.
    ///
    /// # Foreign Keys
    /// - `selected_node_id` references the `node_subscriptions` table.
    /// - `task_small_id` references the `tasks` table.
    /// # Unique Constraint
    /// The `stack_id` field has a unique constraint to ensure no duplicate stack IDs.
    ///
    /// # Returns
    /// A `String` containing the SQL query to create the `stacks` table.
    #[instrument(level = "trace", skip_all)]
    pub(crate) async fn create_stacks(db: &PgPool) -> Result<()> {
        sqlx::query("CREATE TABLE IF NOT EXISTS stacks (
                stack_small_id BIGINT PRIMARY KEY,
                owner TEXT NOT NULL,
                stack_id TEXT UNIQUE NOT NULL,
                task_small_id BIGINT NOT NULL,
                selected_node_id BIGINT NOT NULL,
                num_compute_units BIGINT NOT NULL,
                price BIGINT NOT NULL,
                already_computed_units BIGINT NOT NULL,
                in_settle_period BOOLEAN NOT NULL,
                total_hash BYTEA NOT NULL,
                num_total_messages BIGINT NOT NULL,
                FOREIGN KEY (task_small_id) REFERENCES tasks (task_small_id),
                FOREIGN KEY (selected_node_id, task_small_id) REFERENCES node_subscriptions (node_small_id, task_small_id)
            );").execute(db).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_stacks_owner_address ON stacks (owner);")
            .execute(db)
            .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_stacks_task_small_id ON stacks (task_small_id);",
        )
        .execute(db)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_stacks_stack_small_id ON stacks (stack_small_id);",
        )
        .execute(db)
        .await?;
        Ok(())
    }

    /// Generates the SQL query to create the `stack_settlement_tickets` table.
    ///
    /// This table stores information about settlement tickets for stacks.
    ///
    /// # Table Structure
    /// - `stack_small_id`: INTEGER PRIMARY KEY - The unique identifier for the stack.
    /// - `selected_node_id`: INTEGER NOT NULL - The ID of the node selected for settlement.
    /// - `num_claimed_compute_units`: INTEGER NOT NULL - The number of compute units claimed.
    /// - `requested_attestation_nodes`: TEXT NOT NULL - A list of nodes requested for attestation.
    /// - `committed_stack_proofs`: BYTEA NOT NULL - The committed proofs for the stack settlement.
    /// - `stack_merkle_leaves`: BYTEA NOT NULL - The Merkle leaves for the stack.
    /// - `dispute_settled_at_epoch`: INTEGER - The epoch when the dispute was settled (nullable).
    /// - `already_attested_nodes`: TEXT NOT NULL - A list of nodes that have already attested.
    /// - `is_in_dispute`: BOOLEAN NOT NULL - Indicates whether the settlement is in dispute.
    /// - `user_refund_amount`: INTEGER NOT NULL - The amount to be refunded to the user.
    /// - `is_claimed`: BOOLEAN NOT NULL - Indicates whether the settlement has been claimed.
    ///
    /// # Primary Key
    /// The table uses `stack_small_id` as the primary key.
    ///
    /// # Returns
    /// A `String` containing the SQL query to create the `stack_settlement_tickets` table.
    ///
    /// # Foreign Key
    /// - `stack_small_id` references the `stacks` table.
    ///
    /// # Returns
    /// A `String` containing the SQL query to create the `stack_settlement_tickets` table.
    #[instrument(level = "trace", skip_all)]
    pub(crate) async fn create_stack_settlement_tickets(db: &PgPool) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS stack_settlement_tickets (
            stack_small_id BIGINT PRIMARY KEY,
            selected_node_id INTEGER NOT NULL,
            num_claimed_compute_units INTEGER NOT NULL,
            requested_attestation_nodes TEXT NOT NULL,
            committed_stack_proofs BYTEA NOT NULL,
            stack_merkle_leaves BYTEA NOT NULL,
            dispute_settled_at_epoch INTEGER,
            already_attested_nodes TEXT NOT NULL,
            is_in_dispute BOOLEAN NOT NULL,
            user_refund_amount INTEGER NOT NULL,
            is_claimed BOOLEAN NOT NULL
        );",
        )
        .execute(db)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_stack_settlement_tickets_stack_small_id ON stack_settlement_tickets (stack_small_id);").execute(db).await?;
        Ok(())
    }

    /// Generates the SQL query to create the `stack_attestation_disputes` table.
    ///
    /// This table stores information about disputes related to stack attestations.
    ///
    /// # Table Structure
    /// - `stack_small_id`: INTEGER NOT NULL - The ID of the stack involved in the dispute.
    /// - `attestation_commitment`: BYTEA NOT NULL - The commitment provided by the attesting node.
    /// - `attestation_node_id`: INTEGER NOT NULL - The ID of the node providing the attestation.
    /// - `original_node_id`: INTEGER NOT NULL - The ID of the original node involved in the dispute.
    /// - `original_commitment`: BYTEA NOT NULL - The original commitment that is being disputed.
    ///
    /// # Primary Key
    /// The table uses a composite primary key of (stack_small_id, attestation_node_id).
    ///
    /// # Foreign Key
    /// - `stack_small_id` references the `stacks` table.
    ///
    /// # Returns
    /// A `String` containing the SQL query to create the `stack_attestation_disputes` table.
    #[instrument(level = "trace", skip_all)]
    pub(crate) async fn create_stack_attestation_disputes(db: &PgPool) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS stack_attestation_disputes (
                stack_small_id BIGINT NOT NULL,
                attestation_commitment BYTEA NOT NULL,
                attestation_node_id INTEGER NOT NULL,
                original_node_id INTEGER NOT NULL,
                original_commitment BYTEA NOT NULL,
                PRIMARY KEY (stack_small_id, attestation_node_id),
                FOREIGN KEY (stack_small_id) REFERENCES stacks (stack_small_id)
            )",
        )
        .execute(db)
        .await?;
        Ok(())
    }

    /// Creates the `nodes_public_addresses` table in the database.
    ///
    /// This table stores the public addresses of nodes in the system.
    /// - `node_small_id`: INTEGER PRIMARY KEY - The unique identifier for the node.
    /// - `public_address`: TEXT NOT NULL - The public address of the node.
    ///
    /// # Arguments
    ///
    /// * `db` - A reference to the Postgres database pool.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the table is created successfully, or an error if any operation fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if the SQL query fails to execute.
    /// Possible reasons for failure include:
    /// - Database connection issues
    /// - Insufficient permissions
    /// - Syntax errors in the SQL query
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use sqlx::PgPool;
    /// use atoma_node::atoma_state::queries;
    ///
    /// async fn setup_database(pool: &PgPool) -> Result<(), Box<dyn std::error::Error>> {
    ///     queries::create_nodes_public_addresses(pool).await?;
    ///     Ok(())
    /// }
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub(crate) async fn create_nodes_public_addresses(db: &PgPool) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS node_public_addresses (
                node_small_id BIGINT PRIMARY KEY,
                public_address TEXT NOT NULL
            )",
        )
        .execute(db)
        .await?;
        Ok(())
    }

    /// Creates the `node_throughput_performance` and `node_latency_performance` tables in the database.
    ///
    /// This function creates two tables in the database:
    /// - `node_throughput_performance`: Stores the throughput performance of nodes.
    /// - `node_latency_performance`: Stores the latency performance of nodes.
    ///
    /// # Arguments
    ///
    /// * `db` - A reference to the Postgres database pool.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the tables are created successfully, or an error if any operation fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if the SQL queries fail to execute.
    /// Possible reasons for failure include:
    /// - Database connection issues
    /// - Insufficient permissions
    /// - Syntax errors in the SQL queries
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use sqlx::PgPool;
    /// use atoma_node::atoma_state::queries;
    ///
    /// async fn setup_database(pool: &PgPool) -> Result<(), Box<dyn std::error::Error>> {
    ///     queries::create_nodes_performance(pool).await?;
    ///     Ok(())
    /// }
    /// ```
    pub(crate) async fn create_nodes_performance(db: &PgPool) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS node_throughput_performance (
                node_small_id BIGINT PRIMARY KEY,
                queries BIGINT NOT NULL,
                input_tokens BIGINT NOT NULL,
                output_tokens BIGINT NOT NULL,
                time DOUBLE PRECISION NOT NULL,
                FOREIGN KEY (node_small_id) REFERENCES node_public_addresses (node_small_id)
            )",
        )
        .execute(db)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS node_latency_performance (
                node_small_id BIGINT PRIMARY KEY,
                queries BIGINT NOT NULL,
                latency DOUBLE PRECISION NOT NULL,
                FOREIGN KEY (node_small_id) REFERENCES node_public_addresses (node_small_id)
            )",
        )
        .execute(db)
        .await?;
        Ok(())
    }

    /// Creates all the necessary tables in the database.
    ///
    /// This function executes SQL queries to create the following tables:
    /// - tasks
    /// - node_subscriptions
    /// - stacks
    /// - stack_settlement_tickets
    /// - stack_attestation_disputes
    ///
    /// # Arguments
    ///
    /// * `db` - A reference to the Postgres database pool.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if all tables are created successfully, or an error if any operation fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if any of the SQL queries fail to execute.
    /// Possible reasons for failure include:
    /// - Database connection issues
    /// - Insufficient permissions
    /// - Syntax errors in the SQL queries
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use sqlx::PgPool;
    /// use atoma_node::atoma_state::queries;
    ///
    /// async fn setup_database(pool: &PgPool) -> Result<(), Box<dyn std::error::Error>> {
    ///     queries::create_all_tables(pool).await?;
    ///     Ok(())
    /// }
    /// ```
    #[instrument(level = "trace", skip_all)]
    pub(crate) async fn create_all_tables(db: &PgPool) -> Result<()> {
        create_tasks(db).await?;
        create_subscribed_tasks(db).await?;
        create_stacks(db).await?;
        create_stack_settlement_tickets(db).await?;
        create_stack_attestation_disputes(db).await?;
        create_nodes_public_addresses(db).await?;
        create_nodes_performance(db).await?;

        Ok(())
    }
}
