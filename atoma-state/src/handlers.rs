use atoma_p2p::{broadcast_metrics::NodeMetrics, AtomaP2pEvent};
use atoma_sui::events::{
    AtomaEvent, ClaimedStackEvent, NewKeyRotationEvent, NewStackSettlementAttestationEvent,
    NodePublicKeyCommittmentEvent, NodeRegisteredEvent, NodeSubscribedToTaskEvent,
    NodeSubscriptionUpdatedEvent, NodeUnsubscribedFromTaskEvent, StackAttestationDisputeEvent,
    StackCreatedEvent, StackSettlementTicketClaimedEvent, StackSettlementTicketEvent, StackSmallId,
    StackTrySettleEvent, TaskDeprecationEvent, TaskRegisteredEvent,
};
use atoma_utils::compression::decompress_bytes;
use chrono::{DateTime, Utc};
use remote_attestation_verification::CombinedEvidence;
use tokio::sync::oneshot;
use tracing::{error, info, instrument, trace};

use crate::{
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
        AtomaEvent::ClaimedStackEvent(event) => {
            handle_claimed_stack_event(state_manager, event).await
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
    locked_compute_units: i64,
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
    stack.locked_compute_units = locked_compute_units;
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

/// Handles a claimed stack event.
///
/// This function processes a claimed stack event by parsing the event data,
/// updating the stack as claimed and setting the user refund amount.
///
/// # Arguments
///
/// * `state_manager` - A reference to the `AtomaStateManager` for database operations.
/// * `event` - A `ClaimedStackEvent` containing the details of the claimed stack event.
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if the event was processed successfully, or an error if something went wrong.
///
/// # Errors
///
/// This function will return an error if:
/// * The event data cannot be deserialized into a `ClaimedStackEvent`.
/// * The database operation to update the stack as claimed fails.
#[instrument(level = "trace", skip_all)]
pub async fn handle_claimed_stack_event(
    state_manager: &AtomaStateManager,
    event: ClaimedStackEvent,
) -> Result<()> {
    trace!(
        target = "atoma-state-handlers",
        event = "handle-claimed-stack-event",
        "Processing claimed stack event"
    );
    let ClaimedStackEvent {
        stack_small_id: StackSmallId {
            inner: stack_small_id,
        },
        user_refund_amount,
        ..
    } = event;
    state_manager
        .state
        .update_stack_claimed(stack_small_id as i64, user_refund_amount as i64)
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
        AtomaAtomaStateManagerEvent::LockStack { stack_small_id } => {
            trace!(
                target = "atoma-state-handlers",
                event = "handle-state-manager-event",
                "Locking stack with id: {}",
                stack_small_id
            );
            state_manager.state.lock_stack(stack_small_id).await?;
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
            is_confidential,
            result_sender,
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
        AtomaAtomaStateManagerEvent::GetStacksForTask {
            task_small_id,
            free_compute_units,
            user_id,
            result_sender,
        } => {
            trace!(
                target = "atoma-state-handlers",
                event = "handle-state-manager-event",
                "Getting stacks for task with id: {}",
                task_small_id
            );
            let stack = state_manager
                .state
                .get_stacks_for_task(task_small_id, free_compute_units, user_id)
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
            user_id,
            result_sender,
        } => {
            let node = state_manager
                .state
                .select_node_public_key_for_encryption(&model, max_num_tokens, user_id)
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
            locked_compute_units,
            transaction_timestamp,
            user_id,
            result_sender,
        } => {
            let result = handle_stack_created_event(
                state_manager,
                event,
                locked_compute_units,
                user_id,
                transaction_timestamp,
            )
            .await;
            result_sender
                .send(result)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::UpdateNodeThroughputPerformance {
            timestamp,
            model_name,
            input_tokens,
            output_tokens,
            time,
        } => {
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
        AtomaAtomaStateManagerEvent::GetUserIdByEmailPassword {
            email,
            password,
            result_sender,
        } => {
            let user_id = state_manager
                .state
                .get_user_id_by_email_password(&email, &password)
                .await;
            result_sender
                .send(user_id)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::OAuth {
            email,
            password_salt,
            result_sender,
        } => {
            let user_id = state_manager.state.oauth(&email, &password_salt).await;
            result_sender
                .send(user_id)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::RegisterUserWithPassword {
            user_profile,
            password,
            password_salt,
            result_sender,
        } => {
            let user_id = state_manager
                .state
                .register(user_profile, &password, &password_salt)
                .await;
            result_sender
                .send(user_id)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::GetPasswordSalt {
            email,
            result_sender,
        } => {
            let salt = state_manager.state.get_password_salt(&email).await;
            result_sender
                .send(salt)
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
        AtomaAtomaStateManagerEvent::StoreNewApiToken {
            user_id,
            api_token,
            name,
        } => {
            state_manager
                .state
                .store_api_token(user_id, &api_token, &name)
                .await?;
        }
        AtomaAtomaStateManagerEvent::RevokeApiToken {
            user_id,
            api_token_id,
        } => {
            state_manager
                .state
                .delete_api_token(user_id, api_token_id)
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
        AtomaAtomaStateManagerEvent::RefundUsdc {
            user_id,
            amount,
            result_sender,
        } => {
            let success = state_manager.state.refund_usdc(user_id, amount).await;
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
        AtomaAtomaStateManagerEvent::GetZkSalt {
            user_id,
            result_sender,
        } => {
            let salt = state_manager.state.get_zk_salt(user_id).await;
            result_sender
                .send(salt)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::SetSalt {
            user_id,
            salt,
            result_sender,
        } => {
            let success = state_manager.state.set_zk_salt(user_id, &salt).await;
            result_sender
                .send(success)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::LockUserFiatBalance {
            user_id,
            amount,
            result_sender,
        } => {
            let result = state_manager
                .state
                .lock_user_fiat_balance(user_id, amount)
                .await;
            result_sender
                .send(result)
                .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
        }
        AtomaAtomaStateManagerEvent::UpdateStackNumTokensFiat {
            user_id,
            estimated_amount,
            amount,
        } => {
            state_manager
                .state
                .update_real_amount_fiat_balance(user_id, estimated_amount, amount)
                .await?;
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
        nonce,
    } = event;
    state_manager
        .state
        .insert_new_key_rotation(epoch as i64, key_rotation_counter as i64, nonce as i64)
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
        device_type,
        evidence_bytes,
    } = event;
    let original_evidence_bytes = decompress_bytes(&evidence_bytes)?;
    let evidence_data = serde_json::from_slice::<Vec<CombinedEvidence>>(&original_evidence_bytes)?;
    let mut gpu_evidence_data = Vec::new();
    let mut nvswitch_evidence_data = Vec::new();
    for evidence in evidence_data {
        match evidence {
            CombinedEvidence::Device(evidence) => gpu_evidence_data.push(evidence),
            CombinedEvidence::NvSwitch(evidence) => nvswitch_evidence_data.push(evidence),
        }
    }
    let is_ppcie_mode = gpu_evidence_data.len() == 8 && nvswitch_evidence_data.len() == 4;
    let is_valid = if is_ppcie_mode {
        let mut is_valid = remote_attestation_verification::attest_nvidia_gpu_evidence_list(
            state_manager,
            &gpu_evidence_data,
            &new_public_key,
            device_type,
        )
        .await
        .is_ok();
        is_valid &= remote_attestation_verification::attest_nvidia_nvswitch_evidence_list(
            state_manager,
            &nvswitch_evidence_data,
            &new_public_key,
            device_type,
        )
        .await
        .is_ok();
        is_valid &= remote_attestation_verification::verify_topology(
            &gpu_evidence_data,
            &nvswitch_evidence_data,
        )
        .is_ok();
        is_valid
    } else {
        remote_attestation_verification::attest_nvidia_gpu_evidence_list(
            state_manager,
            &gpu_evidence_data,
            &new_public_key,
            device_type,
        )
        .await
        .is_ok()
    };
    state_manager
        .state
        .update_node_public_key(
            node_id.inner as i64,
            epoch as i64,
            key_rotation_counter as i64,
            new_public_key,
            evidence_bytes,
            i64::from(device_type),
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
    state_manager
        .metrics_collector_sender
        .send((node_small_id, node_metrics))
        .map_err(|_| AtomaStateManagerError::ChannelSendError)?;
    state_manager
        .state
        .update_node_public_address(node_small_id, public_url, country)
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

pub mod remote_attestation_verification {
    use crate::{errors::AtomaStateRemoteAttestationError, AtomaStateManager};

    use base64::{engine::general_purpose::STANDARD, Engine};
    use remote_attestation_verifier::{
        remote_gpu_attestation::AttestRemoteOptions, verify_gpu_attestation,
        verify_nvswitch_attestation, AttestError, DeviceEvidence, NvSwitchEvidence,
    };
    use serde::{Deserialize, Serialize};
    use tracing::instrument;

    type Result<T> = std::result::Result<T, AtomaStateRemoteAttestationError>;

    /// Combined evidence from a device and an NVSwitch
    ///
    /// This enum represents the evidence from a device and an NVSwitch, which is used to verify the integrity and authenticity of the GPU hardware and its execution environment.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "evidence_type")]
    pub enum CombinedEvidence {
        /// Evidence from a device
        #[serde(rename = "device")]
        Device(DeviceEvidence),

        /// Evidence from an NVSwitch
        #[serde(rename = "nvswitch")]
        NvSwitch(NvSwitchEvidence),
    }

    /// Attests the NVIDIA GPU evidence list
    ///
    /// This function attests the NVIDIA GPU evidence list by verifying the evidence data and the topology.
    ///
    /// # Arguments
    ///
    /// * `state_manager` - A reference to the `AtomaStateManager` that provides access to the state database
    /// * `evidence_data` - A reference to the GPU evidence data
    /// * `new_public_key` - The new public key
    /// * `device_type` - The device type
    ///
    /// # Returns
    ///
    /// * `Result<()>` - Ok(()) if the attestation is successful, or an error if the operation failed
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// * The GPU evidence data cannot be decoded
    /// * The topology check fails
    #[instrument(
        level = "info", 
        skip_all,
        fields(
            device_type = device_type,
            new_public_key = hex::encode(new_public_key),
        ),
        err
    )]
    pub async fn attest_nvidia_gpu_evidence_list(
        state_manager: &AtomaStateManager,
        evidence_data: &[DeviceEvidence],
        new_public_key: &[u8],
        device_type: u16,
    ) -> Result<()> {
        let contract_nonce = state_manager
            .state
            .get_contract_key_rotation_nonce()
            .await
            .map_err(|_| AtomaStateRemoteAttestationError::FailedToRetrieveContractNonce)?;
        let should_be_nonce = blake3::hash(
            &[
                &contract_nonce.to_le_bytes()[..],
                new_public_key,
                &device_type.to_le_bytes()[..],
            ]
            .concat(),
        );
        let should_be_nonce_hex = hex::encode(should_be_nonce.as_bytes());
        tracing::info!(
            target = "atoma-state-handlers",
            event = "attest-nvidia-evidence-list",
            contract_nonce = contract_nonce,
            new_public_key = hex::encode(new_public_key),
            device_type = device_type,
            "Attesting NVIDIA evidence list, with should_be_nonce: {should_be_nonce_hex}"
        );
        let result = match verify_gpu_attestation(
            evidence_data,
            &should_be_nonce_hex,
            AttestRemoteOptions::default(),
        )
        .await
        {
            Ok(result) => result,
            Err(e) => {
                tracing::error!(
                    target = "atoma-state-handlers",
                    event = "attest-nvidia-evidence-list",
                    "Attestation failed for device type: {device_type} and public key: {}, with error: {e}",
                    hex::encode(new_public_key),
                );
                return Err(AtomaStateRemoteAttestationError::FailedToAttestRemote(
                    AttestError::RemoteAttestationFailed,
                ));
            }
        };
        if result.0 {
            tracing::info!(
                target = "atoma-state-handlers",
                event = "attest-nvidia-evidence-list",
                "Attestation successful for device type: {device_type} and public key: {}",
                hex::encode(new_public_key),
            );
            Ok(())
        } else {
            tracing::error!(
                target = "atoma-state-handlers",
                event = "attest-nvidia-evidence-list",
                "Attestation failed for device type: {device_type} and public key: {}",
                hex::encode(new_public_key),
            );
            Err(AtomaStateRemoteAttestationError::FailedToAttestRemote(
                AttestError::RemoteAttestationFailed,
            ))
        }
    }

    /// Attests the NVSwitch evidence list
    ///
    /// This function attests the NVSwitch evidence list by verifying the evidence data and the topology.
    ///
    /// # Arguments
    ///
    /// * `state_manager` - A reference to the `AtomaStateManager` that provides access to the state database
    /// * `nvswitch_evidence_data` - A reference to the NVSwitch evidence data
    /// * `new_public_key` - The new public key
    /// * `device_type` - The device type
    ///
    /// # Returns
    ///
    /// * `Result<()>` - Ok(()) if the attestation is successful, or an error if the operation failed
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// * The GPU evidence data cannot be decoded
    /// * The NVSwitch evidence data cannot be decoded
    #[instrument(
        level = "info", 
        skip_all,
        fields(
            device_type = device_type,
            new_public_key = hex::encode(new_public_key),
        ),
        err,
    )]
    pub async fn attest_nvidia_nvswitch_evidence_list(
        state_manager: &AtomaStateManager,
        nvswitch_evidence_data: &[NvSwitchEvidence],
        new_public_key: &[u8],
        device_type: u16,
    ) -> Result<()> {
        let contract_nonce = state_manager
            .state
            .get_contract_key_rotation_nonce()
            .await
            .map_err(|_| AtomaStateRemoteAttestationError::FailedToRetrieveContractNonce)?;
        let should_be_nonce = blake3::hash(
            &[
                &contract_nonce.to_le_bytes()[..],
                new_public_key,
                &device_type.to_le_bytes()[..],
            ]
            .concat(),
        );
        let should_be_nonce_hex = hex::encode(should_be_nonce.as_bytes());
        let (result, _) = verify_nvswitch_attestation(
            nvswitch_evidence_data,
            &should_be_nonce_hex,
            AttestRemoteOptions::default(),
        ).await.map_err(|_| {
            tracing::error!(
                target = "atoma-state-handlers",
                event = "attest-nvidia-evidence-list",
                "NVSwitch attestation verification failed for device type: {device_type} and public key: {}",
                hex::encode(new_public_key),
            );
            AtomaStateRemoteAttestationError::FailedToAttestRemote(
                AttestError::RemoteAttestationFailed,
            )
        })?;
        if result {
            tracing::info!(
                target = "atoma-state-handlers",
                event = "attest-nvidia-evidence-list",
                "Attestation successful for device type: {device_type} and public key: {}",
                hex::encode(new_public_key),
            );
        } else {
            tracing::error!(
                target = "atoma-state-handlers",
                event = "attest-nvidia-evidence-list",
                "Attestation failed for device type: {device_type} and public key: {}",
                hex::encode(new_public_key),
            );
        }

        Ok(())
    }

    /// Verifies the topology of the GPU and NVSwitch evidence data
    ///
    /// This function verifies the topology of the GPU and NVSwitch evidence data by decoding the evidence data and checking the topology.
    ///
    /// # Arguments
    ///
    /// * `gpu_evidence_data` - A reference to the GPU evidence data
    /// * `nvswitch_evidence_data` - A reference to the NVSwitch evidence data
    ///
    /// # Returns
    ///
    /// * `Result<()>` - Ok(()) if the topology is valid, or an error if the operation failed
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// * The GPU evidence data cannot be decoded
    /// * The NVSwitch evidence data cannot be decoded
    /// * The topology check fails
    #[instrument(level = "info", skip_all, fields(topology_check = "true",), err)]
    pub fn verify_topology(
        gpu_evidence_data: &[DeviceEvidence],
        nvswitch_evidence_data: &[NvSwitchEvidence],
    ) -> Result<()> {
        let gpu_evidence_data_refs = gpu_evidence_data
            .iter()
            .map(|evidence| STANDARD.decode(&evidence.evidence))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| {
                tracing::error!(
                    target = "atoma-state-handlers",
                    event = "verify-topology",
                    "Failed to decode GPU evidence data: {e}",
                );
                AtomaStateRemoteAttestationError::FailedToAttestRemote(
                    AttestError::RemoteAttestationFailed,
                )
            })?;
        let nvswitch_evidence_data_refs = nvswitch_evidence_data
            .iter()
            .map(|evidence| STANDARD.decode(&evidence.evidence))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| {
                tracing::error!(
                    target = "atoma-state-handlers",
                    event = "verify-topology",
                    "Failed to decode NVSwitch evidence data from base64 data: {e}",
                );
                AtomaStateRemoteAttestationError::FailedToAttestRemote(
                    AttestError::RemoteAttestationFailed,
                )
            })?;
        let unique_switch_pdis_set = topology::topology::gpu_topology_check(
            &gpu_evidence_data_refs
                .iter()
                .map(Vec::as_slice)
                .collect::<Vec<_>>(),
        )
        .map_err(|e| {
            tracing::error!(
                target = "atoma-state-handlers",
                event = "verify-topology",
                "Failed to check GPU topology: {e}",
            );
            AtomaStateRemoteAttestationError::FailedToAttestRemote(
                AttestError::RemoteAttestationFailed,
            )
        })?;
        topology::topology::switch_topology_check(
            &nvswitch_evidence_data_refs
                .iter()
                .map(Vec::as_slice)
                .collect::<Vec<_>>(),
            gpu_evidence_data.len(),
            unique_switch_pdis_set,
        )
        .map_err(|e| {
            tracing::error!(
                target = "atoma-state-handlers",
                event = "verify-topology",
                "Failed to check NVSwitch topology: {e}",
            );
            AtomaStateRemoteAttestationError::FailedToAttestRemote(
                AttestError::RemoteAttestationFailed,
            )
        })?;
        Ok(())
    }
}
