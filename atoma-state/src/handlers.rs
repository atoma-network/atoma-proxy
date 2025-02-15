use atoma_p2p::{metrics::NodeMetrics, AtomaP2pEvent};
use atoma_sui::events::{
    AtomaEvent, NewKeyRotationEvent, NewStackSettlementAttestationEvent,
    NodePublicKeyCommittmentEvent, NodeRegisteredEvent, NodeSubscribedToTaskEvent,
    NodeSubscriptionUpdatedEvent, NodeUnsubscribedFromTaskEvent, StackAttestationDisputeEvent,
    StackCreatedEvent, StackSettlementTicketClaimedEvent, StackSettlementTicketEvent,
    StackTrySettleEvent, TaskDeprecationEvent, TaskRegisteredEvent,
};
use chrono::{DateTime, Utc};
use tokio::sync::oneshot;
use tracing::{error, info, instrument, trace};

use crate::{
    errors::validate_gpu_metrics,
    state_manager::Result,
    timestamp_to_datetime_or_now,
    types::{AtomaAtomaStateManagerEvent, Stack, StackSettlementTicket},
    AtomaStateManager, AtomaStateManagerError,
};

/// Handles various Atoma network events by delegating them to appropriate handler functions.
///
/// This function serves as the main event handler for the Atoma network, processing different types of events
/// including task management, node subscriptions, stack operations, key rotations, and various system events.
///
/// # Arguments
///
/// * `event` - An `AtomaEvent` enum representing the event to be processed
/// * `state_manager` - A reference to the `AtomaStateManager` that provides access to the state database
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the event was processed successfully, or an error if something went wrong
///
/// # Event Categories
///
/// ## Task Management
/// - `TaskRegisteredEvent` - Handles registration of new tasks
/// - `TaskDeprecationEvent` - Processes task deprecation
/// - `TaskRemovedEvent` - Logs task removal
///
/// ## Node Operations
/// - `NodeSubscribedToTaskEvent` - Manages node task subscriptions
/// - `NodeSubscriptionUpdatedEvent` - Handles subscription updates
/// - `NodeUnsubscribedFromTaskEvent` - Processes task unsubscriptions
/// - `NodeRegisteredEvent` - Handles node registration
/// - `NodePublicKeyCommittmentEvent` - Manages node key rotations
///
/// ## Stack Operations
/// - `StackCreatedEvent` - Processes new stack creation
/// - `StackTrySettleEvent` - Handles stack settlement attempts
/// - `StackSettlementTicketEvent` - Manages settlement tickets
/// - `StackAttestationDisputeEvent` - Handles attestation disputes
///
/// ## System Events
/// - `NewKeyRotationEvent` - Processes system key rotations
/// - `PublishedEvent` - Logs published events
/// - `DisputeEvent` - Logs dispute events
/// - `SettledEvent` - Logs settlement events
///
/// ## AI Model Events
/// - `Text2ImagePromptEvent` - Logs text-to-image prompts
/// - `Text2TextPromptEvent` - Logs text-to-text prompts
///
/// # Examples
///
/// ```rust,ignore
/// use atoma_state::{AtomaStateManager, Result};
/// use atoma_sui::events::AtomaEvent;
///
/// async fn process_event(
///     state_manager: &AtomaStateManager,
///     event: AtomaEvent,
/// ) -> Result<()> {
///     handle_atoma_event(event, state_manager).await
/// }
/// ```
///
/// # Notes
///
/// The function uses the `#[instrument]` attribute for tracing with `skip_all` to prevent logging
/// of potentially sensitive parameters. Many events are currently just logged for monitoring
/// purposes and may be expanded with additional handling logic in the future.
#[instrument(level = "trace", skip_all)]
pub async fn handle_atoma_event(
    event: AtomaEvent,
    state_manager: &AtomaStateManager,
) -> Result<()> {
    match event {
        AtomaEvent::TaskRegisteredEvent(event) => handle_new_task_event(state_manager, event).await,
        AtomaEvent::TaskDeprecationEvent(event) => {
            handle_task_deprecation_event(state_manager, event).await
        }
        AtomaEvent::NodeSubscribedToTaskEvent(event) => {
            handle_node_task_subscription_event(state_manager, event).await
        }
        AtomaEvent::NodeSubscriptionUpdatedEvent(event) => {
            handle_node_task_subscription_updated_event(state_manager, event).await
        }
        AtomaEvent::NodeUnsubscribedFromTaskEvent(event) => {
            handle_node_task_unsubscription_event(state_manager, event).await
        }
        AtomaEvent::StackCreatedEvent((event, timestamp)) => {
            handle_create_stack_stats(
                state_manager,
                event,
                timestamp_to_datetime_or_now(timestamp),
            )
            .await?;
            Ok(())
        }
        AtomaEvent::StackCreateAndUpdateEvent(event) => {
            // NOTE: Don't handle creation here. It's handled when the stack is created right away.
            info!("Stack creates and update event: {:?}", event);
            Ok(())
        }
        AtomaEvent::StackTrySettleEvent((event, timestamp)) => {
            handle_stack_try_settle_event(
                state_manager,
                event,
                timestamp_to_datetime_or_now(timestamp),
            )
            .await
        }
        AtomaEvent::StackSettlementTicketEvent(event) => {
            handle_stack_settlement_ticket_event(state_manager, event).await
        }
        AtomaEvent::StackSettlementTicketClaimedEvent(event) => {
            handle_stack_settlement_ticket_claimed_event(state_manager, event).await
        }
        AtomaEvent::StackAttestationDisputeEvent(event) => {
            handle_stack_attestation_dispute_event(state_manager, event).await
        }
        AtomaEvent::NewStackSettlementAttestationEvent(event) => {
            handle_new_stack_settlement_attestation_event(state_manager, event).await
        }
        AtomaEvent::NewKeyRotationEvent(event) => {
            handle_new_key_rotation_event(state_manager, event).await
        }
        AtomaEvent::NodePublicKeyCommittmentEvent(event) => {
            handle_node_key_rotation_event(state_manager, event).await
        }
        AtomaEvent::PublishedEvent(event) => {
            info!("Published event: {:?}", event);
            Ok(())
        }
        AtomaEvent::NodeRegisteredEvent((event, address)) => {
            handle_node_registration_event(state_manager, event, address.to_string()).await
        }
        AtomaEvent::NodeSubscribedToModelEvent(event) => {
            info!("Node subscribed to model event: {:?}", event);
            Ok(())
        }
        AtomaEvent::FirstSubmissionEvent(event) => {
            info!("First submission event: {:?}", event);
            Ok(())
        }
        AtomaEvent::DisputeEvent(event) => {
            info!("Dispute event: {:?}", event);
            Ok(())
        }
        AtomaEvent::NewlySampledNodesEvent(event) => {
            info!("Newly sampled nodes event: {:?}", event);
            Ok(())
        }
        AtomaEvent::SettledEvent(event) => {
            info!("Settled event: {:?}", event);
            Ok(())
        }
        AtomaEvent::RetrySettlementEvent(event) => {
            info!("Retry settlement event: {:?}", event);
            Ok(())
        }
        AtomaEvent::TaskRemovedEvent(event) => {
            info!("Task removed event: {:?}", event);
            Ok(())
        }
        AtomaEvent::Text2ImagePromptEvent(event) => {
            info!("Text2Image prompt event: {:?}", event);
            Ok(())
        }
        AtomaEvent::Text2TextPromptEvent(event) => {
            info!("Text2Text prompt event: {:?}", event);
            Ok(())
        }
    }
}

/// Handles peer-to-peer (P2P) events in the Atoma network by processing node registration and verification events.
///
/// This function serves as a central handler for P2P events, specifically dealing with:
/// - Node public URL registration events
/// - Node small ID ownership verification events
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` that provides access to the state database
/// * `event` - An `AtomaP2pEvent` enum representing the P2P event to be processed
/// * `sender` - An optional oneshot channel sender to communicate the result of the event handling
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the event was processed successfully, or an error if the operation failed
///
/// # Errors
///
/// This function will return an error if:
/// * The underlying handlers fail to process the event
/// * Database operations fail during event processing
///
/// # Examples
///
/// ```rust,ignore
/// use atoma_state::AtomaStateManager;
/// use atoma_p2p::AtomaP2pEvent;
/// use tokio::sync::oneshot;
///
/// async fn process_p2p_event(
///     state_manager: &AtomaStateManager,
///     event: AtomaP2pEvent,
/// ) {
///     let (tx, rx) = oneshot::channel();
///     if let Err(e) = handle_p2p_event(state_manager, event, Some(tx)).await {
///         eprintln!("Failed to handle P2P event: {}", e);
///     }
///     let result = rx.await.unwrap();
///     if result {
///         println!("P2P event handled successfully");
///     } else {
///         println!("P2P event handling failed");
///     }
/// }
/// ```
///
/// # Event Types
///
/// ## NodePublicUrlRegistrationEvent
/// Registers or updates a node's public URL in the network. This allows other nodes to discover
/// and communicate with the node. Includes:
/// - `public_url`: The URL where the node can be reached
/// - `node_small_id`: The node's unique identifier
/// - `timestamp`: When the registration occurred
///
/// ## VerifyNodeSmallIdOwnership
/// Verifies that a node's small ID is properly owned by associating it with a Sui blockchain
/// address. Includes:
/// - `node_small_id`: The ID to verify
/// - `sui_address`: The blockchain address claiming ownership
///
/// # Notes
///
/// The function uses the `#[instrument]` attribute for tracing, with `skip_all` to prevent
/// logging of potentially sensitive parameters. The actual event processing is delegated to
/// specialized handler functions for each event type.
#[instrument(level = "trace", skip_all)]
pub async fn handle_p2p_event(
    state_manager: &AtomaStateManager,
    event: AtomaP2pEvent,
    sender: Option<oneshot::Sender<bool>>,
) -> Result<()> {
    match event {
        AtomaP2pEvent::NodeMetricsRegistrationEvent {
            public_url,
            node_small_id,
            timestamp,
            country,
            node_metrics,
        } => {
            handle_node_metrics_registration_event(
                state_manager,
                public_url,
                node_small_id as i64,
                timestamp as i64,
                country,
                node_metrics,
            )
            .await
        }
        AtomaP2pEvent::VerifyNodeSmallIdOwnership {
            node_small_id,
            sui_address,
        } => {
            let result = handle_node_small_id_ownership_verification_event(
                state_manager,
                node_small_id as i64,
                sui_address,
            )
            .await;
            if let Some(sender) = sender {
                sender.send(result.is_ok()).map_err(|_| {
                    error!(
                        target = "atoma-state-handlers",
                        event = "handle-node-small-id-ownership-verification-event",
                        "Failed to send result to sender"
                    );
                    AtomaStateManagerError::ChannelSendError
                })?;
            } else {
                error!(
                    target = "atoma-state-handlers",
                    event = "handle-node-small-id-ownership-verification-event",
                    "No sender provided for node small id ownership verification event, this should never happen"
                );
                return Err(AtomaStateManagerError::ChannelSendError);
            }
            Ok(())
        }
    }
}

/// Handles a new task event by processing and inserting it into the database.
///
/// This function takes a serialized `TaskRegisteredEvent`, deserializes it, and
/// inserts the corresponding task into the database using the provided `AtomaStateManager`.
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` for database operations.
/// * `value` - A `serde_json::Value` containing the serialized `TaskRegisteredEvent`.
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the task was successfully processed and inserted, or an error otherwise.
///
/// # Errors
///
/// This function will return an error if:
/// * The `value` cannot be deserialized into a `TaskRegisteredEvent`.
/// * The `AtomaStateManager` fails to insert the new task into the database.
#[instrument(level = "trace", skip_all)]
pub async fn handle_new_task_event(
    state_manager: &AtomaStateManager,
    event: TaskRegisteredEvent,
) -> Result<()> {
    trace!(
        target = "atoma-state-handlers",
        event = "handle-new-task-event",
        "Processing new task event"
    );
    let task = event.into();
    state_manager.state.insert_new_task(task).await?;
    Ok(())
}

/// Handles a task deprecation event.
///
/// This function processes a task deprecation event by parsing the event data,
/// extracting the necessary information, and updating the task's status in the database.
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` for database operations.
/// * `value` - A `serde_json::Value` containing the serialized task deprecation event data.
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the event was processed successfully, or an error if something went wrong.
///
/// # Errors
///
/// This function will return an error if:
/// * The event data cannot be deserialized into a `TaskDeprecationEvent`.
/// * The database operation to deprecate the task fails.
///
/// # Behavior
///
/// The function performs the following steps:
/// 1. Deserializes the input `value` into a `TaskDeprecationEvent`.
/// 2. Extracts the `task_small_id` and `epoch` from the event.
/// 3. Calls the `deprecate_task` method on the `AtomaStateManager` to update the task's status in the database.
#[instrument(level = "trace", skip_all)]
pub async fn handle_task_deprecation_event(
    state_manager: &AtomaStateManager,
    event: TaskDeprecationEvent,
) -> Result<()> {
    trace!(
        target = "atoma-state-handlers",
        event = "handle-task-deprecation-event",
        "Processing task deprecation event"
    );
    let task_small_id = event.task_small_id;
    let epoch = event.epoch;
    state_manager
        .state
        .deprecate_task(task_small_id.inner as i64, epoch as i64)
        .await?;
    Ok(())
}

/// Handles a node task subscription event.
///
/// This function processes a node task subscription event by parsing the event data,
/// extracting the necessary information, and updating the node's subscription to the task in the database.
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` for database operations.
/// * `event` - A `NodeSubscribedToTaskEvent` containing the details of the subscription event.
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the event was processed successfully, or an error if something went wrong.
///
/// # Errors
///
/// This function will return an error if:
/// * The event data cannot be deserialized into a `NodeSubscribedToTaskEvent`.
/// * The database operation to subscribe the node to the task fails.
///
/// # Behavior
///
/// The function performs the following steps:
/// 1. Extracts the `node_small_id`, `task_small_id`, `price_per_one_million_compute_units`, and `max_num_compute_units` from the event.
/// 2. Calls the `subscribe_node_to_task` method on the `AtomaStateManager` to update the node's subscription in the database.
#[instrument(level = "trace", skip_all)]
pub async fn handle_node_task_subscription_event(
    state_manager: &AtomaStateManager,
    event: NodeSubscribedToTaskEvent,
) -> Result<()> {
    trace!(
        target = "atoma-state-handlers",
        event = "handle-node-task-subscription-event",
        "Processing node subscription event"
    );
    let node_small_id = event.node_small_id.inner as i64;
    let task_small_id = event.task_small_id.inner as i64;
    let price_per_one_million_compute_units = event.price_per_one_million_compute_units as i64;
    let max_num_compute_units = event.max_num_compute_units as i64;
    state_manager
        .state
        .subscribe_node_to_task(
            node_small_id,
            task_small_id,
            price_per_one_million_compute_units,
            max_num_compute_units,
        )
        .await?;
    Ok(())
}

/// Handles a node task subscription updated event.
///
/// This function processes a node task subscription updated event by parsing the event data,
/// extracting the necessary information, and updating the node's subscription to the task in the database.
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` for database operations.
/// * `event` - A `NodeSubscriptionUpdatedEvent` containing the details of the subscription update.
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the event was processed successfully, or an error if something went wrong.
///
/// # Errors
///
/// This function will return an error if:
/// * The event data cannot be deserialized into a `NodeSubscriptionUpdatedEvent`.
/// * The database operation to update the node's subscription to the task fails.
///
/// # Behavior
///
/// The function performs the following steps:
/// 1. Extracts the `node_small_id`, `task_small_id`, `price_per_one_million_compute_units`, and `max_num_compute_units` from the event.
/// 2. Calls the `update_node_subscription` method on the `AtomaStateManager` to update the node's subscription in the database.
#[instrument(level = "trace", skip_all)]
pub async fn handle_node_task_subscription_updated_event(
    state_manager: &AtomaStateManager,
    event: NodeSubscriptionUpdatedEvent,
) -> Result<()> {
    trace!(
        target = "atoma-state-handlers",
        event = "handle-node-task-subscription-updated-event",
        "Processing node subscription updated event"
    );
    let node_small_id = event.node_small_id.inner as i64;
    let task_small_id = event.task_small_id.inner as i64;
    let price_per_one_million_compute_units = event.price_per_one_million_compute_units as i64;
    let max_num_compute_units = event.max_num_compute_units as i64;
    state_manager
        .state
        .update_node_subscription(
            node_small_id,
            task_small_id,
            price_per_one_million_compute_units,
            max_num_compute_units,
        )
        .await?;
    Ok(())
}

/// Handles a node task unsubscription event.
///
/// This function processes a node task unsubscription event by parsing the event data,
/// extracting the necessary information, and updating the node's subscription status in the database.
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` for database operations.
/// * `event` - A `NodeUnsubscribedFromTaskEvent` containing the details of the unsubscription event.
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the event was processed successfully, or an error if something went wrong.
///
/// # Errors
///
/// This function will return an error if:
/// * The event data cannot be deserialized into a `NodeUnsubscribedFromTaskEvent`.
/// * The database operation to unsubscribe the node from the task fails.
///
/// # Behavior
///
/// The function performs the following steps:
/// 1. Extracts the `node_small_id` and `task_small_id` from the event.
/// 2. Calls the `unsubscribe_node_from_task` method on the `AtomaStateManager` to update the node's subscription status in the database.
#[instrument(level = "trace", skip_all)]
pub async fn handle_node_task_unsubscription_event(
    state_manager: &AtomaStateManager,
    event: NodeUnsubscribedFromTaskEvent,
) -> Result<()> {
    trace!(
        target = "atoma-state-handlers",
        event = "handle-node-task-unsubscription-event",
        "Processing node unsubscription event"
    );
    let node_small_id = event.node_small_id.inner as i64;
    let task_small_id = event.task_small_id.inner as i64;
    state_manager
        .state
        .unsubscribe_node_from_task(node_small_id, task_small_id)
        .await?;
    Ok(())
}

/// Handles a stack created event.
///
/// This function processes a stack created event by parsing the event data,
/// checking if the selected node is one of the current nodes, and if so,
/// inserting the new stack into the database.
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` for database operations.
/// * `event` - A `StackCreatedEvent` containing the details of the stack creation event.
/// * `node_small_ids` - A slice of `u64` values representing the small IDs of the current nodes.
/// * `acquired_timestamp` - The timestamp of the event.
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the event was processed successfully, or an error if something went wrong.
///
/// # Errors
///
/// This function will return an error if:
/// * The event data cannot be deserialized into a `StackCreatedEvent`.
/// * The database operation to insert the new stack fails.
///
/// # Behavior
///
/// The function performs the following steps:
/// 1. Extracts the `selected_node_id` from the event.
/// 2. Checks if the `selected_node_id` is present in the `node_small_ids` slice.
/// 3. If the node is valid, it converts the event into a stack object and inserts it into the database.
#[instrument(level = "trace", skip_all)]
pub async fn handle_stack_created_event(
    state_manager: &AtomaStateManager,
    event: StackCreatedEvent,
    already_computed_units: i64,
    user_id: i64,
    acquired_timestamp: DateTime<Utc>,
) -> Result<()> {
    let node_small_id = event.selected_node_id.inner;
    trace!(
        target = "atoma-state-handlers",
        event = "handle-stack-created-event",
        "Stack selected current node, with id {node_small_id}, inserting new stack"
    );
    let mut stack: Stack = event.into();
    stack.already_computed_units = already_computed_units;
    state_manager
        .state
        .insert_new_stack(stack, user_id, acquired_timestamp)
        .await?;
    Ok(())
}

/// Handles create stack for stats.
///
/// This function processes a stack created event by parsing the event data,
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` for database operations.
/// * `event` - A `StackCreatedEvent` containing the details of the stack creation event.
/// * `timestamp` - The timestamp of the event.
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the event was processed successfully, or an error if something went wrong.
///
/// # Errors
///
/// This function will return an error if:
/// * The event data cannot be deserialized into a `StackCreatedEvent`.
/// * The database operation to insert the new stack fails.
///
#[instrument(level = "trace", skip_all)]
pub async fn handle_create_stack_stats(
    state_manager: &AtomaStateManager,
    event: StackCreatedEvent,
    timestamp: DateTime<Utc>,
) -> Result<()> {
    let stack = event.into();
    state_manager
        .state
        .new_stats_stack(stack, timestamp)
        .await?;
    Ok(())
}

/// Handles a stack try settle event.
///
/// This function processes a stack try settle event by parsing the event data,
/// converting it into a stack settlement ticket, and inserting it into the database.
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` for database operations.
/// * `event` - A `StackTrySettleEvent` containing the details of the stack try settle event.
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the event was processed successfully, or an error if something went wrong.
///
/// # Errors
///
/// This function will return an error if:
/// * The event data cannot be deserialized into a `StackTrySettleEvent`.
/// * The database operation to insert the new stack settlement ticket fails.
///
/// # Behavior
///
/// The function performs the following steps:
/// 1. Converts the `StackTrySettleEvent` into a stack settlement ticket.
/// 2. Calls the `insert_new_stack_settlement_ticket` method on the `AtomaStateManager` to insert the ticket into the database.
#[instrument(level = "trace", skip_all)]
pub async fn handle_stack_try_settle_event(
    state_manager: &AtomaStateManager,
    event: StackTrySettleEvent,
    timestamp: DateTime<Utc>,
) -> Result<()> {
    trace!(
        target = "atoma-state-handlers",
        event = "handle-stack-try-settle-event",
        "Processing stack try settle event"
    );
    let stack_settlement_ticket = StackSettlementTicket::try_from(event)?;
    state_manager
        .state
        .insert_new_stack_settlement_ticket(stack_settlement_ticket, timestamp)
        .await?;
    Ok(())
}

/// Handles a new stack settlement attestation event.
///
/// This function processes a new stack settlement attestation event by parsing the event data
/// and updating the corresponding stack settlement ticket in the database with attestation commitments.
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` for database operations.
/// * `event` - A `NewStackSettlementAttestationEvent` containing the details of the attestation event.
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the event was processed successfully, or an error if something went wrong.
///
/// # Errors
///
/// This function will return an error if:
/// * The event data cannot be deserialized into a `NewStackSettlementAttestationEvent`.
/// * The database operation to update the stack settlement ticket with attestation commitments fails.
///
/// # Behavior
///
/// The function performs the following steps:
/// 1. Extracts the `stack_small_id`, `attestation_node_id`, `committed_stack_proof`, and `stack_merkle_leaf` from the event.
/// 2. Calls the `update_stack_settlement_ticket_with_attestation_commitments` method on the `AtomaStateManager` to update the database.
#[instrument(level = "trace", skip_all)]
pub async fn handle_new_stack_settlement_attestation_event(
    state_manager: &AtomaStateManager,
    event: NewStackSettlementAttestationEvent,
) -> Result<()> {
    trace!(
        target = "atoma-state-handlers",
        event = "handle-new-stack-settlement-attestation-event",
        "Processing new stack settlement attestation event"
    );
    let stack_small_id = event.stack_small_id.inner as i64;
    let attestation_node_id = event.attestation_node_id.inner as i64;
    let committed_stack_proof = event.committed_stack_proof;
    let stack_merkle_leaf = event.stack_merkle_leaf;

    state_manager
        .state
        .update_stack_settlement_ticket_with_attestation_commitments(
            stack_small_id,
            committed_stack_proof,
            stack_merkle_leaf,
            attestation_node_id,
        )
        .await?;
    Ok(())
}

/// Handles a stack settlement ticket event.
///
/// This function processes a stack settlement ticket event by parsing the event data
/// and updating the corresponding stack settlement ticket in the database.
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` for database operations.
/// * `event` - A `StackSettlementTicketEvent` containing the details of the stack settlement ticket event.
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the event was processed successfully, or an error if something went wrong.
///
/// # Errors
///
/// This function will return an error if:
/// * The event data cannot be deserialized into a `StackSettlementTicketEvent`.
/// * The database operation to settle the stack settlement ticket fails.
///
/// # Behavior
///
/// The function performs the following steps:
/// 1. Extracts the `stack_small_id` and `dispute_settled_at_epoch` from the event.
/// 2. Calls the `settle_stack_settlement_ticket` method on the `AtomaStateManager` to update the database.
#[instrument(level = "trace", skip_all)]
pub async fn handle_stack_settlement_ticket_event(
    state_manager: &AtomaStateManager,
    event: StackSettlementTicketEvent,
) -> Result<()> {
    trace!(
        target = "atoma-state-handlers",
        event = "handle-stack-settlement-ticket-event",
        "Processing stack settlement ticket event"
    );
    let stack_small_id = event.stack_small_id.inner as i64;
    let dispute_settled_at_epoch = event.dispute_settled_at_epoch as i64;
    state_manager
        .state
        .settle_stack_settlement_ticket(stack_small_id, dispute_settled_at_epoch)
        .await?;
    Ok(())
}

/// Handles a stack settlement ticket claimed event.
///
/// This function processes a stack settlement ticket claimed event by parsing the event data
/// and updating the corresponding stack settlement ticket in the database with claim information.
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` for database operations.
/// * `event` - A `StackSettlementTicketClaimedEvent` containing the details of the stack settlement ticket claimed event.
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the event was processed successfully, or an error if something went wrong.
///
/// # Errors
///
/// This function will return an error if:
/// * The event data cannot be deserialized into a `StackSettlementTicketClaimedEvent`.
/// * The database operation to update the stack settlement ticket with claim information fails.
///
/// # Behavior
///
/// The function performs the following steps:
/// 1. Extracts the `stack_small_id` and `user_refund_amount` from the event.
/// 2. Calls the `update_stack_settlement_ticket_with_claim` method on the `AtomaStateManager` to update the database.
#[instrument(level = "trace", skip_all)]
pub async fn handle_stack_settlement_ticket_claimed_event(
    state_manager: &AtomaStateManager,
    event: StackSettlementTicketClaimedEvent,
) -> Result<()> {
    trace!(
        target = "atoma-state-handlers",
        event = "handle-stack-settlement-ticket-claimed-event",
        "Processing stack settlement ticket claimed event"
    );
    let stack_small_id = event.stack_small_id.inner as i64;
    let user_refund_amount = event.user_refund_amount as i64;
    state_manager
        .state
        .update_stack_settlement_ticket_with_claim(stack_small_id, user_refund_amount)
        .await?;
    Ok(())
}

/// Handles a stack attestation dispute event.
///
/// This function processes a stack attestation dispute event by parsing the event data
/// and inserting the dispute information into the database.
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` for database operations.
/// * `event` - A `StackAttestationDisputeEvent` containing the details of the dispute event.
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the event was processed successfully, or an error if something went wrong.
///
/// # Errors
///
/// This function will return an error if:
/// * The event data cannot be deserialized into a `StackAttestationDisputeEvent`.
/// * The database operation to insert the stack attestation dispute fails.
///
/// # Behavior
///
/// The function performs the following steps:
/// 1. Converts the `StackAttestationDisputeEvent` into a stack attestation dispute object.
/// 2. Calls the `insert_stack_attestation_dispute` method on the `AtomaStateManager` to insert the dispute into the database.
#[instrument(level = "trace", skip_all)]
pub async fn handle_stack_attestation_dispute_event(
    state_manager: &AtomaStateManager,
    event: StackAttestationDisputeEvent,
) -> Result<()> {
    trace!(
        target = "atoma-state-handlers",
        event = "handle-stack-attestation-dispute-event",
        "Processing stack attestation dispute event"
    );
    let stack_attestation_dispute = event.into();
    state_manager
        .state
        .insert_stack_attestation_dispute(stack_attestation_dispute)
        .await?;
    Ok(())
}

/// Handles node registration event.
///
/// This function processes a node registration event by parsing the event data
/// and inserting the node into the database.
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` for database operations.
/// * `event` - A `NodeRegisteredEvent` containing the details of the node registration event.
/// * `address` - The public address of the node.
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the event was processed successfully, or an error if something went wrong.
///
/// # Errors
///
/// This function will return an error if:
/// * The event data cannot be deserialized into a `NodeRegisteredEvent`.
/// * The database operation to insert the node fails.
///
/// # Behavior
///
/// The function performs the following steps:
/// 1. Extracts the `node_small_id` from the event.
/// 2. Calls the `insert_new_node` method on the `AtomaStateManager` to insert the node into the database.
#[instrument(level = "trace", skip_all)]
pub async fn handle_node_registration_event(
    state_manager: &AtomaStateManager,
    event: NodeRegisteredEvent,
    address: String,
) -> Result<()> {
    state_manager
        .state
        .insert_new_node(event.node_small_id.inner as i64, event.badge_id, address)
        .await?;
    Ok(())
}

/// Handles events related to the state manager.
///
/// This function processes various events that are sent to the state manager,
/// including requests to get available stacks with compute units, update the number
/// of tokens for a stack, and update the total hash of a stack.
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` for database operations.
/// * `event` - An `AtomaAtomaStateManagerEvent` enum that specifies the type of event to handle.
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the event was processed successfully, or an error if something went wrong.
///
/// # Errors
///
/// This function may return an error if:
/// * The database operations for updating tokens or hashes fail.
/// * The result sender fails to send the result for the `GetAvailableStackWithComputeUnits` event.
///
/// # Behavior
///
/// The function performs the following steps:
/// 1. Matches the incoming event to determine the type of operation to perform.
/// 2. For `GetAvailableStackWithComputeUnits`, it retrieves the available stack and sends the result.
/// 3. For `UpdateStackNumTokens`, it updates the number of tokens for the specified stack.
/// 4. For `UpdateStackTotalHash`, it updates the total hash for the specified stack.
#[instrument(level = "trace", skip_all)]
pub async fn handle_state_manager_event(
    state_manager: &AtomaStateManager,
    event: AtomaAtomaStateManagerEvent,
) -> Result<()> {
    match event {
        AtomaAtomaStateManagerEvent::GetAvailableStackWithComputeUnits {
            stack_small_id,
            public_key,
            total_num_tokens,
            result_sender,
        } => {
            trace!(
                target = "atoma-state-handlers",
                event = "handle-state-manager-event",
                "Getting available stack with compute units for stack with id: {}",
                stack_small_id
            );
            let result = state_manager
                .state
                .get_available_stack_with_compute_units(
                    stack_small_id,
                    &public_key,
                    total_num_tokens,
                )
                .await;
            result_sender
                .send(result)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::UpdateStackNumTokens {
            stack_small_id,
            estimated_total_tokens,
            total_tokens,
        } => {
            trace!(
                target = "atoma-state-handlers",
                event = "handle-state-manager-event",
                "Updating stack num tokens for stack with id: {}",
                stack_small_id
            );
            state_manager
                .state
                .update_stack_num_tokens(stack_small_id, estimated_total_tokens, total_tokens)
                .await?;
        }
        AtomaAtomaStateManagerEvent::UpdateStackTotalHash {
            stack_small_id,
            total_hash,
        } => {
            trace!(
                target = "atoma-state-handlers",
                event = "handle-state-manager-event",
                "Updating stack total hash for stack with id: {}",
                stack_small_id
            );
            state_manager
                .state
                .update_stack_total_hash(stack_small_id, total_hash)
                .await?;
        }
        AtomaAtomaStateManagerEvent::GetStacksForModel {
            model,
            free_compute_units,
            user_id,
            result_sender,
            is_confidential,
        } => {
            trace!(
                target = "atoma-state-handlers",
                event = "handle-state-manager-event",
                "Getting stacks for model: {} with free compute units: {}",
                model,
                free_compute_units
            );
            let stack = state_manager
                .state
                .get_stacks_for_model(&model, free_compute_units, user_id, is_confidential)
                .await;
            result_sender
                .send(stack)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::GetTasksForModel {
            model,
            result_sender,
        } => {
            trace!(
                target = "atoma-state-handlers",
                event = "handle-state-manager-event",
                "Getting tasks for model: {}",
                model
            );
            let tasks = state_manager.state.get_tasks_for_model(&model).await;
            result_sender
                .send(tasks)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::GetCheapestNodeForModel {
            model,
            is_confidential,
            result_sender,
        } => {
            trace!(
                target = "atoma-state-handlers",
                event = "handle-state-manager-event",
                "Getting cheapest node for model: {}",
                model
            );
            let node = state_manager
                .state
                .get_cheapest_node_for_model(&model, is_confidential)
                .await;
            result_sender
                .send(node)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::LockComputeUnitsForStack {
            stack_small_id,
            available_compute_units,
            result_sender,
        } => {
            let is_locked = state_manager
                .state
                .lock_compute_units(stack_small_id, available_compute_units)
                .await;
            result_sender
                .send(is_locked)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::SelectNodePublicKeyForEncryption {
            model,
            max_num_tokens,
            result_sender,
        } => {
            let node = state_manager
                .state
                .select_node_public_key_for_encryption(&model, max_num_tokens)
                .await?;
            result_sender
                .send(node)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::SelectNodePublicKeyForEncryptionForNode {
            node_small_id,
            result_sender,
        } => {
            let node = state_manager
                .state
                .select_node_public_key_for_encryption_for_node(node_small_id)
                .await?;
            result_sender
                .send(node)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::VerifyStackForConfidentialComputeRequest {
            stack_small_id,
            available_compute_units,
            result_sender,
        } => {
            let is_valid = state_manager
                .state
                .verify_stack_for_confidential_compute_request(
                    stack_small_id,
                    available_compute_units,
                )
                .await;
            result_sender
                .send(is_valid)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::UpsertNodePublicAddress {
            node_small_id,
            public_address,
            country,
        } => {
            trace!(
                target = "atoma-state-handlers",
                event = "handle-state-manager-event",
                "Upserting public address/country for node with id: {}",
                node_small_id
            );
            state_manager
                .state
                .update_node_public_address(node_small_id, public_address, country)
                .await?;
        }
        AtomaAtomaStateManagerEvent::GetNodePublicAddress {
            node_small_id,
            result_sender,
        } => {
            trace!(
                target = "atoma-state-handlers",
                event = "handle-state-manager-event",
                "Getting public address for node with id: {}",
                node_small_id
            );
            let public_address = state_manager
                .state
                .get_node_public_address(node_small_id)
                .await;
            result_sender
                .send(public_address)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::GetNodePublicUrlAndSmallId {
            stack_small_id,
            result_sender,
        } => {
            let result = state_manager
                .state
                .get_node_public_url_and_small_id(stack_small_id)
                .await;
            result_sender
                .send(result)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::GetNodeSuiAddress {
            node_small_id,
            result_sender,
        } => {
            trace!(
                target = "atoma-state-handlers",
                event = "handle-state-manager-event",
                "Getting sui address for node with id: {}",
                node_small_id
            );
            let sui_address = state_manager
                .state
                .get_node_sui_address(node_small_id)
                .await;
            result_sender
                .send(sui_address)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::NewStackAcquired {
            event,
            already_computed_units,
            transaction_timestamp,
            user_id,
        } => {
            handle_stack_created_event(
                state_manager,
                event,
                already_computed_units,
                user_id,
                transaction_timestamp,
            )
            .await?;
        }
        AtomaAtomaStateManagerEvent::UpdateNodeThroughputPerformance {
            timestamp,
            model_name,
            node_small_id,
            input_tokens,
            output_tokens,
            time,
        } => {
            state_manager
                .state
                .update_node_throughput_performance(
                    node_small_id,
                    input_tokens,
                    output_tokens,
                    time,
                )
                .await?;
            state_manager
                .state
                .add_compute_units_processed(
                    timestamp,
                    model_name,
                    input_tokens + output_tokens,
                    time,
                )
                .await?;
        }
        AtomaAtomaStateManagerEvent::UpdateNodePrefillPerformance {
            node_small_id,
            tokens,
            time,
        } => {
            state_manager
                .state
                .update_node_prefill_performance(node_small_id, tokens, time)
                .await?;
        }
        AtomaAtomaStateManagerEvent::UpdateNodeDecodePerformance {
            node_small_id,
            tokens,
            time,
        } => {
            state_manager
                .state
                .update_node_decode_performance(node_small_id, tokens, time)
                .await?;
        }
        AtomaAtomaStateManagerEvent::UpdateNodeLatencyPerformance {
            timestamp,
            node_small_id,
            latency,
        } => {
            state_manager
                .state
                .update_node_latency_performance(node_small_id, latency)
                .await?;
            state_manager.state.add_latency(timestamp, latency).await?;
        }
        AtomaAtomaStateManagerEvent::GetSelectedNodeX25519PublicKey {
            selected_node_id,
            result_sender,
        } => {
            let public_key = state_manager
                .state
                .get_selected_node_x25519_public_key(selected_node_id)
                .await;
            result_sender
                .send(public_key)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::GetUserIdByUsernamePassword {
            username,
            password,
            result_sender,
        } => {
            let user_id = state_manager
                .state
                .get_user_id_by_username_password(&username, &password)
                .await;
            result_sender
                .send(user_id)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::OAuth {
            username,
            result_sender,
        } => {
            let user_id = state_manager.state.oauth(&username).await;
            result_sender
                .send(user_id)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::RegisterUserWithPassword {
            username,
            password,
            result_sender,
        } => {
            let user_id = state_manager.state.register(&username, &password).await;
            result_sender
                .send(user_id)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::IsRefreshTokenValid {
            user_id,
            refresh_token_hash,
            result_sender,
        } => {
            let is_valid = state_manager
                .state
                .is_refresh_token_valid(user_id, &refresh_token_hash)
                .await;
            result_sender
                .send(is_valid)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::StoreRefreshToken {
            user_id,
            refresh_token_hash,
        } => {
            state_manager
                .state
                .store_refresh_token(user_id, &refresh_token_hash)
                .await?;
        }
        AtomaAtomaStateManagerEvent::RevokeRefreshToken {
            user_id,
            refresh_token_hash,
        } => {
            state_manager
                .state
                .delete_refresh_token(user_id, &refresh_token_hash)
                .await?;
        }
        AtomaAtomaStateManagerEvent::IsApiTokenValid {
            api_token,
            result_sender,
        } => {
            let user_id = state_manager.state.is_api_token_valid(&api_token).await;
            result_sender
                .send(user_id)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::StoreNewApiToken { user_id, api_token } => {
            state_manager
                .state
                .store_api_token(user_id, &api_token)
                .await?;
        }
        AtomaAtomaStateManagerEvent::RevokeApiToken { user_id, api_token } => {
            state_manager
                .state
                .delete_api_token(user_id, &api_token)
                .await?;
        }
        AtomaAtomaStateManagerEvent::GetApiTokensForUser {
            user_id,
            result_sender,
        } => {
            let api_tokens = state_manager.state.get_api_tokens_for_user(user_id).await;
            result_sender
                .send(api_tokens)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::UpdateSuiAddress {
            user_id,
            sui_address,
        } => {
            state_manager
                .state
                .update_sui_address(user_id, sui_address)
                .await?;
        }
        AtomaAtomaStateManagerEvent::GetSuiAddress {
            user_id,
            result_sender,
        } => {
            let sui_address = state_manager.state.get_sui_address(user_id).await;
            result_sender
                .send(sui_address)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::ConfirmUser {
            sui_address,
            user_id,
            result_sender,
        } => {
            let confirmation = state_manager.state.confirm_user(sui_address, user_id).await;
            result_sender
                .send(confirmation)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::TopUpBalance { user_id, amount } => {
            state_manager.state.top_up_balance(user_id, amount).await?;
        }
        AtomaAtomaStateManagerEvent::DeductFromUsdc {
            user_id,
            amount,
            result_sender,
        } => {
            let success = state_manager.state.deduct_from_usdc(user_id, amount).await;
            result_sender
                .send(success)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::InsertNewUsdcPaymentDigest {
            digest,
            result_sender,
        } => {
            let success = state_manager
                .state
                .insert_new_usdc_payment_digest(digest)
                .await;
            result_sender
                .send(success)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::GetSalt {
            user_id,
            result_sender,
        } => {
            let salt = state_manager.state.get_salt(user_id).await;
            result_sender
                .send(salt)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::SetSalt {
            user_id,
            salt,
            result_sender,
        } => {
            let success = state_manager.state.set_salt(user_id, &salt).await;
            result_sender
                .send(success)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
    }
    Ok(())
}

/// Handles a new key rotation event by updating the key rotation state in the database.
///
/// This function processes a key rotation event that occurs when the system performs a periodic
/// key rotation. It extracts the epoch and key rotation counter from the event and updates
/// the corresponding values in the state database.
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` for database operations
/// * `event` - A `NewKeyRotationEvent` containing:
///   * `epoch` - The epoch number when the key rotation occurred
///   * `key_rotation_counter` - The counter tracking the number of key rotations
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the key rotation was processed successfully, or an error if something went wrong
///
/// # Errors
///
/// This function will return an error if:
/// * The database operation to update the key rotation state fails
///
/// # Example
///
/// ```rust,ignore
/// use atoma_state::AtomaStateManager;
/// use atoma_sui::events::NewKeyRotationEvent;
///
/// async fn rotate_key(state_manager: &AtomaStateManager, event: NewKeyRotationEvent) {
///     if let Err(e) = handle_new_key_rotation_event(state_manager, event).await {
///         eprintln!("Failed to handle key rotation: {}", e);
///     }
/// }
/// ```
#[instrument(level = "trace", skip_all)]
pub async fn handle_new_key_rotation_event(
    state_manager: &AtomaStateManager,
    event: NewKeyRotationEvent,
) -> Result<()> {
    let NewKeyRotationEvent {
        epoch,
        key_rotation_counter,
    } = event;
    state_manager
        .state
        .insert_new_key_rotation(epoch as i64, key_rotation_counter as i64)
        .await?;
    Ok(())
}

/// Handles a node key rotation event by updating the node's public key in the database.
///
/// This function processes a node key rotation event, which occurs when a node updates its
/// cryptographic keys. It extracts the relevant information from the event and updates
/// the node's public key and associated data in the state database.
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` for database operations
/// * `event` - A `NodeKeyRotationEvent` containing the details of the key rotation:
///   * `epoch` - The epoch number when the key rotation occurred
///   * `node_id` - The identifier of the node performing the key rotation
///   * `node_badge_id` - The badge identifier associated with the node
///   * `new_public_key` - The new public key for the node
///   * `tee_remote_attestation_bytes` - Remote attestation data for trusted execution environment
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the key rotation was processed successfully, or an error if something went wrong
///
/// # Errors
///
/// This function will return an error if:
/// * The database operation to update the node's public key fails
///
/// # Example
///
/// ```rust,ignore
/// use atoma_state::AtomaStateManager;
/// use atoma_sui::events::NodeKeyRotationEvent;
///
/// async fn rotate_key(state_manager: &AtomaStateManager, event: NodeKeyRotationEvent) {
///     if let Err(e) = handle_node_key_rotation_event(state_manager, event).await {
///         eprintln!("Failed to handle key rotation: {}", e);
///     }
/// }
/// ```
#[instrument(level = "trace", skip_all)]
pub async fn handle_node_key_rotation_event(
    state_manager: &AtomaStateManager,
    event: NodePublicKeyCommittmentEvent,
) -> Result<()> {
    info!("Node key rotation event: {:?}", event);
    let NodePublicKeyCommittmentEvent {
        epoch,
        key_rotation_counter,
        node_id,
        new_public_key,
        tee_remote_attestation_bytes,
    } = event;
    let is_valid =
        utils::verify_quote_v4_attestation(&tee_remote_attestation_bytes, &new_public_key)
            .await
            .is_ok();
    state_manager
        .state
        .update_node_public_key(
            node_id.inner as i64,
            epoch as i64,
            key_rotation_counter as i64,
            new_public_key,
            tee_remote_attestation_bytes,
            is_valid,
        )
        .await?;
    Ok(())
}

/// Handles a node's public URL registration event by updating the node's URL in the state database.
///
/// This function processes events when nodes register or update their public URLs. It stores the URL
/// along with the registration timestamp in the state database, allowing other components to discover
/// and communicate with nodes in the network.
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` that provides access to the state database
/// * `public_url` - The public URL where the node can be reached
/// * `node_small_id` - The unique identifier of the node registering its URL
/// * `timestamp` - The Unix timestamp when the registration occurred
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the URL registration was successful, or an error if the operation failed
///
/// # Errors
///
/// This function will return an error if:
/// * The database operation to register the node's public URL fails
/// * The state manager encounters any internal errors during the registration process
///
/// # Example
///
/// ```rust,ignore
/// use atoma_state::AtomaStateManager;
///
/// async fn register_node_url(
///     state_manager: &AtomaStateManager,
///     url: String,
///     node_id: i64,
///     timestamp: i64
/// ) {
///     if let Err(e) = NodeMetricsRegistrationEvent(
///         state_manager,
///         url,
///         node_id,
///         timestamp
///     ).await {
///         eprintln!("Failed to register node URL: {}", e);
///     }
/// }
/// ```
///
/// # Notes
///
/// The function uses the `tracing` crate to log information about the URL registration event,
/// which can be useful for debugging and monitoring the system's behavior.
#[instrument(level = "trace", skip_all)]
pub(crate) async fn handle_node_metrics_registration_event(
    state_manager: &AtomaStateManager,
    public_url: String,
    node_small_id: i64,
    timestamp: i64,
    country: String,
    node_metrics: NodeMetrics,
) -> Result<()> {
    info!(
        target = "atoma-state-handlers",
        event = "handle-node-public-url-registration-event",
        public_url = public_url,
        node_small_id = node_small_id,
        timestamp = timestamp,
        "Node public url registration event"
    );
    let NodeMetrics {
        cpu_usage,
        cpu_frequency,
        ram_used,
        ram_total,
        ram_swap_used,
        ram_swap_total,
        num_cpus,
        network_rx,
        network_tx,
        num_gpus,
        gpus,
    } = node_metrics;

    validate_gpu_metrics(
        num_gpus,
        &gpus.iter().map(|gpu| gpu.memory_used).collect::<Vec<_>>(),
        &gpus.iter().map(|gpu| gpu.memory_total).collect::<Vec<_>>(),
        &gpus.iter().map(|gpu| gpu.memory_free).collect::<Vec<_>>(),
        &gpus
            .iter()
            .map(|gpu| gpu.percentage_time_read_write)
            .collect::<Vec<_>>(),
        &gpus
            .iter()
            .map(|gpu| gpu.percentage_time_gpu_execution)
            .collect::<Vec<_>>(),
        &gpus.iter().map(|gpu| gpu.temperature).collect::<Vec<_>>(),
        &gpus.iter().map(|gpu| gpu.power_usage).collect::<Vec<_>>(),
    )?;

    // Collect each metric using iterators
    let gpu_memory_used: Vec<_> = gpus.iter().map(|gpu| gpu.memory_used).collect();
    let gpu_memory_total: Vec<_> = gpus.iter().map(|gpu| gpu.memory_total).collect();
    let gpu_memory_free: Vec<_> = gpus.iter().map(|gpu| gpu.memory_free).collect();
    let gpu_percentage_time_read_write: Vec<_> = gpus
        .iter()
        .map(|gpu| gpu.percentage_time_read_write)
        .collect();
    let gpu_percentage_time_execution: Vec<_> = gpus
        .iter()
        .map(|gpu| gpu.percentage_time_gpu_execution)
        .collect();
    let gpu_temperatures: Vec<_> = gpus.iter().map(|gpu| gpu.temperature).collect();
    let gpu_power_usages: Vec<_> = gpus.iter().map(|gpu| gpu.power_usage).collect();

    let node_performance = node_performance::get_node_performance(
        &state_manager.state,
        node_small_id,
        num_cpus,
        f64::from(cpu_usage),
        cpu_frequency,
        ram_used as i64,
        ram_total as i64,
        ram_swap_used as i64,
        ram_swap_total as i64,
        network_rx as i64,
        network_tx as i64,
        gpu_memory_free.iter().map(|&x| x as i64).collect(),
        gpu_memory_total.iter().map(|&x| x as i64).collect(),
        gpu_percentage_time_execution
            .iter()
            .map(|&x| f64::from(x) / 100.0)
            .collect(),
        gpu_temperatures.iter().map(|&x| f64::from(x)).collect(),
        gpu_power_usages.iter().map(|&x| f64::from(x)).collect(),
    )
    .await?;

    state_manager
        .state
        .register_node_public_url(node_small_id, public_url, timestamp, country)
        .await?;
    state_manager
        .state
        .insert_node_metrics(
            node_small_id,
            timestamp,
            cpu_usage,
            num_cpus as i32,
            ram_used as i64,
            ram_total as i64,
            ram_swap_used as i64,
            ram_swap_total as i64,
            network_rx as i64,
            network_tx as i64,
            num_gpus as i32,
            gpu_memory_used.iter().map(|&x| x as i64).collect(),
            gpu_memory_total.iter().map(|&x| x as i64).collect(),
            gpu_memory_free.iter().map(|&x| x as i64).collect(),
            gpu_percentage_time_read_write
                .iter()
                .map(|&x| f64::from(x) / 100.0)
                .collect(),
            gpu_percentage_time_execution
                .iter()
                .map(|&x| f64::from(x) / 100.0)
                .collect(),
            gpu_temperatures
                .iter()
                .map(|&x| f64::from(x) / 100.0)
                .collect(),
            gpu_power_usages
                .iter()
                .map(|&x| f64::from(x) / 100.0)
                .collect(),
        )
        .await?;
    state_manager
        .state
        .insert_node_performance(
            node_small_id,
            timestamp,
            node_performance.total_performance_score,
        )
        .await?;
    Ok(())
}

/// Handles a node small ID ownership verification event by updating the node's ownership status in the database.
///
/// This function processes events that verify the ownership of a node's small ID by associating it with a
/// Sui blockchain address. This verification is crucial for ensuring that only authorized nodes can participate
/// in the network and that node identifiers cannot be spoofed.
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` that provides access to the state database
/// * `node_small_id` - The small ID of the node being verified (as an i64)
/// * `sui_address` - The Sui blockchain address claiming ownership of the node (as a String)
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the verification was successful, or an error if the operation failed
///
/// # Errors
///
/// This function will return an error if:
/// * The database operation to verify the node's ownership fails
/// * The state manager encounters any internal errors during the verification process
///
/// # Example
///
/// ```rust,ignore
/// use atoma_state::AtomaStateManager;
///
/// async fn verify_node_ownership(
///     state_manager: &AtomaStateManager,
///     node_id: i64,
///     address: String,
/// ) {
///     if let Err(e) = handle_node_small_id_ownership_verification_event(
///         state_manager,
///         node_id,
///         address,
///     ).await {
///         eprintln!("Failed to verify node ownership: {}", e);
///     }
/// }
/// ```
///
/// # Notes
///
/// The function uses the `tracing` crate to log information about the verification event,
/// which can be useful for debugging and monitoring the system's behavior. The logging
/// includes the node's small ID and the Sui address claiming ownership.
///
/// This verification is an important security measure that helps maintain the integrity
/// of the node network by ensuring that node identifiers are properly associated with
/// blockchain addresses.
#[instrument(level = "trace", skip_all)]
pub(crate) async fn handle_node_small_id_ownership_verification_event(
    state_manager: &AtomaStateManager,
    node_small_id: i64,
    sui_address: String,
) -> Result<()> {
    info!(
        target = "atoma-state-handlers",
        event = "handle-node-small-id-ownership-verification-event",
        node_small_id = node_small_id,
        sui_address = sui_address,
        "Node small id ownership verification event"
    );
    state_manager
        .state
        .verify_node_small_id_ownership(node_small_id, sui_address)
        .await?;
    Ok(())
}

pub mod utils {
    use super::{AtomaStateManagerError, Result};

    use dcap_qvl::collateral::get_collateral;
    use dcap_qvl::quote::{Quote, Report};
    use dcap_qvl::verify::verify;
    use std::time::Duration;

    /// The timeout to use for quote verification.
    const TIMEOUT: Duration = Duration::from_secs(10);

    /// The TCB update mode to use for quote verification.
    const TCB_UPDATE_MODE: &str = "early";

    /// Verifies a TEE (Trusted Execution Environment) remote attestation quote using Intel's DCAP Quote Verification Library.
    ///
    /// This function performs verification of a Quote V4 attestation by:
    /// 1. Retrieving collateral data from Intel's Provisioning Certificate Caching Service (PCCS)
    /// 2. Verifying the quote against the collateral using the current timestamp
    ///
    /// # Arguments
    ///
    /// * `tee_remote_attestation_bytes` - A byte slice containing the TEE remote attestation quote data
    /// * `new_public_key` - A byte slice containing the public key to be verified (currently unused in verification)
    ///
    /// # Returns
    ///
    /// * `Result<()>` - Ok(()) if verification succeeds, or an error if verification fails
    ///
    /// # Errors
    ///
    /// This function will return an error in the following cases:
    /// * If collateral retrieval from PCCS fails
    /// * If the system time cannot be determined
    /// * If quote verification fails
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use your_crate::verify_quote_v4_attestation;
    ///
    /// async fn verify_attestation() {
    ///     let quote_data = vec![/* quote data */];
    ///     let public_key = vec![/* public key data */];
    ///
    ///     match verify_quote_v4_attestation(&quote_data, &public_key).await {
    ///         Ok(()) => println!("Attestation verified successfully"),
    ///         Err(e) => eprintln!("Attestation verification failed: {:?}", e),
    ///     }
    /// }
    /// ```
    ///
    /// # Notes
    ///
    /// * Uses Intel's PCCS service at a hardcoded URL with a 10-second timeout
    /// * The `new_public_key` parameter is currently passed through but not used in the verification process
    /// * This function is specifically for Quote V4 format attestations
    pub async fn verify_quote_v4_attestation(
        quote_bytes: &[u8],
        new_public_key: &[u8],
    ) -> Result<()> {
        let quote = Quote::parse(quote_bytes)
            .map_err(|e| AtomaStateManagerError::FailedToParseQuote(format!("{e:?}")))?;
        let fmspc = quote
            .fmspc()
            .map_err(|e| AtomaStateManagerError::FailedToRetrieveFmspc(format!("{e:?}")))?;
        #[allow(clippy::uninlined_format_args)]
        let certification_tcb_url = format!(
            "https://api.trustedservices.intel.com/tdx/certification/v4/tcb?fmspc={:?}&update={TCB_UPDATE_MODE}",
            fmspc
        );
        let collateral = get_collateral(&certification_tcb_url, quote_bytes, TIMEOUT)
            .await
            .map_err(|e| AtomaStateManagerError::FailedToRetrieveCollateral(format!("{e:?}")))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| AtomaStateManagerError::UnixTimeWentBackwards(e.to_string()))?
            .as_secs();
        match quote.report {
            Report::SgxEnclave(_) => {
                return Err(AtomaStateManagerError::FailedToVerifyQuote(
                    "Report SGX type not supported".to_string(),
                ));
            }
            Report::TD10(report) => {
                if report.report_data != new_public_key {
                    return Err(AtomaStateManagerError::FailedToVerifyQuote(
                        "Report TD10 data does not match new public key".to_string(),
                    ));
                }
            }
            Report::TD15(report) => {
                if report.base.report_data != new_public_key {
                    return Err(AtomaStateManagerError::FailedToVerifyQuote(
                        "Report TD15 data does not match new public key".to_string(),
                    ));
                }
            }
        }
        verify(quote_bytes, &collateral, now)
            .map_err(|e| AtomaStateManagerError::FailedToVerifyQuote(format!("{e:?}")))?;
        Ok(())
    }
}

pub mod node_performance {
    use serde::{Deserialize, Serialize};
    use tracing::instrument;

    use crate::{state_manager::Result, types::PerformanceWeights, AtomaState};

    /// Represents the performance metrics and scoring weights for a node in the Atoma network.
    ///
    /// This struct combines both the overall performance score of a node and the weights used
    /// to calculate that score from individual hardware component metrics.
    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct NodePerformance {
        /// The aggregate performance score of the node, calculated by combining
        /// weighted scores from GPU, CPU, RAM, and network performance metrics
        pub total_performance_score: f64,
    }

    /// Calculates a comprehensive performance score for a node based on its hardware metrics and resource utilization.
    ///
    /// This function evaluates node performance across multiple dimensions including GPU, CPU, RAM, and network metrics.
    /// It uses configurable weights to compute a normalized score that represents the node's overall capability and
    /// efficiency. The final score is smoothed using an exponential moving average.
    ///
    /// # Performance Scoring Components
    ///
    /// ## GPU Performance (weighted by `gpu_score_weight`)
    /// Combines four weighted sub-scores:
    ///
    /// 1. **VRAM Usage** (`gpu_vram_weight`)
    ///    - Higher is better (indicates efficient KV cache allocation)
    ///    - Uses minimum usage percentage across all GPUs
    ///    - Score = (used_memory / total_memory) * weight
    ///
    /// 2. **Execution Availability** (`gpu_exec_avail_weight`)
    ///    - Lower is better (indicates GPU availability)
    ///    - Averages execution rates across all GPUs
    ///    - Score = average(execution_rate * weight)
    ///
    /// 3. **Temperature** (`gpu_temp_weight`)
    ///    - Lower is better
    ///    - Uses step function:
    ///      * 1.0 for temps <= threshold
    ///      * Linear decrease from 1.0 to 0.0 between threshold and max
    ///      * 0.0 for temps >= max
    ///    - Uses minimum score across all GPUs
    ///
    /// 4. **Power Usage** (`gpu_power_weight`)
    ///    - Lower is better
    ///    - Uses step function:
    ///      * 1.0 for power <= threshold
    ///      * Linear decrease from 1.0 to 0.0 between threshold and max
    ///      * 0.0 for power >= max
    ///    - Averages across all GPUs
    ///
    /// ## CPU Performance (`cpu_score_weight`)
    /// - Balances current usage with available capacity
    /// - Score = (cpu_usage * weight) + ((1 - cpu_usage) * weight)
    ///
    /// ## Memory Performance
    /// 1. **RAM** (`ram_score_weight`)
    ///    - Score = (1 - ram_used/ram_total) * weight
    ///
    /// 2. **Swap** (`swap_ram_score_weight`)
    ///    - Score = (1 - swap_used/swap_total) * weight
    ///
    /// ## Network Performance (`network_score_weight`)
    /// - Inverse of total network traffic
    /// - Score = 1 / (1 + rx + tx)
    ///
    /// # Final Score Calculation
    /// 1. Combines all weighted component scores
    /// 2. Applies exponential moving average:
    ///    - Uses `moving_avg_window_size` and `moving_avg_smooth_factor`
    ///    - Smooths score with previous performance data
    ///
    /// # Arguments
    ///
    /// * `state` - Reference to AtomaState for accessing performance weights
    /// * `node_small_id` - The small ID of the node
    /// * `num_cpus` - The number of CPUs on the node
    /// * `cpu_usage` - CPU usage (0.0 to 1.0)
    /// * `cpu_frequency` - CPU frequency in MHz
    /// * `ram_used` - Used RAM in bytes
    /// * `ram_total` - Total RAM in bytes
    /// * `ram_swap_used` - Used swap space in bytes
    /// * `ram_swap_total` - Total swap space in bytes
    /// * `network_rx` - Network receive throughput in bytes
    /// * `network_tx` - Network transmit throughput in bytes
    /// * `num_gpus` - Number of GPUs
    /// * `gpu_memory_used` - Vector of used GPU memory per GPU in bytes
    /// * `gpu_memory_total` - Vector of total GPU memory per GPU in bytes
    /// * `gpu_percentage_time_execution` - Vector of GPU execution time percentages (0.0 to 1.0)
    /// * `gpu_temperatures` - Vector of GPU temperatures in Celsius
    /// * `gpu_power_usages` - Vector of GPU power usage in Watts
    ///
    /// # Returns
    ///
    /// * `Result<NodePerformance>` - Structure containing the calculated total performance score
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Unable to retrieve performance weights from state
    /// - Unable to retrieve previous performance scores
    #[instrument(level = "trace", skip_all)]
    #[allow(clippy::similar_names)]
    #[allow(clippy::too_many_arguments)]
    pub async fn get_node_performance(
        state: &AtomaState,
        node_small_id: i64,
        num_cpus: u32,
        cpu_usage: f64,
        cpu_frequency: u64,
        ram_used: i64,
        ram_total: i64,
        ram_swap_used: i64,
        ram_swap_total: i64,
        network_rx: i64,
        network_tx: i64,
        gpu_memory_free: Vec<i64>,
        gpu_memory_total: Vec<i64>,
        gpu_percentage_time_execution: Vec<f64>,
        gpu_temperatures: Vec<f64>,
        gpu_power_usages: Vec<f64>,
    ) -> Result<NodePerformance> {
        let PerformanceWeights {
            gpu_score_weight,
            cpu_score_weight,
            ram_score_weight,
            swap_ram_score_weight,
            network_score_weight,
            gpu_vram_weight,
            gpu_exec_avail_weight,
            gpu_temp_weight,
            gpu_power_weight,
            gpu_temp_threshold,
            gpu_temp_max,
            gpu_power_threshold,
            gpu_power_max,
            moving_avg_window_size,
            moving_avg_smooth_factor,
        } = state.get_performance_weights().await?;

        let previous_performance_score = state.get_latest_performance_scores(node_small_id).await?;

        // --- GPU Subscores --- //
        let gpu_vram_score = compute_gpu_vram_utilization_score(
            &gpu_memory_free,
            &gpu_memory_total,
            gpu_vram_weight,
        );
        let gpu_exec_score =
            compute_gpu_execution_score(&gpu_percentage_time_execution, gpu_exec_avail_weight);
        let gpu_temp_score = compute_gpu_temp_score(
            &gpu_temperatures,
            gpu_temp_threshold,
            gpu_temp_max,
            gpu_temp_weight,
        );
        let gpu_power_score = compute_gpu_power_score(
            &gpu_power_usages,
            gpu_power_threshold,
            gpu_power_max,
            gpu_power_weight,
        );
        let gpu_performance_score =
            gpu_score_weight * (gpu_vram_score + gpu_exec_score + gpu_temp_score + gpu_power_score);

        // --- CPU Subscore --- //
        let cpu_score = compute_cpu_score(cpu_usage, num_cpus, cpu_frequency, cpu_score_weight);

        // --- Memory Subscore --- //
        let ram_score = compute_ram_score(ram_used, ram_total, ram_score_weight);
        let swap_ram_score =
            compute_swap_ram_score(ram_swap_used, ram_swap_total, swap_ram_score_weight);

        // --- Network Subscore --- //
        let network_score = compute_network_score(network_rx, network_tx, network_score_weight);

        // --- Total Performance Score --- //
        let total_current_performance_score = compute_total_current_performance_score(
            cpu_score,
            gpu_performance_score,
            ram_score,
            swap_ram_score,
            network_score,
        );

        // --- Exponential Moving Average --- //
        let total_performance_score = compute_exponential_moving_average(
            total_current_performance_score,
            previous_performance_score.map(|score| score.performance_score),
            moving_avg_smooth_factor,
            moving_avg_window_size,
        );

        Ok(NodePerformance {
            total_performance_score,
        })
    }

    /// Calculates a weighted score representing GPU VRAM utilization across multiple GPUs.
    ///
    /// This function computes a score based on the free VRAM percentage of the most utilized GPU.
    /// A lower score indicates higher VRAM utilization, which is generally preferred for optimal
    /// KV cache allocation in machine learning workloads.
    ///
    /// # Arguments
    ///
    /// * `gpu_memory_free` - A slice containing the amount of free memory (in bytes) for each GPU
    /// * `gpu_memory_total` - A slice containing the total memory capacity (in bytes) for each GPU
    /// * `gpu_vram_weight` - A weight factor to scale the final score (typically between 0.0 and 1.0)
    ///
    /// # Returns
    ///
    /// Returns a weighted score where:
    /// - Lower scores indicate higher VRAM utilization (preferred)
    /// - Higher scores indicate lower VRAM utilization
    /// - The score is scaled by `gpu_vram_weight`
    /// - Returns 0.0 if there are no GPUs or if the slices are empty
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let gpu_memory_free = vec![4_000_000_000, 6_000_000_000];  // 4GB and 6GB free
    /// let gpu_memory_total = vec![16_000_000_000, 16_000_000_000];  // Two 16GB GPUs
    /// let gpu_vram_weight = 0.3;
    ///
    /// let score = compute_gpu_vram_utilization_score(
    ///     &gpu_memory_free,
    ///     &gpu_memory_total,
    ///     gpu_vram_weight
    /// );
    /// // score ≈ 0.075 (0.3 * (4GB/16GB))
    /// ```
    ///
    /// # Notes
    ///
    /// - The function uses the GPU with the lowest free memory percentage to calculate the score,
    ///   as this represents the most constrained resource
    /// - The calculation assumes that higher VRAM utilization is better for performance
    /// - Both input slices must be of equal length, representing the same GPUs in the same order
    /// - Returns -1.0 if the input slices are empty (TODO: We will introduce other accelerators metrics in the future)
    #[allow(clippy::cast_precision_loss)]
    fn compute_gpu_vram_utilization_score(
        gpu_memory_free: &[i64],
        gpu_memory_total: &[i64],
        gpu_vram_weight: f64,
    ) -> f64 {
        // TODO: The current implementation is not correct. In production systems, using PagedAttention (e.g. vLLM), for optimized batched
        // inference, KV cache is structured in both virtual and physical memory blocks. Moreover, these blocks are
        // allocated in the VRAM, at the spawn of the node.
        //
        // The score should be computed based on the amount of how many virtual/physical blocks were allocated in total to a given model,
        // and how many requests in parallel such node could handle. This means, that such computation should be model dependent.
        //
        // In case no GPU is available in the system, we return -1.0 to penalize for the lack of accelerators.
        if gpu_memory_free.is_empty() || gpu_memory_total.is_empty() {
            return -1.0;
        }
        gpu_vram_weight
            * gpu_memory_free
                .iter()
                .zip(gpu_memory_total.iter())
                .map(|(free, total)| (*free as f64) / (*total as f64))
                .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap_or(0f64)
    }

    /// Calculates a weighted score representing GPU execution availability across multiple GPUs.
    ///
    /// This function computes a score based on the average execution time percentage across all GPUs,
    /// where lower execution time indicates more available GPU capacity. The final score is inverted
    /// and weighted to align with the scoring system where higher scores indicate better availability.
    ///
    /// # Arguments
    ///
    /// * `gpu_percentage_time_execution` - A slice containing execution time percentages (0.0 to 1.0) for each GPU,
    ///                                    where 1.0 represents 100% utilization
    /// * `gpu_exec_avail_weight` - A weight factor to scale the final score (typically between 0.0 and 1.0)
    ///
    /// # Returns
    ///
    /// Returns a weighted score where:
    /// - Higher scores indicate more available GPU capacity (preferred)
    /// - Lower scores indicate less available GPU capacity
    /// - The score is scaled by `gpu_exec_avail_weight`
    /// - Returns 0.0 if there are no GPUs or if the slice is empty
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let gpu_execution_times = vec![0.3, 0.5];  // 30% and 50% GPU utilization
    /// let gpu_exec_weight = 0.3;
    ///
    /// let score = compute_gpu_execution_score(&gpu_execution_times, gpu_exec_weight);
    /// // score ≈ 0.18 (0.3 * (1.0 - (0.3 + 0.5)/2))
    /// ```
    ///
    /// # Notes
    ///
    /// - The function averages execution percentages across all GPUs to get an overall utilization metric
    /// - The average is inverted (1.0 - avg) since lower utilization indicates better availability
    /// - The final score is weighted to integrate with the larger node performance scoring system
    /// - Returns -1.0 if the input slice is empty (TODO: We will introduce other accelerators metrics in the future)
    #[allow(clippy::cast_precision_loss)]
    fn compute_gpu_execution_score(
        gpu_percentage_time_execution: &[f64],
        gpu_exec_avail_weight: f64,
    ) -> f64 {
        if gpu_percentage_time_execution.is_empty() {
            return -1.0;
        }
        // Lower execution percentage is better so invert the average
        let avg_exec = gpu_percentage_time_execution.iter().sum::<f64>()
            / (gpu_percentage_time_execution.len() as f64);
        gpu_exec_avail_weight * (1.0 - avg_exec)
    }

    /// Calculates a normalized temperature score for a set of GPUs based on their temperatures.
    ///
    /// This function evaluates GPU temperatures against defined thresholds and produces a single score
    /// representing the thermal state of the worst-performing GPU. The scoring uses a step function that:
    ///
    /// - Returns 1.0 for temperatures <= threshold (optimal performance)
    /// - Returns 0.0 for temperatures >= max (critical temperature)
    /// - Linearly scales between 1.0 and 0.0 for temperatures between threshold and max
    ///
    /// The final score is determined by the lowest-scoring GPU, as the system's performance
    /// is constrained by its most thermally stressed component.
    ///
    /// # Arguments
    ///
    /// * `gpu_temperatures` - A slice of f64 values representing the temperature of each GPU in Celsius
    /// * `threshold` - The temperature threshold below which performance is considered optimal (e.g., 80.0°C)
    /// * `max` - The maximum acceptable temperature above which performance is considered critical (e.g., 100.0°C)
    ///
    /// # Returns
    ///
    /// Returns a f64 score between 0.0 and 1.0 where:
    /// - 1.0 indicates all GPUs are operating at or below the optimal threshold
    /// - 0.0 indicates at least one GPU has reached or exceeded the maximum temperature
    /// - Values between 0.0 and 1.0 indicate linear degradation based on the worst-performing GPU
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let temperatures = vec![75.0, 82.0, 95.0];
    /// let threshold = 80.0;
    /// let max = 100.0;
    ///
    /// let score = score_gpu_temp(&temperatures, threshold, max);
    /// // Returns approximately 0.25 because the worst temperature (95.0)
    /// // is 75% of the way between threshold (80.0) and max (100.0)
    /// ```
    ///
    /// # Notes
    ///
    /// - Returns 0.0 if the input slice is empty
    /// - Uses partial_cmp for floating point comparisons, defaulting to 0.0 if comparison fails
    /// - The linear scaling provides a smooth transition between optimal and critical temperatures
    /// - Returns -1.0 if the input slice is empty (TODO: We will introduce other accelerators metrics in the future)
    fn compute_gpu_temp_score(
        gpu_temperatures: &[f64],
        threshold: f64,
        max: f64,
        gpu_temp_weight: f64,
    ) -> f64 {
        if gpu_temperatures.is_empty() {
            return -1.0;
        }
        // For each GPU, assign:
        //   1.0 if temp <= threshold,
        //   0.0 if temp >= max,
        //   and linearly scaled in between.
        // Finally, take the minimum score across all GPUs.
        gpu_temp_weight
            * gpu_temperatures
                .iter()
                .map(|&temp| {
                    if temp <= threshold {
                        1.0
                    } else if temp >= max {
                        0.0
                    } else {
                        1.0 - ((temp - threshold) / (max - threshold))
                    }
                })
                .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap_or(0.0)
    }

    /// Calculates a normalized power usage score for a set of GPUs based on their power consumption.
    ///
    /// This function evaluates GPU power usage against defined thresholds and produces a single score
    /// representing the power efficiency state of the worst-performing GPU. The scoring uses a step function that:
    ///
    /// - Returns 1.0 for power usage <= threshold (optimal efficiency)
    /// - Returns 0.0 for power usage >= max (critical power consumption)
    /// - Linearly scales between 1.0 and 0.0 for power usage between threshold and max
    ///
    /// The final score is determined by the lowest-scoring GPU, as the system's efficiency
    /// is constrained by its most power-hungry component.
    ///
    /// # Arguments
    ///
    /// * `gpu_power_usages` - A slice of f64 values representing the power usage of each GPU in Watts
    /// * `gpu_power_threshold` - The power threshold below which efficiency is considered optimal (e.g., 200W)
    /// * `gpu_power_max` - The maximum acceptable power usage above which efficiency is considered critical (e.g., 300W)
    /// * `gpu_power_weight` - A weight factor to scale the final score (typically between 0.0 and 1.0)
    ///
    /// # Returns
    ///
    /// Returns a weighted f64 score between 0.0 and 1.0 where:
    /// - 1.0 indicates all GPUs are operating at or below the optimal power threshold
    /// - 0.0 indicates at least one GPU has reached or exceeded the maximum power usage
    /// - Values between 0.0 and 1.0 indicate linear degradation based on the worst-performing GPU
    /// - The final score is scaled by `gpu_power_weight`
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let power_usages = vec![180.0, 220.0, 250.0];  // Power usage in Watts
    /// let threshold = 200.0;
    /// let max = 300.0;
    /// let weight = 0.2;
    ///
    /// let score = compute_gpu_power_score(&power_usages, threshold, max, weight);
    /// // Returns approximately 0.1 because:
    /// // 1. Worst GPU (250W) is 50% between threshold (200W) and max (300W)
    /// // 2. Score of 0.5 is then weighted by 0.2
    /// ```
    ///
    /// # Notes
    ///
    /// - Returns 0.0 if the input slice is empty
    /// - Uses partial_cmp for floating point comparisons, defaulting to 0.0 if comparison fails
    /// - The linear scaling provides a smooth transition between optimal and critical power usage levels
    /// - This score can be used as part of a larger node performance evaluation system
    /// - Returns -1.0 if the input slice is empty (TODO: We will introduce other accelerators metrics in the future)
    fn compute_gpu_power_score(
        gpu_power_usages: &[f64],
        gpu_power_threshold: f64,
        gpu_power_max: f64,
        gpu_power_weight: f64,
    ) -> f64 {
        if gpu_power_usages.is_empty() {
            return -1.0;
        }
        // For each GPU, assign:
        //   1.0 if power <= threshold,
        //   0.0 if power >= max,
        //   and linearly scaled in between.
        // Finally, take the minimum score across all GPUs.
        gpu_power_weight
            * gpu_power_usages
                .iter()
                .map(|&power| {
                    if power <= gpu_power_threshold {
                        1.0
                    } else if power >= gpu_power_max {
                        0.0
                    } else {
                        1.0 - ((power - gpu_power_threshold)
                            / (gpu_power_max - gpu_power_threshold))
                    }
                })
                .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap_or(0.0)
    }

    /// Calculates a normalized CPU performance score based on CPU usage, number of cores, and frequency.
    ///
    /// This function computes a weighted score that considers both CPU utilization and frequency:
    /// - CPU usage is normalized by the number of cores and inverted so lower usage yields higher scores
    /// - CPU frequency is normalized assuming a typical range of 1.0 to 5.0 GHz
    /// - The final score combines usage and frequency with equal weights
    ///
    /// # Arguments
    ///
    /// * `cpu_usage` - CPU utilization as a fraction between 0.0 and 1.0 (e.g., 0.5 = 50% usage)
    /// * `num_cpus` - Number of CPU cores available
    /// * `cpu_frequency` - Current CPU frequency in GHz (e.g., 3.5 for 3.5 GHz)
    /// * `cpu_score_weight` - Weight factor to scale the final score (typically between 0.0 and 1.0)
    ///
    /// # Returns
    ///
    /// Returns a weighted score between 0.0 and `cpu_score_weight` where higher values indicate better CPU availability
    /// and performance. The score is calculated as:
    /// ```text
    /// cpu_score_weight * (0.5 * usage_score + 0.5 * freq_score)
    /// ```
    /// where:
    /// - `usage_score` = 1.0 - (cpu_usage / num_cpus)
    /// - `freq_score` = min(cpu_frequency / 5.0, 1.0)
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// # use your_crate::compute_cpu_score;
    /// // CPU at 50% usage across 4 cores (12.5% per core), 3.2 GHz, weight of 0.2
    /// let score = compute_cpu_score(0.5, 4, 3.2, 0.2);
    /// // Returns ~0.17 because:
    /// // usage_score = 1.0 - (0.5/4) = 0.875
    /// // freq_score = 3.2/5.0 = 0.64
    /// // final_score = 0.2 * (0.5 * 0.875 + 0.5 * 0.64) = 0.2 * 0.7575 = 0.1515
    /// ```
    ///
    /// # Notes
    ///
    /// - CPU usage is normalized by the number of cores to better represent per-core availability
    /// - Frequency is normalized assuming 5.0 GHz as a typical maximum
    /// - The score equally weights normalized usage and frequency
    /// - Higher scores indicate better CPU performance and availability for tasks
    #[allow(clippy::cast_precision_loss)]
    fn compute_cpu_score(
        cpu_usage: f64,
        num_cpus: u32,
        cpu_frequency: u64,
        cpu_score_weight: f64,
    ) -> f64 {
        // Normalize CPU usage score (0.0 to 1.0, higher is better)
        let usage_score = (cpu_usage / f64::from(num_cpus)).mul_add(-1.0, 1.0);

        // NOTE: Normalize frequency score (assuming typical range of 1.0 to 5.0 GHz)
        let freq_score = (cpu_frequency as f64 / 5000.0).clamp(0.0, 1.0);

        // Combine scores with equal weight between usage and frequency
        cpu_score_weight * 0.5f64.mul_add(usage_score, 0.5 * freq_score)
    }

    /// Calculates a normalized RAM usage score based on memory utilization.
    ///
    /// This function computes a score based on the percentage of RAM used, where lower usage
    /// indicates better performance. The score is linearly scaled between 0.0 and 1.0, with 0.0
    /// representing 100% RAM usage and 1.0 representing 0% RAM usage. The final score is weighted
    /// by the provided `ram_score_weight` factor.
    ///
    /// # Arguments
    ///
    /// * `ram_used` - The amount of RAM currently in use (in bytes)
    /// * `ram_total` - The total amount of RAM available (in bytes)
    /// * `ram_score_weight` - A weight factor to scale the final score (typically between 0.0 and 1.0)
    ///
    /// # Returns
    ///
    /// Returns a weighted f64 score between 0.0 and `ram_score_weight` where:
    /// - `ram_score_weight` indicates 0% RAM usage (best performance)
    /// - 0.0 indicates 100% RAM usage (worst performance)
    /// - Values between are linearly scaled based on RAM usage percentage
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let ram_used = 8 * 1024 * 1024 * 1024;  // 8GB used
    /// let ram_total = 16 * 1024 * 1024 * 1024;  // 16GB total
    /// let weight = 0.15;
    ///
    /// let score = compute_ram_score(ram_used, ram_total, weight);
    /// // Returns 0.075 because:
    /// // 1. RAM usage is 50% (8GB/16GB)
    /// // 2. Available RAM percentage is 50% (1.0 - 0.5)
    /// // 3. Final score is 0.5 * 0.15 = 0.075
    /// ```
    ///
    /// # Notes
    ///
    /// - The function assumes valid input where `ram_used` <= `ram_total`
    /// - The score is part of a larger node performance evaluation system
    /// - Higher scores indicate better RAM availability for tasks
    #[allow(clippy::cast_precision_loss)]
    fn compute_ram_score(ram_used: i64, ram_total: i64, ram_score_weight: f64) -> f64 {
        let ram_used_percentage = (ram_used as f64) / (ram_total as f64);
        ram_score_weight * (1.0 - ram_used_percentage)
    }

    /// Calculates a normalized swap memory usage score based on swap space utilization.
    ///
    /// This function computes a score based on the percentage of swap memory used, where lower usage
    /// indicates better performance. The score is linearly scaled between 0.0 and 1.0, with 0.0
    /// representing 100% swap usage and 1.0 representing 0% swap usage. The final score is weighted
    /// by the provided `swap_ram_score_weight` factor.
    ///
    /// # Arguments
    ///
    /// * `ram_swap_used` - The amount of swap memory currently in use (in bytes)
    /// * `ram_swap_total` - The total amount of swap memory available (in bytes)
    /// * `swap_ram_score_weight` - A weight factor to scale the final score (typically between 0.0 and 1.0)
    ///
    /// # Returns
    ///
    /// Returns a weighted f64 score between 0.0 and `swap_ram_score_weight` where:
    /// - `swap_ram_score_weight` indicates 0% swap usage (best performance)
    /// - 0.0 indicates 100% swap usage (worst performance)
    /// - Values between are linearly scaled based on swap usage percentage
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let swap_used = 2 * 1024 * 1024 * 1024;  // 2GB swap used
    /// let swap_total = 8 * 1024 * 1024 * 1024;  // 8GB total swap
    /// let weight = 0.05;
    ///
    /// let score = compute_swap_ram_score(swap_used, swap_total, weight);
    /// // Returns 0.0375 because:
    /// // 1. Swap usage is 25% (2GB/8GB)
    /// // 2. Available swap percentage is 75% (1.0 - 0.25)
    /// // 3. Final score is 0.75 * 0.05 = 0.0375
    /// ```
    ///
    /// # Notes
    ///
    /// - The function assumes valid input where `ram_swap_used` <= `ram_swap_total`
    /// - The score is part of a larger node performance evaluation system
    /// - Higher scores indicate better swap memory availability
    /// - Swap memory usage is generally less critical than RAM usage, hence typically lower weights
    #[allow(clippy::cast_precision_loss)]
    fn compute_swap_ram_score(
        ram_swap_used: i64,
        ram_swap_total: i64,
        swap_ram_score_weight: f64,
    ) -> f64 {
        let ram_swap_used_percentage = (ram_swap_used as f64) / (ram_swap_total as f64);
        swap_ram_score_weight * (1.0 - ram_swap_used_percentage)
    }

    /// Calculates a normalized network performance score based on network traffic.
    ///
    /// This function computes a score based on the total network traffic (receive + transmit),
    /// where lower traffic indicates better network availability. The score uses an inverse
    /// relationship with traffic volume, scaled by the provided weight factor.
    ///
    /// # Arguments
    ///
    /// * `network_rx` - The network receive traffic in bytes
    /// * `network_tx` - The network transmit traffic in bytes
    /// * `network_score_weight` - A weight factor to scale the final score (typically between 0.0 and 1.0)
    ///
    /// # Returns
    ///
    /// Returns a weighted f64 score where:
    /// - Higher scores indicate lower network utilization (better availability)
    /// - Lower scores indicate higher network utilization
    /// - The score is scaled by `network_score_weight`
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let rx = 1000;  // 1KB received
    /// let tx = 2000;  // 2KB transmitted
    /// let weight = 0.2;
    ///
    /// let score = compute_network_score(rx, tx, weight);
    /// // Returns 0.066 because:
    /// // 1. Base score = 1 / (1 + 1000 + 2000) = 0.333
    /// // 2. Final score = 0.333 * 0.2 = 0.066
    /// ```
    ///
    /// # Notes
    ///
    /// - Uses an inverse relationship to ensure higher traffic results in lower scores
    /// - The denominator adds 1 to prevent division by zero
    /// - The score is part of a larger node performance evaluation system
    #[allow(clippy::cast_precision_loss)]
    fn compute_network_score(network_rx: i64, network_tx: i64, network_score_weight: f64) -> f64 {
        let total_network_usage = network_rx as f64 + network_tx as f64;
        network_score_weight * (1.0 / (1.0 + total_network_usage))
    }

    /// Computes the total current performance score based on the individual scores and weights.
    ///
    /// This function takes the individual performance scores and weights for each component and
    /// computes the total performance score by summing them up.
    ///
    /// # Arguments
    ///
    /// * `cpu_score` - The score for the CPU performance
    /// * `gpu_performance_score` - The score for the GPU performance
    /// * `ram_score` - The score for the RAM performance
    /// * `swap_ram_score` - The score for the swap RAM performance
    /// * `network_score` - The score for the network performance
    ///
    /// # Returns
    ///
    /// Returns a f64 score between 0.0 and 1.0 where:
    /// - 0.0 indicates the worst performance
    /// - 1.0 indicates the best performance
    fn compute_total_current_performance_score(
        cpu_score: f64,
        gpu_performance_score: f64,
        ram_score: f64,
        swap_ram_score: f64,
        network_score: f64,
    ) -> f64 {
        cpu_score + gpu_performance_score + ram_score + swap_ram_score + network_score
    }

    /// Computes an exponential moving average (EMA) based on the current value and previous average.
    ///
    /// This function calculates a weighted average that gives more importance to recent values while
    /// still considering historical data. The weighting is determined by the smooth factor and window size.
    ///
    /// # Arguments
    ///
    /// * `current_value` - The newest value to include in the average
    /// * `previous_average` - Option containing the previous moving average, if any
    /// * `smooth_factor` - Factor controlling how quickly the EMA responds to new values (0.0 to 1.0)
    /// * `window_size` - Size of the moving window for the average
    ///
    /// # Returns
    ///
    /// Returns the new exponential moving average as an f64.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let current = 0.75;
    /// let previous = Some(0.70);
    /// let smooth_factor = 0.7;
    /// let window_size = 10;
    ///
    /// let ema = compute_exponential_moving_average(current, previous, smooth_factor, window_size);
    /// ```
    ///
    /// # Notes
    ///
    /// - If no previous average exists, returns the current value
    /// - The smooth factor determines how quickly the EMA responds to changes:
    ///   - Higher values (closer to 1.0) give more weight to recent values
    ///   - Lower values (closer to 0.0) give more weight to historical values
    /// - The window size affects the overall smoothing effect:
    ///   - Larger windows result in more gradual changes
    ///   - Smaller windows allow for more rapid adaptation to new values
    fn compute_exponential_moving_average(
        current_value: f64,
        previous_average: Option<f64>,
        smooth_factor: f64,
        window_size: i32,
    ) -> f64 {
        previous_average.map_or(current_value, |prev_avg| {
            let alpha = smooth_factor / (1.0 + f64::from(window_size));
            current_value.mul_add(alpha, prev_avg * (1.0 - alpha))
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        use crate::state_manager::tests::{setup_test_db, truncate_tables};
        use proptest::{collection::vec, prop_assert, prop_assert_eq, proptest};

        #[test]
        fn test_exponential_moving_average() {
            // Test with no previous average
            let ema1 = compute_exponential_moving_average(0.5, None, 0.7, 10);
            assert!((ema1 - 0.5).abs() < 1e-10);

            // Test with previous average
            let ema2 = compute_exponential_moving_average(0.8, Some(0.5), 0.7, 10);
            // Expected: 0.8 * (0.7/11) + 0.5 * (1 - 0.7/11) ≈ 0.52
            assert!((ema2 - 0.519_090_909_090_909).abs() < 1e-10);

            // Test with extreme smooth factor
            let ema3 = compute_exponential_moving_average(1.0, Some(0.0), 1.0, 10);
            // Expected: 1.0 * (1.0/11) + 0.0 * (1 - 1.0/11) ≈ 0.09
            assert!((ema3 - 0.090_909_090_909_090_91).abs() < 1e-10);

            // Test with zero smooth factor
            let ema4 = compute_exponential_moving_average(1.0, Some(0.5), 0.0, 10);
            // Expected: 1.0 * (0.0/11) + 0.5 * (1 - 0.0/11) ≈ 0.5
            assert!((ema4 - 0.5).abs() < 1e-10);
        }

        // Helper function to create default performance weights
        const fn default_performance_weights() -> PerformanceWeights {
            PerformanceWeights {
                gpu_score_weight: 0.4,
                cpu_score_weight: 0.2,
                ram_score_weight: 0.15,
                swap_ram_score_weight: 0.05,
                network_score_weight: 0.2,
                gpu_vram_weight: 0.3,
                gpu_exec_avail_weight: 0.3,
                gpu_temp_weight: 0.2,
                gpu_power_weight: 0.2,
                gpu_temp_threshold: 80.0,
                gpu_temp_max: 100.0,
                gpu_power_threshold: 200.0,
                gpu_power_max: 300.0,
                moving_avg_window_size: 10,
                moving_avg_smooth_factor: 0.7,
            }
        }

        async fn insert_performance_weights(state: &AtomaState) -> Result<()> {
            state
                .insert_performance_weights(default_performance_weights())
                .await?;
            Ok(())
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn test_optimal_performance() {
            let state = setup_test_db().await;
            truncate_tables(&state.db).await;
            insert_performance_weights(&state)
                .await
                .expect("Failed to insert performance weights");

            let result = get_node_performance(
                &state,
                1,
                1,
                0.5,                     // Balanced CPU usage
                2500,                    // 2.5 GHz CPU frequency
                8 * 1024 * 1024 * 1024,  // 8GB RAM used
                16 * 1024 * 1024 * 1024, // 16GB RAM total
                0,                       // No swap used
                8 * 1024 * 1024 * 1024,  // 8GB swap total
                1000,                    // Low network usage
                1000,
                vec![14 * 1024 * 1024 * 1024, 14 * 1024 * 1024 * 1024], // 8GB VRAM used per GPU
                vec![16 * 1024 * 1024 * 1024, 16 * 1024 * 1024 * 1024], // 16GB VRAM total per GPU
                vec![0.3, 0.3],                                         // Low GPU execution time
                vec![60.0, 65.0],                                       // Good temperatures
                vec![150.0, 160.0],                                     // Efficient power usage
            )
            .await
            .unwrap();
            assert!(
                result.total_performance_score > 0.57,
                "Expected high performance score, got {}",
                result.total_performance_score
            );
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn test_poor_performance() {
            let state = setup_test_db().await;
            truncate_tables(&state.db).await;
            insert_performance_weights(&state)
                .await
                .expect("Failed to insert performance weights");

            let result = get_node_performance(
                &state,
                1,
                1,
                0.95,                    // High CPU usage
                2000,                    // 2 GHz CPU frequency
                15 * 1024 * 1024 * 1024, // Nearly full RAM
                16 * 1024 * 1024 * 1024,
                7 * 1024 * 1024 * 1024, // High swap usage
                8 * 1024 * 1024 * 1024,
                100_000, // High network usage
                100_000,
                vec![15 * 1024 * 1024 * 1024, 15 * 1024 * 1024 * 1024], // High VRAM usage
                vec![16 * 1024 * 1024 * 1024, 16 * 1024 * 1024 * 1024],
                vec![0.9, 0.9],     // High GPU utilization
                vec![95.0, 98.0],   // High temperatures
                vec![280.0, 290.0], // High power usage
            )
            .await
            .unwrap();

            assert!(
                result.total_performance_score < 1.0,
                "Expected low performance score, got {}",
                result.total_performance_score
            );
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn test_single_gpu_system() {
            let state = setup_test_db().await;
            truncate_tables(&state.db).await;
            insert_performance_weights(&state)
                .await
                .expect("Failed to insert performance weights");

            let result = get_node_performance(
                &state,
                1,
                1,
                0.6,
                2500,
                8 * 1024 * 1024 * 1024,
                16 * 1024 * 1024 * 1024,
                0,
                8 * 1024 * 1024 * 1024,
                2000,
                2000,
                vec![8 * 1024 * 1024 * 1024],
                vec![16 * 1024 * 1024 * 1024],
                vec![0.4],
                vec![70.0],
                vec![180.0],
            )
            .await
            .unwrap();
            assert!(
                result.total_performance_score > 0.0,
                "Single GPU system should produce valid score, got {}",
                result.total_performance_score
            );
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn test_no_gpus() {
            let state = setup_test_db().await;
            truncate_tables(&state.db).await;
            insert_performance_weights(&state)
                .await
                .expect("Failed to insert performance weights");

            let result = get_node_performance(
                &state,
                1,
                1,
                0.5,
                2500,
                8 * 1024 * 1024 * 1024,
                16 * 1024 * 1024 * 1024,
                8 * 1024 * 1024 * 1024,
                1000,
                1000,
                0,
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
            )
            .await
            .unwrap();

            assert!(
                result.total_performance_score < 0.0,
                "System without GPUs should still get valid score, got {}",
                result.total_performance_score
            );
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn test_temperature_thresholds() {
            let state = setup_test_db().await;
            truncate_tables(&state.db).await;
            insert_performance_weights(&state)
                .await
                .expect("Failed to insert performance weights");

            // Test different temperature scenarios
            let temps = vec![
                (vec![70.0, 75.0], "below threshold"),
                (vec![85.0, 90.0], "between threshold and max"),
                (vec![105.0, 110.0], "above max"),
            ];

            for (gpu_temps, scenario) in temps {
                let result = get_node_performance(
                    &state,
                    1,
                    1,
                    0.5,
                    2500,
                    8 * 1024 * 1024 * 1024,
                    16 * 1024 * 1024 * 1024,
                    0,
                    8 * 1024 * 1024 * 1024,
                    1000,
                    1000,
                    vec![8 * 1024 * 1024 * 1024, 8 * 1024 * 1024 * 1024],
                    vec![16 * 1024 * 1024 * 1024, 16 * 1024 * 1024 * 1024],
                    vec![0.3, 0.3],
                    gpu_temps,
                    vec![150.0, 160.0],
                )
                .await
                .unwrap();
                match scenario {
                    "below threshold" => assert!(
                        result.total_performance_score > 0.52,
                        "Expected higher score for low temperatures, got {}",
                        result.total_performance_score
                    ),
                    "between threshold and max" => assert!(
                        result.total_performance_score > 0.48
                            && result.total_performance_score < 0.49,
                        "Expected medium score for medium temperatures, got {}",
                        result.total_performance_score
                    ),
                    "above max" => assert!(
                        result.total_performance_score < 0.45,
                        "Expected low score for high temperatures, got {}",
                        result.total_performance_score
                    ),
                    _ => unreachable!(),
                }
            }
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn test_power_usage_thresholds() {
            let state = setup_test_db().await;
            truncate_tables(&state.db).await;
            insert_performance_weights(&state)
                .await
                .expect("Failed to insert performance weights");

            // Test different power usage scenarios
            let power_usages = vec![
                (vec![150.0, 160.0], "below threshold"),
                (vec![250.0, 260.0], "between threshold and max"),
                (vec![310.0, 320.0], "above max"),
            ];

            for (gpu_power, scenario) in power_usages {
                let result = get_node_performance(
                    &state,
                    1,
                    1,
                    0.5,
                    2500,
                    8 * 1024 * 1024 * 1024,
                    16 * 1024 * 1024 * 1024,
                    0,
                    8 * 1024 * 1024 * 1024,
                    1000,
                    1000,
                    vec![8 * 1024 * 1024 * 1024, 8 * 1024 * 1024 * 1024],
                    vec![16 * 1024 * 1024 * 1024, 16 * 1024 * 1024 * 1024],
                    vec![0.3, 0.3],
                    vec![70.0, 75.0],
                    gpu_power,
                )
                .await
                .unwrap();
                match scenario {
                    "below threshold" => assert!(
                        result.total_performance_score > 0.52,
                        "Expected higher score for low power usage, got {}",
                        result.total_performance_score
                    ),
                    "between threshold and max" => assert!(
                        result.total_performance_score > 0.48
                            && result.total_performance_score < 0.49,
                        "Expected medium score for medium power usage, got {}",
                        result.total_performance_score
                    ),
                    "above max" => assert!(
                        result.total_performance_score < 0.45,
                        "Expected low score for high power usage, got {}",
                        result.total_performance_score
                    ),
                    _ => unreachable!(),
                }
            }
        }

        proptest! {
            #[test]
            fn test_compute_gpu_vram_score(
                // Generate base free memory values and increases
                free1 in vec(1_i64..500_000_000_000_i64, 10),
                free_increase in vec(1_i64..500_000_000_000_i64, 10),
                total in vec(1_000_000_000_000_i64..2_000_000_000_000_i64, 10),
                weight in 0.0..1.0f64,
            ) {
                // Create second free vector by adding increases
                let free2: Vec<i64> = free1.iter()
                    .zip(free_increase.iter())
                    .map(|(&f1, &inc)| f1 + inc)
                    .collect();

                // Ensure all vectors have the same length
                prop_assert_eq!(free1.len(), 10);
                prop_assert_eq!(free2.len(), 10);
                prop_assert_eq!(total.len(), 10);

                let score1 = compute_gpu_vram_utilization_score(&free1, &total, weight);
                let score2 = compute_gpu_vram_utilization_score(&free2, &total, weight);

                // Basic range check
                prop_assert!(score1 >= -1.0 && score1 <= weight);
                prop_assert!(score2 >= -1.0 && score2 <= weight);

                // Verify that score increases with more free memory
                prop_assert!(score2 > score1,
                    "Score should increase with more free memory. Score1: {}, Score2: {}, Free1: {:?}, Free2: {:?}",
                    score1, score2, free1, free2
                );
            }

            #[test]
            #[allow(clippy::cast_precision_loss)]
            fn test_compute_gpu_execution_score(
                // Generate base execution times and increases (as percentages)
                exec_times1 in vec(0.0..0.9f64, 10),
                exec_increase in vec(0.01..0.1f64, 10),
                weight in 0.0..1.0f64,
            ) {
                // Create second execution times vector by adding increases
                let exec_times2: Vec<f64> = exec_times1.iter()
                    .zip(exec_increase.iter())
                    .map(|(&e1, &inc)| (e1 + inc).min(1.0))
                    .collect();

                // Ensure all vectors have the same length
                prop_assert_eq!(exec_times1.len(), 10);
                prop_assert_eq!(exec_times2.len(), 10);

                let score1 = compute_gpu_execution_score(&exec_times1, weight);
                let score2 = compute_gpu_execution_score(&exec_times2, weight);

                // Basic range check
                prop_assert!(score1 >= -1.0 && score1 <= weight);
                prop_assert!(score2 >= -1.0 && score2 <= weight);

                // Verify that score decreases with more execution time
                prop_assert!(score2 < score1,
                    "Score should decrease with more execution time. Score1: {}, Score2: {}, Exec1: {:?}, Exec2: {:?}",
                    score1, score2, exec_times1, exec_times2
                );

                // Verify linear relationship
                let avg_exec1: f64 = exec_times1.iter().sum::<f64>() / exec_times1.len() as f64;
                let avg_exec2: f64 = exec_times2.iter().sum::<f64>() / exec_times2.len() as f64;
                let exec_diff = avg_exec2 - avg_exec1;
                let score_diff = score1 - score2;

                // The score difference should be proportional to execution time difference
                // score = weight * (1 - avg_exec)
                // Therefore: score_diff = weight * exec_diff
                prop_assert!(weight.mul_add(-exec_diff, score_diff).abs() < 1e-10,
                    "Score difference should be proportional to execution time difference. \
                    Expected: {}, Actual: {}, Exec diff: {}, Weight: {}",
                    weight * exec_diff, score_diff, exec_diff, weight
                );
            }

            #[test]
            fn test_compute_gpu_temp_score(
                // Generate temperatures in three ranges: below threshold, between, and above max
                temps_below in vec(20.0..80.0f64, 1..5),     // Below threshold (80°C)
                temps_between in vec(80.0..100.0f64, 1..5),  // Between threshold and max
                temps_above in vec(100.0..120.0f64, 1..5),   // Above max (100°C)
                threshold in 75.0..85.0f64,                   // Allow some variance in threshold
                max in 95.0..105.0f64,                       // Allow some variance in max
                weight in 0.0..1.0f64,
            ) {
                // Verify threshold is less than max
                prop_assert!(threshold < max);

                // Test temperatures below threshold (should all give weight)
                if temps_below.iter().all(|&t| t <= threshold) {
                    let score_below = compute_gpu_temp_score(&temps_below, threshold, max, weight);
                    prop_assert!((score_below - weight).abs() < 1e-10,
                        "Temperatures below threshold should give full weight score. \
                        Expected: {}, Got: {}, Temps: {:?}",
                        weight, score_below, temps_below
                    );
                }

                // Test temperatures above max (should all give 0)
                if temps_above.iter().all(|&t| t >= max) {
                    let score_above = compute_gpu_temp_score(&temps_above, threshold, max, weight);
                    prop_assert!(score_above.abs() < 1e-10,
                        "Temperatures above max should give zero score. \
                        Got: {}, Temps: {:?}",
                        score_above, temps_above
                    );
                }

                // Test linear scaling for temperatures between threshold and max
                if !temps_between.is_empty() {
                    let score_between = compute_gpu_temp_score(&temps_between, threshold, max, weight);

                    // Get the highest temperature (worst case determines score)
                    let max_temp = temps_between.iter()
                        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                        .unwrap();

                    if (*max_temp < max) && (*max_temp > threshold) {
                        // Calculate expected score for the highest temperature
                        let expected_score = weight * (1.0 - (*max_temp - threshold) / (max - threshold));

                        prop_assert!((score_between - expected_score).abs() < 1e-10,
                            "Temperature between threshold and max should scale linearly. \
                            Expected: {}, Got: {}, Max Temp: {}, Threshold: {}, Max: {}",
                            expected_score, score_between, max_temp, threshold, max
                        );
                    } else if *max_temp < threshold {
                        prop_assert!((score_between - weight).abs() < 1e-10,
                            "Temperature between threshold and max should scale linearly. \
                            Got: {}, Expected: {}, Max Temp: {}, Threshold: {}, Max: {}",
                            score_between, weight, max_temp, threshold, max
                        );
                    } else  {
                        prop_assert!(score_between.abs() < 1e-10,
                            "Temperature between threshold and max should scale linearly. \
                            Got: {}, Max Temp: {}, Threshold: {}, Max: {}",
                            score_between, max_temp, threshold, max
                        );
                    }
                }

                // Test monotonic decrease across ranges
                let mut all_temps = Vec::new();
                all_temps.extend(temps_below);
                all_temps.extend(temps_between);
                all_temps.extend(temps_above);

                if all_temps.len() >= 2 {
                    let scores: Vec<(f64, f64)> = all_temps.windows(2)
                        .map(|window| {
                            let score1 = compute_gpu_temp_score(&[window[0]], threshold, max, weight);
                            let score2 = compute_gpu_temp_score(&[window[1]], threshold, max, weight);
                            if window[0] < window[1] {
                                (score1, score2)
                            } else {
                                (score2, score1)
                            }
                        })
                        .collect();

                    for (score1, score2) in scores {
                        prop_assert!(score1 >= score2,
                            "Scores should monotonically decrease with increasing temperature. \
                            Score1: {}, Score2: {}",
                            score1, score2
                        );
                    }
                }
            }

            #[test]
            fn test_compute_gpu_power_score(
                // Generate power usage in three ranges: below threshold, between, and above max
                power_below in vec(50.0..200.0f64, 1..5),     // Below threshold (200W)
                power_between in vec(200.0..300.0f64, 1..5),  // Between threshold and max
                power_above in vec(300.0..400.0f64, 1..5),    // Above max (300W)
                threshold in 180.0..220.0f64,                 // Allow some variance in threshold
                max in 280.0..320.0f64,                       // Allow some variance in max
                weight in 0.0..1.0f64,
            ) {
                // Verify threshold is less than max
                prop_assert!(threshold < max);

                // Test power usage below threshold (should all give weight)
                if power_below.iter().all(|&p| p <= threshold) {
                    let score_below = compute_gpu_power_score(&power_below, threshold, max, weight);
                    prop_assert!((score_below - weight).abs() < 1e-10,
                        "Power usage below threshold should give full weight score. \
                        Expected: {}, Got: {}, Power: {:?}",
                        weight, score_below, power_below
                    );
                }

                // Test power usage above max (should all give 0)
                if power_above.iter().all(|&p| p >= max) {
                    let score_above = compute_gpu_power_score(&power_above, threshold, max, weight);
                    prop_assert!(score_above.abs() < 1e-10,
                        "Power usage above max should give zero score. \
                        Got: {}, Power: {:?}",
                        score_above, power_above
                    );
                }

                // Test linear scaling for power usage between threshold and max
                if !power_between.is_empty() {
                    let score_between = compute_gpu_power_score(&power_between, threshold, max, weight);

                    // Get the highest power usage (worst case determines score)
                    let max_power = power_between.iter()
                        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                        .unwrap();

                    if (*max_power < max) && (*max_power > threshold) {
                        // Calculate expected score for the highest power usage
                        let expected_score = weight * (1.0 - (*max_power - threshold) / (max - threshold));

                        prop_assert!((score_between - expected_score).abs() < 1e-10,
                            "Power usage between threshold and max should scale linearly. \
                            Expected: {}, Got: {}, Max Power: {}, Threshold: {}, Max: {}",
                            expected_score, score_between, max_power, threshold, max
                        );
                    } else if *max_power < threshold {
                        prop_assert!((score_between - weight).abs() < 1e-10,
                            "Power usage between threshold and max should scale linearly. \
                            Got: {}, Expected: {}, Max Power: {}, Threshold: {}, Max: {}",
                            score_between, weight, max_power, threshold, max
                        );
                    } else {
                        prop_assert!(score_between.abs() < 1e-10,
                            "Power usage between threshold and max should scale linearly. \
                            Got: {}, Max Power: {}, Threshold: {}, Max: {}",
                            score_between, max_power, threshold, max
                        );
                    }
                }

                // Test monotonic decrease across ranges
                let mut all_power = Vec::new();
                all_power.extend(power_below);
                all_power.extend(power_between);
                all_power.extend(power_above);

                if all_power.len() >= 2 {
                    let scores: Vec<(f64, f64)> = all_power.windows(2)
                        .map(|window| {
                            let score1 = compute_gpu_power_score(&[window[0]], threshold, max, weight);
                            let score2 = compute_gpu_power_score(&[window[1]], threshold, max, weight);
                            if window[0] < window[1] {
                                (score1, score2)
                            } else {
                                (score2, score1)
                            }
                        })
                        .collect();

                    for (score1, score2) in scores {
                        prop_assert!(score1 >= score2,
                            "Scores should monotonically decrease with increasing power usage. \
                            Score1: {}, Score2: {}",
                            score1, score2
                        );
                    }
                }
            }

            #[test]
            fn test_compute_cpu_score(
                cpu_usage in 0.0..1.0f64,
                num_cpus in 1u32..128u32,
                cpu_frequency in 1000u64..5000u64,  // 1-5 GHz
                weight in 0.0..1.0f64,
            ) {
                let score = compute_cpu_score(cpu_usage, num_cpus, cpu_frequency, weight);

                // Basic range check
                prop_assert!(score >= 0.0 && score <= weight);

                // Test monotonicity with respect to CPU usage (higher usage should give lower score)
                if cpu_usage < 0.9 {  // Leave room for increase
                    let higher_usage_score = compute_cpu_score(cpu_usage + 0.1, num_cpus, cpu_frequency, weight);
                    prop_assert!(higher_usage_score < score,
                        "Score should decrease with higher CPU usage. Original: {}, Higher Usage: {}",
                        score, higher_usage_score
                    );
                }

                // Test monotonicity with respect to CPU frequency (higher frequency should give higher score)
                if cpu_frequency < 4900 {  // Leave room for increase
                    let higher_freq_score = compute_cpu_score(cpu_usage, num_cpus, cpu_frequency + 100, weight);
                    prop_assert!(higher_freq_score > score,
                        "Score should increase with higher CPU frequency. Original: {}, Higher Freq: {}",
                        score, higher_freq_score
                    );
                }

                // Test scaling with weight
                let half_weight_score = compute_cpu_score(cpu_usage, num_cpus, cpu_frequency, weight / 2.0);
                prop_assert!(half_weight_score.mul_add(2.0, -score) < 1e-10,
                    "Score should scale linearly with weight. Full: {}, Half: {}",
                    score, half_weight_score
                );
            }

            #[test]
            #[allow(clippy::cast_precision_loss)]
            fn test_compute_ram_score(
                // Generate RAM values ensuring used <= total
                ram_total in 1_000_000_000i64..1_000_000_000_000i64,
                ram_used_percentage in 0.0..1.0f64,
                weight in 0.0..1.0f64,
            ) {
                let ram_used = (ram_total as f64 * ram_used_percentage) as i64;
                let score = compute_ram_score(ram_used, ram_total, weight);

                // Basic range check
                prop_assert!(score >= 0.0 && score <= weight);

                // Test perfect score when no RAM is used
                let perfect_score = compute_ram_score(0, ram_total, weight);
                prop_assert!((perfect_score - weight).abs() < 1e-10,
                    "Should get maximum score when no RAM is used. Expected: {}, Got: {}",
                    weight, perfect_score
                );

                // Test zero score when all RAM is used
                let worst_score = compute_ram_score(ram_total, ram_total, weight);
                prop_assert!(worst_score.abs() < 1e-10,
                    "Should get zero score when all RAM is used. Got: {}",
                    worst_score
                );

                // Test monotonicity
                if ram_used < ram_total {
                    let higher_usage_score = compute_ram_score(ram_used + 1, ram_total, weight);
                    prop_assert!(higher_usage_score < score,
                        "Score should decrease with higher RAM usage. Original: {}, Higher Usage: {}",
                        score, higher_usage_score
                    );
                }

                // Test linear scaling with weight
                let half_weight_score = compute_ram_score(ram_used, ram_total, weight / 2.0);
                prop_assert!(half_weight_score.mul_add(2.0, -score) < 1e-10,
                    "Score should scale linearly with weight. Full: {}, Half: {}",
                    score, half_weight_score
                );
            }

            #[test]
            #[allow(clippy::cast_precision_loss)]
            fn test_compute_swap_ram_score(
                // Generate swap values ensuring used <= total
                swap_total in 1_000_000_000i64..1_000_000_000_000i64,
                swap_used_percentage in 0.0..1.0f64,
                weight in 0.0..1.0f64,
            ) {
                let swap_used = (swap_total as f64 * swap_used_percentage) as i64;
                let score = compute_swap_ram_score(swap_used, swap_total, weight);

                // Basic range check
                prop_assert!(score >= 0.0 && score <= weight);

                // Test perfect score when no swap is used
                let perfect_score = compute_swap_ram_score(0, swap_total, weight);
                prop_assert!((perfect_score - weight).abs() < 1e-10,
                    "Should get maximum score when no swap is used. Expected: {}, Got: {}",
                    weight, perfect_score
                );

                // Test zero score when all swap is used
                let worst_score = compute_swap_ram_score(swap_total, swap_total, weight);
                prop_assert!(worst_score.abs() < 1e-10,
                    "Should get zero score when all swap is used. Got: {}",
                    worst_score
                );

                // Test monotonicity
                if swap_used < swap_total {
                    let higher_usage_score = compute_swap_ram_score(swap_used + 1, swap_total, weight);
                    prop_assert!(higher_usage_score < score,
                        "Score should decrease with higher swap usage. Original: {}, Higher Usage: {}",
                        score, higher_usage_score
                    );
                }

                // Test linear scaling with weight
                let half_weight_score = compute_swap_ram_score(swap_used, swap_total, weight / 2.0);
                prop_assert!(half_weight_score.mul_add(2.0, -score) < 1e-10,
                    "Score should scale linearly with weight. Full: {}, Half: {}",
                    score, half_weight_score
                );
            }

            #[test]
            fn test_compute_network_score(
                // Generate network traffic values
                network_rx in 0i64..1_000_000_000i64,
                network_tx in 0i64..1_000_000_000i64,
                weight in 0.01..1.0f64,
            ) {
                let score = compute_network_score(network_rx, network_tx, weight);

                // Basic range check
                prop_assert!(score >= 0.0 && score <= weight);

                // Test perfect score with no network traffic
                let perfect_score = compute_network_score(0, 0, weight);
                prop_assert!((perfect_score - weight).abs() < 1e-10,
                    "Should get weight/2 score when no network traffic. Expected: {}, Got: {}",
                    weight, perfect_score
                );

                // Test monotonicity with respect to receive traffic
                if network_rx < 1_000_000_000 - 1000 {
                    let higher_rx_score = compute_network_score(network_rx + 1000, network_tx, weight);
                    prop_assert!(higher_rx_score < score,
                        "Score should decrease with higher receive traffic. Original: {}, Higher RX: {}",
                        score, higher_rx_score
                    );
                }

                // Test monotonicity with respect to transmit traffic
                if network_tx < 1_000_000_000 - 1000 {
                    let higher_tx_score = compute_network_score(network_rx, network_tx + 1000, weight);
                    prop_assert!(higher_tx_score < score,
                        "Score should decrease with higher transmit traffic. Original: {}, Higher TX: {}",
                        score, higher_tx_score
                    );
                }

                // Test scaling with weight
                let half_weight_score = compute_network_score(network_rx, network_tx, weight / 2.0);
                prop_assert!(half_weight_score.mul_add(2.0, -score) < 1e-10,
                    "Score should scale linearly with weight. Full: {}, Half: {}",
                    score, half_weight_score
                );
            }
        }
    }
}
