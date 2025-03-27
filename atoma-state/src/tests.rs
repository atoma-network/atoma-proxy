use crate::state_manager::Result;

use super::*;
use atoma_p2p::broadcast_metrics::{
    ChatCompletionsMetrics, EmbeddingsMetrics, ImageGenerationMetrics, ModelMetrics, NodeMetrics,
};
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

pub const POSTGRES_TEST_DB_URL: &str = "postgres://atoma:atoma@localhost:5432/atoma";

/// Creates a new AtomaState instance for testing.
///
/// # Returns
///
/// - `AtomaState`: A new AtomaState instance.
///
/// # Panics
///
/// This function will panic if the database connection pool is not valid.
pub async fn setup_test_db() -> AtomaState {
    AtomaState::new_from_url(POSTGRES_TEST_DB_URL)
        .await
        .unwrap()
}

/// Truncates all tables in the database.
///
/// # Arguments
///
/// * `db` - The database connection pool.
///
/// # Panics
///
/// This function will panic if the database connection pool is not valid.
pub async fn truncate_tables(db: &sqlx::PgPool) {
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
                users,
                key_rotations",
    )
    .execute(db)
    .await
    .expect("Failed to truncate tables");
}

#[tokio::test]
#[serial_test::serial]
async fn test_verify_node_small_id_ownership() {
    let state = setup_test_db().await;
    truncate_tables(&state.db).await;

    // Insert test node data
    sqlx::query("INSERT INTO nodes (node_small_id, node_id, sui_address, public_address) VALUES ($1, $2, $3, $4)")
            .bind(1i64)
            .bind("0xa12...beef")
            .bind("0x123...456")
            .bind("test_address_1")
            .execute(&state.db)
            .await
            .expect("Failed to insert test node");

    // Test cases
    let test_cases = vec![
        // Valid case
        (1i64, "test_address_1".to_string(), true),
        // Wrong address
        (1i64, "wrong_address".to_string(), false),
        // Non-existent node
        (999i64, "test_address_1".to_string(), false),
    ];

    for (node_id, address, should_succeed) in test_cases {
        let result = state.verify_node_small_id_ownership(node_id, address).await;

        match (should_succeed, result) {
            (true, Ok(()))
            | (false, Err(AtomaStateManagerError::NodeSmallIdOwnershipVerificationFailed)) => {
                // 1. Test passed - verification succeeded as expected
                // 2. Test passed - verification failed as expected
            }
            (true, Err(e)) => {
                panic!("Expected verification to succeed, but got error: {e}");
            }
            (false, Ok(())) => {
                panic!("Expected verification to fail, but it succeeded");
            }
            (_, Err(e)) => {
                panic!("Unexpected error: {e}");
            }
        }
    }
}

#[tokio::test]
#[serial_test::serial]
async fn test_verify_node_small_id_ownership_with_multiple_nodes() {
    let state = setup_test_db().await;
    truncate_tables(&state.db).await;

    // Insert multiple test nodes
    let test_nodes = vec![
        (1i64, "test_address_1"),
        (2i64, "test_address_2"),
        (3i64, "test_address_3"),
    ];

    for (node_id, address) in &test_nodes {
        sqlx::query("INSERT INTO nodes (node_small_id, public_address, sui_address, node_id) VALUES ($1, $2, $3, $4)")
                .bind(*node_id)
                .bind(*address)
                .bind("0x123...456")
                .bind("0xa12...beef")
                .execute(&state.db)
                .await
                .expect("Failed to insert test node");
    }

    // Verify correct mappings
    for (node_id, address) in &test_nodes {
        let result = state
            .verify_node_small_id_ownership(*node_id, (*address).to_string())
            .await;
        assert!(
            result.is_ok(),
            "Verification should succeed for valid node-address pair"
        );
    }

    // Verify incorrect mappings
    for (node_id, address) in &test_nodes {
        let wrong_address = format!("wrong_{address}");
        let result = state
            .verify_node_small_id_ownership(*node_id, wrong_address)
            .await;
        assert!(
            matches!(
                result,
                Err(AtomaStateManagerError::NodeSmallIdOwnershipVerificationFailed)
            ),
            "Verification should fail for invalid address"
        );
    }
}

#[tokio::test]
#[serial_test::serial]
async fn test_verify_node_small_id_ownership_with_empty_db() {
    let state = setup_test_db().await;
    truncate_tables(&state.db).await;

    let result = state
        .verify_node_small_id_ownership(1, "any_address".to_string())
        .await;

    assert!(
        matches!(
            result,
            Err(AtomaStateManagerError::NodeSmallIdOwnershipVerificationFailed)
        ),
        "Verification should fail when database is empty"
    );
}

async fn insert_test_node(db: &sqlx::PgPool, node_small_id: i64) {
    sqlx::query("INSERT INTO nodes (node_small_id, node_id, sui_address, public_address) VALUES ($1, $2, $3, $4)")
            .bind(node_small_id)
            .bind("test_node_id")
            .bind("test_sui_address")
            .bind("test_public_address")
            .execute(db)
            .await
            .expect("Failed to insert test node");
}

async fn get_node_public_url(db: &sqlx::PgPool, node_small_id: i64) -> Option<(String, i64)> {
    sqlx::query_as::<_, (String, i64)>(
        "SELECT public_address, timestamp FROM nodes WHERE node_small_id = $1",
    )
    .bind(node_small_id)
    .fetch_optional(db)
    .await
    .expect("Failed to fetch node public URL")
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

/// Helper function to create a test key rotation
async fn create_key_rotation(
    pool: &sqlx::PgPool,
    epoch: i64,
    key_rotation_counter: i64,
    nonce: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO key_rotations (epoch, key_rotation_counter, nonce) VALUES ($1, $2, $3)",
    )
    .bind(epoch)
    .bind(key_rotation_counter)
    .bind(nonce)
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
            "INSERT INTO node_subscriptions (node_small_id, task_small_id, price_per_one_million_compute_units, max_num_compute_units, valid)
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
            "INSERT INTO nodes (node_small_id, sui_address, public_address, country, node_id) VALUES ($1, $2, $3, $4, $5)"
        )
        .bind(node_small_id)
        .bind(Uuid::new_v4().to_string())
        .bind("test_public_address")
        .bind("test_country")
        .bind("asdfghjkl")
        .execute(pool)
        .await?;
    Ok(())
}

/// Helper function to create a test node public key
async fn create_test_node_public_key(
    pool: &sqlx::PgPool,
    node_small_id: i64,
    key_rotation_counter: i64,
    is_valid: bool,
) -> sqlx::Result<()> {
    sqlx::query(
            "INSERT INTO node_public_keys (node_small_id, public_key, is_valid, epoch, key_rotation_counter, evidence_bytes, device_type)
             VALUES ($1, $2, $3, $4, $5, $6, $7)"
        )
        .bind(node_small_id)
        .bind(vec![0u8; 32]) // dummy public key
        .bind(is_valid)
        .bind(1i64)
        .bind(key_rotation_counter)
        .bind(vec![0u8; 32]) // dummy attestation bytes
        .bind(1i64)
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
    sqlx::query(
        "INSERT INTO users (id, email, password_salt, password_hash) VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(format!("user_{user_id}")) // Create unique email
    .bind("test_password_salt") // Default password salt
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
                price_per_one_million_compute_units,
                already_computed_units,
                in_settle_period,
                total_hash,
                num_total_messages,
                user_id,
                acquired_timestamp
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
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
    .bind(chrono::Utc::now()) // Acquired timestamp
    .execute(pool)
    .await?;
    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_register_node_public_url_success() {
    let state = setup_test_db().await;
    truncate_tables(&state.db).await;

    // Insert test node
    insert_test_node(&state.db, 1).await;

    let public_url = "https://test.example.com".to_string();
    let timestamp = chrono::Utc::now().timestamp();

    // Test registration
    let result = state
        .register_node_public_url(1, public_url.clone(), timestamp, "US".to_string())
        .await;
    assert!(result.is_ok(), "Registration should succeed");

    // Verify the registration
    let stored_data = get_node_public_url(&state.db, 1)
        .await
        .expect("Node should exist");
    assert_eq!(stored_data.0, public_url);
    assert_eq!(stored_data.1, timestamp);
}

#[tokio::test]
#[serial_test::serial]
async fn test_register_node_public_url_update_existing() {
    let state = setup_test_db().await;
    truncate_tables(&state.db).await;

    // Insert test node with initial URL
    insert_test_node(&state.db, 1).await;
    let initial_url = "https://initial.example.com".to_string();
    let initial_timestamp = chrono::Utc::now().timestamp();
    state
        .register_node_public_url(1, initial_url, initial_timestamp, "US".to_string())
        .await
        .unwrap();

    // Update with new URL
    let new_url = "https://updated.example.com".to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));
    let new_timestamp = chrono::Utc::now().timestamp();
    let result = state
        .register_node_public_url(1, new_url.clone(), new_timestamp, "US".to_string())
        .await;
    result.unwrap();
    // assert!(result.is_ok(), "URL update should succeed");

    // Verify the update
    let stored_data = get_node_public_url(&state.db, 1)
        .await
        .expect("Node should exist");
    assert_eq!(stored_data.0, new_url);
    assert_eq!(stored_data.1, new_timestamp);
}

#[tokio::test]
#[serial_test::serial]
async fn test_get_cheapest_node_basic() {
    let state = setup_test_db().await;
    truncate_tables(&state.db).await;

    // Create test data
    create_test_task(&state.db, 1, "gpt-4", 0).await.unwrap();
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
    assert_eq!(node.price_per_one_million_compute_units, 100);
    assert_eq!(node.max_num_compute_units, 1000);
    assert_eq!(node.node_small_id, 1);
}

#[tokio::test]
#[serial_test::serial]
async fn test_register_node_public_url_nonexistent_node() {
    let state = setup_test_db().await;
    truncate_tables(&state.db).await;

    let start_time = std::time::Instant::now();
    let result = state
        .register_node_public_url(
            999, // Non-existent node
            "https://test.example.com".to_string(),
            chrono::Utc::now().timestamp(),
            "US".to_string(),
        )
        .await;

    // Verify error and retry behavior
    assert!(matches!(result, Err(AtomaStateManagerError::NodeNotFound)));

    // Verify that it took at least the expected retry time
    // 3 retries * 100ms = 300ms minimum
    let elapsed = start_time.elapsed();
    assert!(
        elapsed >= std::time::Duration::from_millis(300),
        "Should have waited for at least 300ms, but only waited for {}ms",
        elapsed.as_millis()
    );
}

#[tokio::test]
#[serial_test::serial]
async fn test_register_node_public_url_delayed_node_creation() {
    let state = setup_test_db().await;
    truncate_tables(&state.db).await;

    // Spawn a task to insert the node after a delay
    let db_clone = state.db.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        insert_test_node(&db_clone, 2).await;
    });

    // Attempt to register URL (should succeed after retry)
    let public_url = "https://delayed.example.com".to_string();
    let timestamp = chrono::Utc::now().timestamp();
    let result = state
        .register_node_public_url(2, public_url.clone(), timestamp, "US".to_string())
        .await;

    assert!(
        result.is_ok(),
        "Registration should succeed after node creation"
    );

    // Verify the registration
    let stored_data = get_node_public_url(&state.db, 2)
        .await
        .expect("Node should exist");
    assert_eq!(stored_data.0, public_url);
    assert_eq!(stored_data.1, timestamp);
}

#[tokio::test]
#[serial_test::serial]
async fn test_get_cheapest_node_multiple_prices() {
    let state = setup_test_db().await;
    truncate_tables(&state.db).await;

    // Create test data with multiple nodes at different prices
    create_test_task(&state.db, 1, "gpt-4", 0).await.unwrap();
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
    assert_eq!(node.price_per_one_million_compute_units, 50);
    assert_eq!(node.node_small_id, 2);
}

#[tokio::test]
#[serial_test::serial]
async fn test_register_node_public_url_concurrent_updates() {
    let state = setup_test_db().await;
    truncate_tables(&state.db).await;

    // Spawn multiple concurrent update attempts
    let mut handles = vec![];
    for i in 0..5 {
        // Insert test node
        insert_test_node(&state.db, i).await;
        let state_clone = state.clone();
        let url = format!("https://concurrent{i}.example.com");
        let timestamp = chrono::Utc::now().timestamp();

        handles.push(tokio::spawn(async move {
            state_clone
                .register_node_public_url(i, url, timestamp, "US".to_string())
                .await
        }));
    }

    // Wait for all updates to complete
    let results: Vec<Result<()>> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    // Verify all updates succeeded
    assert!(
        results.iter().all(std::result::Result::is_ok),
        "All updates should succeed"
    );

    // Verify final state (should be the last update)
    let stored_data = get_node_public_url(&state.db, 3)
        .await
        .expect("Node should exist");
    assert!(stored_data.0.starts_with("https://concurrent"));
}

#[tokio::test]
#[serial_test::serial]
async fn test_get_cheapest_node_confidential() {
    let state = setup_test_db().await;
    truncate_tables(&state.db).await;

    // Create test data for confidential computing
    create_test_task(&state.db, 1, "gpt-4", 1).await.unwrap();
    create_key_rotation(&state.db, 1, 1, 1).await.unwrap();
    create_test_node(&state.db, 1).await.unwrap();
    create_test_node_subscription(&state.db, 1, 1, 100, 1000)
        .await
        .unwrap();
    create_key_rotation(&state.db, 2, 2, 1)
        .await
        .expect("Failed to create key rotation");
    create_test_node_public_key(&state.db, 1, 2, true)
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
    create_test_task(&state.db, 1, "gpt-4", 1).await.unwrap();
    create_test_node(&state.db, 1).await.unwrap();
    create_test_node_subscription(&state.db, 1, 1, 100, 1000)
        .await
        .unwrap();
    create_key_rotation(&state.db, 1, 1, 1).await.unwrap();
    create_test_node_public_key(&state.db, 1, 1, false)
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
    create_test_task(&state.db, 1, "gpt-4", 0).await.unwrap();
    create_test_node(&state.db, 1).await.unwrap();

    // Create invalid subscription (valid = false)
    sqlx::query(
            "INSERT INTO node_subscriptions (node_small_id, task_small_id, price_per_one_million_compute_units, max_num_compute_units, valid)
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
    create_test_task(&state.db, 1, "gpt-4", 0).await.unwrap();
    create_test_task(&state.db, 2, "gpt-4", 1).await.unwrap();
    create_key_rotation(&state.db, 1, 1, 1).await.unwrap();

    create_test_node(&state.db, 1).await.unwrap();
    create_test_node(&state.db, 2).await.unwrap();

    create_test_node_subscription(&state.db, 1, 1, 100, 1000)
        .await
        .unwrap();
    create_test_node_subscription(&state.db, 2, 2, 50, 1000)
        .await
        .unwrap();
    create_key_rotation(&state.db, 2, 1, 1).await.unwrap();
    create_test_node_public_key(&state.db, 2, 1, true)
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
    assert_eq!(node.price_per_one_million_compute_units, 50);

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
    create_test_task(&state.db, 1, "gpt-4", 1).await?;

    Ok(state)
}

#[tokio::test]
#[serial_test::serial]
async fn test_basic_selection() -> Result<()> {
    let state = setup_test_environment().await?;

    // Setup single valid node
    create_test_node(&state.db, 1).await?;
    create_test_node_subscription(&state.db, 1, 1, 100, 1000).await?;
    create_key_rotation(&state.db, 1, 1, 1).await?;
    create_test_node_public_key(&state.db, 1, 1, true).await?;
    create_test_stack(&state.db, 1, 1, 1, 100, 1000, 1)
        .await
        .unwrap();
    let result = state
        .select_node_public_key_for_encryption("gpt-4", 800, 1)
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
    for (node_id, (i, price)) in [(1, 200), (2, 100), (3, 300)].iter().enumerate() {
        create_test_node(&state.db, node_id as i64).await?;
        create_test_node_subscription(&state.db, node_id as i64, 1, *price, 1000).await?;
        create_key_rotation(&state.db, i64::from(*i), i64::from(*i), 1).await?;
        create_test_node_public_key(&state.db, node_id as i64, i64::from(*i), true).await?;
        create_test_stack(
            &state.db,
            1,
            node_id as i64,
            node_id as i64,
            *price,
            1000,
            node_id as i64,
        )
        .await
        .unwrap();
    }

    let result = state
        .select_node_public_key_for_encryption("gpt-4", 800, node_id)
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
async fn test_register_node_public_url_invalid_inputs() {
    let state = setup_test_db().await;
    truncate_tables(&state.db).await;

    // Insert test node
    insert_test_node(&state.db, 4).await;

    // Test with empty URL
    let result = state
        .register_node_public_url(
            4,
            "https://test.com".to_string(),
            chrono::Utc::now().timestamp(),
            "US".to_string(),
        )
        .await;
    assert!(result.is_ok(), "Empty URL should be allowed");

    std::thread::sleep(std::time::Duration::from_secs(1));
    // Test with negative timestamp
    let result = state
        .register_node_public_url(4, "https://test.com".to_string(), -4, "US".to_string())
        .await;
    assert!(result.is_err(), "Negative timestamp should not be allowed");
}

#[tokio::test]
#[serial_test::serial]
async fn test_compute_capacity_requirements() -> Result<()> {
    let state = setup_test_environment().await?;

    // Setup nodes with different compute capacities
    create_test_node(&state.db, 1).await?;
    create_test_node_subscription(&state.db, 1, 1, 100, 500).await?; // Insufficient capacity
    create_key_rotation(&state.db, 1, 1, 1).await?;
    create_test_node_public_key(&state.db, 1, 1, true).await?;
    create_test_stack(&state.db, 1, 1, 1, 100, 500, 1)
        .await
        .unwrap();

    let result = state
        .select_node_public_key_for_encryption("gpt-4", 800, 1)
        .await?;
    assert!(
        result.is_none(),
        "Should not select node with insufficient capacity"
    );

    // Add node with sufficient capacity
    create_test_node(&state.db, 2).await?;
    create_test_node_subscription(&state.db, 2, 1, 100, 1000).await?;
    create_key_rotation(&state.db, 2, 2, 1).await?;
    create_test_node_public_key(&state.db, 2, 2, true).await?;
    create_test_stack(&state.db, 1, 2, 2, 100, 1000, 2)
        .await
        .unwrap();

    let result = state
        .select_node_public_key_for_encryption("gpt-4", 800, 1)
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
    create_key_rotation(&state.db, 1, 1, 1).await?;
    create_test_node_public_key(&state.db, 1, 1, false).await?;
    create_test_stack(&state.db, 1, 1, 1, 100, 1000, 1)
        .await
        .unwrap();

    let result = state
        .select_node_public_key_for_encryption("gpt-4", 800, 1)
        .await?;
    assert!(
        result.is_none(),
        "Should not select node with invalid public key"
    );

    // Test non-existent model
    let result = state
        .select_node_public_key_for_encryption("nonexistent-model", 800, 1)
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

    // Create task with security level 0 (non-confidential)
    create_test_task(&state.db, 2, "gpt-4", 0).await?;

    // Setup valid node subscribed to non-confidential task
    create_test_node(&state.db, 1).await?;
    create_test_node_subscription(&state.db, 1, 2, 100, 1000).await?;
    create_key_rotation(&state.db, 1, 1, 1).await?;
    create_test_node_public_key(&state.db, 1, 1, true).await?;
    create_test_stack(&state.db, 2, 1, 1, 100, 1000, 1)
        .await
        .unwrap();
    let result = state
        .select_node_public_key_for_encryption("gpt-4", 800, 1)
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
    create_key_rotation(&state.db, 1, 1, 1).await?;
    create_test_node_public_key(&state.db, 1, 1, true).await?;
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
            .select_node_public_key_for_encryption("gpt-4", tokens, 1)
            .await?;
        assert_eq!(
            result.is_some(),
            should_succeed,
            "Failed edge case: {case} with {tokens} tokens"
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
    create_key_rotation(&state.db, 1, 1, 1).await?;
    create_test_node_public_key(&state.db, 1, 1, true).await?;
    create_test_stack(&state.db, 1, 1, 1, 100, 1000, 1)
        .await
        .unwrap();
    let futures: Vec<_> = (0..5)
        .map(|_| state.select_node_public_key_for_encryption("gpt-4", 800, 1))
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
    create_key_rotation(&state.db, 1, 1, 1).await.unwrap();
    create_test_node(&state.db, 1).await?;

    // Insert test node public key
    sqlx::query(
            r"INSERT INTO node_public_keys (node_small_id, epoch, key_rotation_counter, public_key, evidence_bytes, device_type, is_valid)
               VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(1i64)
        .bind(1i64)
        .bind(1i64)
        .bind(vec![1u8, 2, 3, 4]) // Example public key bytes
        .bind(vec![1u8, 2, 3, 4]) // Example tee remote attestation bytes
        .bind(1i64)
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

    create_key_rotation(&state.db, 1, 1, 1).await.unwrap();
    // Insert multiple test node public keys
    for i in 1..=3 {
        // Create test node
        create_test_node(&state.db, i).await?;
        sqlx::query(
                r"INSERT INTO node_public_keys (node_small_id, epoch, key_rotation_counter, public_key, evidence_bytes, device_type, is_valid)
                   VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(i)
            .bind(i)
            .bind(1)
            .bind(vec![i as u8; 4]) // Different key for each node
            .bind(vec![i as u8; 4]) // Example tee remote attestation bytes
            .bind(i)
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
    create_key_rotation(&state.db, 1, 1, 1).await.unwrap();

    // Test with negative node_small_id
    let result = state
        .select_node_public_key_for_encryption_for_node(-1)
        .await?;
    assert!(result.is_none());

    // Insert invalid public key (empty)
    sqlx::query(
            r"INSERT INTO node_public_keys (node_small_id, epoch, key_rotation_counter, public_key, evidence_bytes, device_type, is_valid)
               VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(1i64)
        .bind(1i64)
        .bind(1i64)
        .bind(Vec::<u8>::new()) // Empty public key
        .bind(vec![1u8, 2, 3, 4]) // Example tee remote attestation bytes
        .bind(1i64)
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
    create_key_rotation(&state.db, 1, 1, 1).await.unwrap();

    // Insert test data
    sqlx::query(
            r"INSERT INTO node_public_keys (node_small_id, epoch, key_rotation_counter, public_key, evidence_bytes, device_type, is_valid)
               VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(1i64)
        .bind(1i64)
        .bind(1i64)
        .bind(vec![1u8, 2, 3, 4]) // Example public key bytes
        .bind(vec![1u8, 2, 3, 4]) // Example tee remote attestation bytes
        .bind(1i64)
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

#[tokio::test]
#[serial_test::serial]
async fn test_verify_stack_for_confidential_compute_request() -> Result<()> {
    let state = setup_test_db().await;
    truncate_tables(&state.db).await;

    // Set up test data
    sqlx::query(
            r"
            INSERT INTO tasks (task_small_id, task_id, model_name, security_level, role, is_deprecated, valid_until_epoch, deprecated_at_epoch, minimum_reputation_score)
            VALUES
                (1, 'task1', 'gpt-4', 1, 0, false, null, null, 0),  -- Confidential task
                (2, 'task2', 'gpt-4', 0, 0, false, null, null, 0)   -- Non-confidential task
            ",
        )
        .execute(&state.db)
        .await?;

    sqlx::query(
        r"
            INSERT INTO key_rotations (epoch, key_rotation_counter, nonce)
            VALUES (1, 1, 1)
            ",
    )
    .execute(&state.db)
    .await?;

    sqlx::query(
            r"
            INSERT INTO node_public_keys (node_small_id, epoch, key_rotation_counter, public_key, evidence_bytes, device_type, is_valid)
            VALUES
                (1, 1, 1, 'key1', 'attestation1', 1, true),   -- Valid key
                (2, 1, 1, 'key2', 'attestation2', 1, false),  -- Invalid key
                (3, 1, 1, 'key3', 'attestation3', 1, true)    -- Valid key
            ",
        )
        .execute(&state.db)
        .await?;

    sqlx::query(
            r"
            INSERT INTO stacks (
                stack_small_id, owner, stack_id, task_small_id, selected_node_id, price_per_one_million_compute_units,
                num_compute_units, already_computed_units, in_settle_period, total_hash, num_total_messages, user_id, acquired_timestamp
            )
            VALUES
                (1, '0x1', 'stack1', 1, 1, 100, 1000, 0, false, 'hash1', 1, 1, now()),      -- Valid stack, confidential task
                (2, '0x2', 'stack2', 1, 2, 100, 1000, 0, false, 'hash2', 1, 1, now()),      -- Invalid node key
                (3, '0x3', 'stack3', 2, 1, 100, 1000, 0, false, 'hash3', 1, 1, now()),      -- Non-confidential task
                (4, '0x4', 'stack4', 1, 1, 100, 1000, 900, false, 'hash4', 1, 1, now()),    -- Not enough compute units
                (5, '0x5', 'stack5', 1, 1, 100, 1000, 0, true, 'hash5', 1, 1, now()),       -- In settle period
                (6, '0x6', 'stack6', 1, 3, 100, 1000, 500, false, 'hash6', 1, 1, now())     -- Valid stack with partial usage
            ",
        )
        .execute(&state.db)
        .await?;

    // Test cases
    let test_cases = vec![
        // (stack_small_id, available_compute_units, expected_result, description)
        (1, 500, true, "Valid stack with enough compute units"),
        (2, 500, false, "Stack with invalid node key"),
        (3, 500, false, "Non-confidential task stack"),
        (4, 200, false, "Stack with insufficient compute units"),
        (5, 500, false, "Stack in settle period"),
        (6, 400, true, "Valid stack with partial usage"),
        (
            6,
            600,
            false,
            "Valid stack but requesting too many compute units",
        ),
        (999, 500, false, "Non-existent stack"),
    ];

    for (stack_id, compute_units, expected, description) in test_cases {
        let result = state
            .verify_stack_for_confidential_compute_request(stack_id, compute_units)
            .await?;
        assert_eq!(
                result, expected,
                "Failed test case: {description} (stack_id: {stack_id}, compute_units: {compute_units})",
            );
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_verify_stack_for_confidential_compute_request_with_key_rotation() -> Result<()> {
    let state = setup_test_db().await;
    truncate_tables(&state.db).await;

    // Set up initial test data
    sqlx::query(
            r"
            INSERT INTO tasks (task_small_id, task_id, model_name, security_level, role, is_deprecated, valid_until_epoch, deprecated_at_epoch, minimum_reputation_score)
            VALUES (1, 'task1', 'gpt-4', 1, 0, false, null, null, 0)
            ",
        )
        .execute(&state.db)
        .await?;

    sqlx::query(
        r"
            INSERT INTO key_rotations (epoch, key_rotation_counter, nonce)
            VALUES (1, 1, 1)
            ",
    )
    .execute(&state.db)
    .await?;

    sqlx::query(
            r"
            INSERT INTO node_public_keys (node_small_id, epoch, key_rotation_counter, public_key, evidence_bytes, device_type, is_valid)
            VALUES (1, 1, 1, 'key1', 'attestation1', 1, true)
            ",
        )
        .execute(&state.db)
        .await?;

    sqlx::query(
            r"
            INSERT INTO stacks (
                stack_small_id, owner, stack_id, task_small_id, selected_node_id, price_per_one_million_compute_units,
                num_compute_units, already_computed_units, in_settle_period, total_hash, num_total_messages, user_id, acquired_timestamp
            )
            VALUES (1, '0x1', 'stack1', 1, 1, 100, 1000, 0, false, 'hash1', 1, 1, now())
            ",
        )
        .execute(&state.db)
        .await?;

    // Verify stack is initially valid
    let result = state
        .verify_stack_for_confidential_compute_request(1, 500)
        .await?;
    assert!(result, "Stack should be valid with current key rotation");

    // Simulate key rotation
    sqlx::query(
        r"
            INSERT INTO key_rotations (epoch, key_rotation_counter, nonce)
            VALUES (2, 2, 2)
            ",
    )
    .execute(&state.db)
    .await?;

    // Verify stack is now invalid due to outdated key
    let result = state
        .verify_stack_for_confidential_compute_request(1, 500)
        .await?;
    assert!(!result, "Stack should be invalid after key rotation");

    // Update node's key for new rotation
    sqlx::query(
        r"
            UPDATE node_public_keys
            SET public_key = 'key1_new',
                evidence_bytes = 'attestation1_new',
                key_rotation_counter = 2,
                epoch = 2,
                device_type = 1
            WHERE node_small_id = 1
            AND key_rotation_counter = 1
            ",
    )
    .execute(&state.db)
    .await?;

    // Verify stack is valid again with new key
    let result = state
        .verify_stack_for_confidential_compute_request(1, 500)
        .await?;
    assert!(result, "Stack should be valid with updated key");

    Ok(())
}

#[test]
#[allow(clippy::float_cmp)]
fn test_store_chat_completions_metrics() {
    // Create a new NodeMetricsCollector instance
    let collector = NodeMetricsCollector::new().unwrap();

    // Create test data
    let chat_completions = ChatCompletionsMetrics {
        gpu_kv_cache_usage_perc: 75.5,
        cpu_kv_cache_usage_perc: 45.2,
        time_to_first_token: 0.15,
        time_per_output_token: 0.05,
        num_running_requests: 3,
        num_waiting_requests: 2,
    };
    let model = "gpt-4";
    let node_small_id = 42;

    // Store the metrics
    collector.store_chat_completions_metrics(&chat_completions, model, node_small_id);

    // Verify each metric was stored correctly
    let labels = [model, &node_small_id.to_string()];

    assert_eq!(
        collector
            .get_chat_completions_gpu_kv_cache_usage()
            .with_label_values(&labels)
            .get(),
        75.5_f64
    );
    assert_eq!(
        collector
            .get_chat_completions_cpu_kv_cache_usage()
            .with_label_values(&labels)
            .get(),
        45.2_f64
    );
    assert_eq!(
        collector
            .get_chat_completions_ttft()
            .with_label_values(&labels)
            .get(),
        0.15_f64
    );
    assert_eq!(
        collector
            .get_chat_completions_tpot()
            .with_label_values(&labels)
            .get(),
        0.05_f64
    );
    assert_eq!(
        collector
            .get_chat_completions_num_running_requests()
            .with_label_values(&labels)
            .get(),
        3.0_f64
    );
    assert_eq!(
        collector
            .get_chat_completions_num_waiting_requests()
            .with_label_values(&labels)
            .get(),
        2.0_f64
    );
}

#[test]
#[allow(clippy::float_cmp)]
#[allow(clippy::too_many_lines)]
fn test_store_chat_completions_metrics_multiple_models() {
    let collector = NodeMetricsCollector::new().unwrap();

    // Test data for first model
    let chat_completions_1 = ChatCompletionsMetrics {
        gpu_kv_cache_usage_perc: 75.5,
        cpu_kv_cache_usage_perc: 45.2,
        time_to_first_token: 0.15,
        time_per_output_token: 0.05,
        num_running_requests: 3,
        num_waiting_requests: 2,
    };
    let model_1 = "gpt-4";
    let node_small_id_1 = 42;

    // Test data for second model
    let chat_completions_2 = ChatCompletionsMetrics {
        gpu_kv_cache_usage_perc: 60.0,
        cpu_kv_cache_usage_perc: 30.0,
        time_to_first_token: 0.1,
        time_per_output_token: 0.03,
        num_running_requests: 1,
        num_waiting_requests: 0,
    };
    let model_2 = "gpt-3.5-turbo";
    let node_small_id_2 = 43;

    // Store metrics for both models
    collector.store_chat_completions_metrics(&chat_completions_1, model_1, node_small_id_1);
    collector.store_chat_completions_metrics(&chat_completions_2, model_2, node_small_id_2);

    // Verify metrics for first model
    let labels_1 = [model_1, &node_small_id_1.to_string()];
    assert_eq!(
        collector
            .get_chat_completions_gpu_kv_cache_usage()
            .with_label_values(&labels_1)
            .get(),
        75.5_f64
    );
    assert_eq!(
        collector
            .get_chat_completions_cpu_kv_cache_usage()
            .with_label_values(&labels_1)
            .get(),
        45.2_f64
    );
    assert_eq!(
        collector
            .get_chat_completions_ttft()
            .with_label_values(&labels_1)
            .get(),
        0.15_f64
    );
    assert_eq!(
        collector
            .get_chat_completions_tpot()
            .with_label_values(&labels_1)
            .get(),
        0.05_f64
    );
    assert_eq!(
        collector
            .get_chat_completions_num_running_requests()
            .with_label_values(&labels_1)
            .get(),
        3.0_f64
    );
    assert_eq!(
        collector
            .get_chat_completions_num_waiting_requests()
            .with_label_values(&labels_1)
            .get(),
        2.0_f64
    );

    // Verify metrics for second model
    let labels_2 = [model_2, &node_small_id_2.to_string()];
    assert_eq!(
        collector
            .get_chat_completions_gpu_kv_cache_usage()
            .with_label_values(&labels_2)
            .get(),
        60.0_f64
    );
    assert_eq!(
        collector
            .get_chat_completions_cpu_kv_cache_usage()
            .with_label_values(&labels_2)
            .get(),
        30.0_f64
    );
    assert_eq!(
        collector
            .get_chat_completions_ttft()
            .with_label_values(&labels_2)
            .get(),
        0.1_f64
    );
    assert_eq!(
        collector
            .get_chat_completions_tpot()
            .with_label_values(&labels_2)
            .get(),
        0.03_f64
    );
    assert_eq!(
        collector
            .get_chat_completions_num_running_requests()
            .with_label_values(&labels_2)
            .get(),
        1.0_f64
    );
    assert_eq!(
        collector
            .get_chat_completions_num_waiting_requests()
            .with_label_values(&labels_2)
            .get(),
        0.0_f64
    );
}

#[test]
#[allow(clippy::float_cmp)]
fn test_store_embeddings_metrics() {
    // Create a new NodeMetricsCollector instance
    let collector = NodeMetricsCollector::new().unwrap();

    // Create test data
    let embeddings = EmbeddingsMetrics {
        embeddings_queue_duration: 0.25,
        embeddings_inference_duration: 0.15,
        embeddings_input_length: 1000.0,
        embeddings_batch_size: 10.0,
        embeddings_batch_tokens: 1000.0,
    };
    let model = "text-embedding-ada-002";
    let node_small_id = 42;

    // Store the metrics
    collector.store_embeddings_metrics(&embeddings, model, node_small_id);

    // Verify each metric was stored correctly
    let labels = [model, &node_small_id.to_string()];

    assert_eq!(
        collector
            .get_embeddings_queue_duration()
            .with_label_values(&labels)
            .get(),
        0.25
    );
    assert_eq!(
        collector
            .get_embeddings_inference_duration()
            .with_label_values(&labels)
            .get(),
        0.15
    );
    assert_eq!(
        collector
            .get_embeddings_input_length()
            .with_label_values(&labels)
            .get(),
        1000.0
    );
    assert_eq!(
        collector
            .get_embeddings_batch_size()
            .with_label_values(&labels)
            .get(),
        10.0
    );
    assert_eq!(
        collector
            .get_embeddings_batch_tokens()
            .with_label_values(&labels)
            .get(),
        1000.0
    );
}

#[test]
#[allow(clippy::float_cmp)]
fn test_store_embeddings_metrics_multiple_models() {
    let collector = NodeMetricsCollector::new().unwrap();

    // Test data for first model
    let embeddings_1 = EmbeddingsMetrics {
        embeddings_queue_duration: 0.25,
        embeddings_inference_duration: 0.15,
        embeddings_input_length: 1000.0,
        embeddings_batch_size: 10.0,
        embeddings_batch_tokens: 1000.0,
    };
    let model_1 = "text-embedding-ada-002";
    let node_small_id_1 = 42;

    // Test data for second model
    let embeddings_2 = EmbeddingsMetrics {
        embeddings_queue_duration: 0.15,
        embeddings_inference_duration: 0.05,
        embeddings_input_length: 500.0,
        embeddings_batch_size: 5.0,
        embeddings_batch_tokens: 500.0,
    };
    let model_2 = "text-embedding-3-small";
    let node_small_id_2 = 43;

    // Store metrics for both models
    collector.store_embeddings_metrics(&embeddings_1, model_1, node_small_id_1);
    collector.store_embeddings_metrics(&embeddings_2, model_2, node_small_id_2);

    // Verify metrics for first model
    let labels_1 = [model_1, &node_small_id_1.to_string()];
    assert_eq!(
        collector
            .get_embeddings_queue_duration()
            .with_label_values(&labels_1)
            .get(),
        0.25
    );
    assert_eq!(
        collector
            .get_embeddings_inference_duration()
            .with_label_values(&labels_1)
            .get(),
        0.15
    );
    assert_eq!(
        collector
            .get_embeddings_input_length()
            .with_label_values(&labels_1)
            .get(),
        1000.0
    );
    assert_eq!(
        collector
            .get_embeddings_batch_size()
            .with_label_values(&labels_1)
            .get(),
        10.0
    );
    // Verify metrics for second model
    let labels_2 = [model_2, &node_small_id_2.to_string()];
    assert_eq!(
        collector
            .get_embeddings_queue_duration()
            .with_label_values(&labels_2)
            .get(),
        0.15
    );
    assert_eq!(
        collector
            .get_embeddings_inference_duration()
            .with_label_values(&labels_2)
            .get(),
        0.05
    );
    assert_eq!(
        collector
            .get_embeddings_input_length()
            .with_label_values(&labels_2)
            .get(),
        500.0
    );
    assert_eq!(
        collector
            .get_embeddings_batch_size()
            .with_label_values(&labels_2)
            .get(),
        5.0
    );
    assert_eq!(
        collector
            .get_embeddings_batch_tokens()
            .with_label_values(&labels_2)
            .get(),
        500.0
    );
}

#[test]
#[allow(clippy::float_cmp)]
fn test_store_image_generation_metrics() {
    // Create a new NodeMetricsCollector instance
    let collector = NodeMetricsCollector::new().unwrap();

    // Create test data
    let image_generation = ImageGenerationMetrics {
        image_generation_latency: 1.5,
        num_running_requests: 2,
    };
    let model = "dall-e-3";
    let node_small_id = 42;

    // Store the metrics
    collector.store_image_generation_metrics(&image_generation, model, node_small_id);

    // Verify each metric was stored correctly
    let labels = [model, &node_small_id.to_string()];

    assert_eq!(
        collector
            .get_image_generation_latency()
            .with_label_values(&labels)
            .get(),
        1.5
    );
    assert_eq!(
        collector
            .get_image_generation_num_running_requests()
            .with_label_values(&labels)
            .get(),
        2.0
    );
}

#[test]
#[allow(clippy::float_cmp)]
fn test_store_image_generation_metrics_multiple_models() {
    let collector = NodeMetricsCollector::new().unwrap();

    // Test data for first model
    let image_generation_1 = ImageGenerationMetrics {
        image_generation_latency: 1.5,
        num_running_requests: 2,
    };
    let model_1 = "dall-e-3";
    let node_small_id_1 = 42;

    // Test data for second model
    let image_generation_2 = ImageGenerationMetrics {
        image_generation_latency: 0.8,
        num_running_requests: 1,
    };
    let model_2 = "dall-e-2";
    let node_small_id_2 = 43;

    // Store metrics for both models
    collector.store_image_generation_metrics(&image_generation_1, model_1, node_small_id_1);
    collector.store_image_generation_metrics(&image_generation_2, model_2, node_small_id_2);

    // Verify metrics for first model
    let labels_1 = [model_1, &node_small_id_1.to_string()];
    assert_eq!(
        collector
            .get_image_generation_latency()
            .with_label_values(&labels_1)
            .get(),
        1.5
    );
    assert_eq!(
        collector
            .get_image_generation_num_running_requests()
            .with_label_values(&labels_1)
            .get(),
        2.0
    );

    // Verify metrics for second model
    let labels_2 = [model_2, &node_small_id_2.to_string()];
    assert_eq!(
        collector
            .get_image_generation_latency()
            .with_label_values(&labels_2)
            .get(),
        0.8
    );
    assert_eq!(
        collector
            .get_image_generation_num_running_requests()
            .with_label_values(&labels_2)
            .get(),
        1.0
    );
}

#[tokio::test]
#[allow(clippy::significant_drop_tightening)]
async fn test_retrieve_best_available_nodes_for_chat_completions() {
    // Use mockito's static server URL
    let mut server = mockito::Server::new_async().await;

    // Create mock response data
    let mock_response = json!({
        "status": "success",
        "data": {
            "resultType": "vector",
            "result": [
                {
                    "metric": { "node_small_id": "42" },
                    "value": [1_614_858_713.144, "0.2"]
                },
                {
                    "metric": { "node_small_id": "43" },
                    "value": [1_614_858_713.144, "0.33"]
                }
            ]
        }
    });

    // Set up mock endpoint
    let _m = server
        .mock("GET", "/api/v1/query")
        .match_query(mockito::Matcher::Any) // Accept any query parameter
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(serde_json::to_string(&mock_response).unwrap())
        .create();

    // Test retrieving best available nodes
    let result = NodeMetricsCollector::retrieve_best_available_nodes_for_chat_completions(
        &server.url(),
        "gpt-4",
        Some(2),
    )
    .await
    .expect("Failed to retrieve best available nodes");

    assert_eq!(result, vec![42, 43]);
}

#[tokio::test]
#[allow(clippy::significant_drop_tightening)]
async fn test_retrieve_best_available_nodes_for_embeddings() {
    // Use mockito's static server URL
    let mut server = mockito::Server::new_async().await;

    // Create mock response data
    let mock_response = json!({
        "status": "success",
        "data": {
            "resultType": "vector",
            "result": [
                {
                    "metric": { "node_small_id": "44" },
                    "value": [1_614_858_713.144, "0.15"]
                },
                {
                    "metric": { "node_small_id": "45" },
                    "value": [1_614_858_713.144, "0.25"]
                }
            ]
        }
    });

    // Set up mock endpoint
    let _m = server
        .mock("GET", "/api/v1/query")
        .match_query(mockito::Matcher::Any) // Accept any query parameter
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(serde_json::to_string(&mock_response).unwrap())
        .create();

    // Test retrieving best available nodes
    let result = NodeMetricsCollector::retrieve_best_available_nodes_for_embeddings(
        &server.url(),
        "text-embedding-ada-002",
        Some(2),
    )
    .await
    .expect("Failed to retrieve best available nodes");

    assert_eq!(result, vec![44, 45]);
}

#[tokio::test]
#[allow(clippy::significant_drop_tightening)]
async fn test_retrieve_best_available_nodes_for_image_generation() {
    // Use mockito's static server URL
    let mut server = mockito::Server::new_async().await;

    // Create mock response data
    let mock_response = json!({
        "status": "success",
        "data": {
            "resultType": "vector",
            "result": [
                {
                    "metric": { "node_small_id": "46" },
                    "value": [1_614_858_713.144, "1.2"]
                },
                {
                    "metric": { "node_small_id": "47" },
                    "value": [1_614_858_713.144, "1.5"]
                }
            ]
        }
    });

    // Set up mock endpoint
    let _m = server
        .mock("GET", "/api/v1/query")
        .match_query(mockito::Matcher::Any) // Accept any query parameter
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(serde_json::to_string(&mock_response).unwrap())
        .create();

    // Test retrieving best available nodes
    let result = NodeMetricsCollector::retrieve_best_available_nodes_for_image_generation(
        &server.url(),
        "dall-e-3",
        Some(2),
    )
    .await
    .expect("Failed to retrieve best available nodes");

    assert_eq!(result, vec![46, 47]);
}

#[test]
#[allow(clippy::too_many_lines)]
#[allow(clippy::float_cmp)]
fn test_reset_metrics() {
    // Create a new NodeMetricsCollector instance
    let collector = NodeMetricsCollector::new().unwrap();

    // Store some test metrics
    let chat_completions = ChatCompletionsMetrics {
        gpu_kv_cache_usage_perc: 75.5,
        cpu_kv_cache_usage_perc: 45.2,
        time_to_first_token: 0.15,
        time_per_output_token: 0.05,
        num_running_requests: 3,
        num_waiting_requests: 2,
    };
    let embeddings = EmbeddingsMetrics {
        embeddings_queue_duration: 0.25,
        embeddings_inference_duration: 0.15,
        embeddings_input_length: 1000.0,
        embeddings_batch_size: 10.0,
        embeddings_batch_tokens: 1000.0,
    };
    let image_generation = ImageGenerationMetrics {
        image_generation_latency: 1.5,
        num_running_requests: 2,
    };

    // Store metrics for different models
    collector.store_chat_completions_metrics(&chat_completions, "gpt-4", 42);
    collector.store_embeddings_metrics(&embeddings, "text-embedding-ada-002", 43);
    collector.store_image_generation_metrics(&image_generation, "dall-e-3", 44);

    // Verify metrics were stored
    let chat_labels = ["gpt-4", "42"];
    let embeddings_labels = ["text-embedding-ada-002", "43"];
    let image_labels = ["dall-e-3", "44"];

    assert_eq!(
        collector
            .get_chat_completions_gpu_kv_cache_usage()
            .with_label_values(&chat_labels)
            .get(),
        75.5
    );
    assert_eq!(
        collector
            .get_embeddings_queue_duration()
            .with_label_values(&embeddings_labels)
            .get(),
        0.25
    );
    assert_eq!(
        collector
            .get_embeddings_inference_duration()
            .with_label_values(&embeddings_labels)
            .get(),
        0.15
    );
    assert_eq!(
        collector
            .get_image_generation_latency()
            .with_label_values(&image_labels)
            .get(),
        1.5
    );
    assert_eq!(
        collector
            .get_embeddings_input_length()
            .with_label_values(&embeddings_labels)
            .get(),
        1000.0
    );
    assert_eq!(
        collector
            .get_embeddings_batch_size()
            .with_label_values(&embeddings_labels)
            .get(),
        10.0
    );
    // Reset all metrics
    collector.reset_metrics();

    // Verify all metrics were reset to their default values
    assert_eq!(
        collector
            .get_chat_completions_gpu_kv_cache_usage()
            .with_label_values(&chat_labels)
            .get(),
        0.0
    );
    assert_eq!(
        collector
            .get_chat_completions_cpu_kv_cache_usage()
            .with_label_values(&chat_labels)
            .get(),
        0.0
    );
    assert_eq!(
        collector
            .get_chat_completions_ttft()
            .with_label_values(&chat_labels)
            .get(),
        0.0
    );
    assert_eq!(
        collector
            .get_chat_completions_tpot()
            .with_label_values(&chat_labels)
            .get(),
        0.0
    );
    assert_eq!(
        collector
            .get_chat_completions_num_running_requests()
            .with_label_values(&chat_labels)
            .get(),
        0.0
    );
    assert_eq!(
        collector
            .get_chat_completions_num_waiting_requests()
            .with_label_values(&chat_labels)
            .get(),
        0.0
    );
    assert_eq!(
        collector
            .get_embeddings_queue_duration()
            .with_label_values(&embeddings_labels)
            .get(),
        0.0
    );
    assert_eq!(
        collector
            .get_embeddings_inference_duration()
            .with_label_values(&embeddings_labels)
            .get(),
        0.0
    );
    assert_eq!(
        collector
            .get_embeddings_input_length()
            .with_label_values(&embeddings_labels)
            .get(),
        0.0
    );
    assert_eq!(
        collector
            .get_embeddings_batch_size()
            .with_label_values(&embeddings_labels)
            .get(),
        0.0
    );
    assert_eq!(
        collector
            .get_embeddings_batch_tokens()
            .with_label_values(&embeddings_labels)
            .get(),
        0.0
    );
    assert_eq!(
        collector
            .get_image_generation_latency()
            .with_label_values(&image_labels)
            .get(),
        0.0
    );
    assert_eq!(
        collector
            .get_image_generation_num_running_requests()
            .with_label_values(&image_labels)
            .get(),
        0.0
    );
}

#[test]
#[allow(clippy::too_many_lines)]
#[allow(clippy::float_cmp)]
fn test_store_metrics() {
    // Create a new NodeMetricsCollector instance
    let collector = NodeMetricsCollector::new().unwrap();

    // Create a NodeMetrics instance with metrics for multiple models
    let mut model_metrics = HashMap::new();

    // Add chat completions metrics
    model_metrics.insert(
        "gpt-4".to_string(),
        ModelMetrics::ChatCompletions(ChatCompletionsMetrics {
            gpu_kv_cache_usage_perc: 75.5,
            cpu_kv_cache_usage_perc: 45.2,
            time_to_first_token: 0.15,
            time_per_output_token: 0.05,
            num_running_requests: 3,
            num_waiting_requests: 2,
        }),
    );

    // Add embeddings metrics
    model_metrics.insert(
        "text-embedding-ada-002".to_string(),
        ModelMetrics::Embeddings(EmbeddingsMetrics {
            embeddings_queue_duration: 0.25,
            embeddings_inference_duration: 0.15,
            embeddings_input_length: 1000.0,
            embeddings_batch_size: 10.0,
            embeddings_batch_tokens: 1000.0,
        }),
    );

    // Add image generation metrics
    model_metrics.insert(
        "dall-e-3".to_string(),
        ModelMetrics::ImageGeneration(ImageGenerationMetrics {
            image_generation_latency: 1.5,
            num_running_requests: 2,
        }),
    );

    // Create the NodeMetrics instance
    let node_metrics = NodeMetrics { model_metrics };

    // Store all metrics at once
    let node_small_id = 42;
    collector.store_metrics(&node_metrics, node_small_id);

    // Verify that all metrics were stored correctly
    let chat_labels = ["gpt-4", "42"];
    let embeddings_labels = ["text-embedding-ada-002", "42"];
    let image_labels = ["dall-e-3", "42"];

    // Verify chat completions metrics
    assert_eq!(
        collector
            .get_chat_completions_gpu_kv_cache_usage()
            .with_label_values(&chat_labels)
            .get(),
        75.5
    );
    assert_eq!(
        collector
            .get_chat_completions_cpu_kv_cache_usage()
            .with_label_values(&chat_labels)
            .get(),
        45.2
    );
    assert_eq!(
        collector
            .get_chat_completions_ttft()
            .with_label_values(&chat_labels)
            .get(),
        0.15
    );
    assert_eq!(
        collector
            .get_chat_completions_tpot()
            .with_label_values(&chat_labels)
            .get(),
        0.05
    );
    assert_eq!(
        collector
            .get_chat_completions_num_running_requests()
            .with_label_values(&chat_labels)
            .get(),
        3.0
    );
    assert_eq!(
        collector
            .get_chat_completions_num_waiting_requests()
            .with_label_values(&chat_labels)
            .get(),
        2.0
    );

    // Verify embeddings metrics
    assert_eq!(
        collector
            .get_embeddings_queue_duration()
            .with_label_values(&embeddings_labels)
            .get(),
        0.25
    );
    assert_eq!(
        collector
            .get_embeddings_inference_duration()
            .with_label_values(&embeddings_labels)
            .get(),
        0.15
    );
    assert_eq!(
        collector
            .get_embeddings_input_length()
            .with_label_values(&embeddings_labels)
            .get(),
        1000.0
    );
    assert_eq!(
        collector
            .get_embeddings_batch_size()
            .with_label_values(&embeddings_labels)
            .get(),
        10.0
    );
    assert_eq!(
        collector
            .get_embeddings_batch_tokens()
            .with_label_values(&embeddings_labels)
            .get(),
        1000.0
    );
    // Verify image generation metrics
    assert_eq!(
        collector
            .get_image_generation_latency()
            .with_label_values(&image_labels)
            .get(),
        1.5
    );
    assert_eq!(
        collector
            .get_image_generation_num_running_requests()
            .with_label_values(&image_labels)
            .get(),
        2.0
    );
}

#[tokio::test]
#[allow(clippy::significant_drop_tightening)]
async fn test_retrieve_nodes_error_handling() {
    // Use mockito's static server URL
    let mut server = mockito::Server::new_async().await;

    // Test case 1: HTTP error (404 Not Found)
    let _m = server
        .mock("GET", "/api/v1/query")
        .match_query(mockito::Matcher::Any)
        .with_status(404)
        .with_body("Not Found")
        .create();

    // Test all three public methods with the error condition
    let chat_result = NodeMetricsCollector::retrieve_best_available_nodes_for_chat_completions(
        &server.url(),
        "gpt-4",
        Some(5),
    )
    .await;

    let embeddings_result = NodeMetricsCollector::retrieve_best_available_nodes_for_embeddings(
        &server.url(),
        "text-embedding-ada-002",
        Some(5),
    )
    .await;

    let image_result = NodeMetricsCollector::retrieve_best_available_nodes_for_image_generation(
        &server.url(),
        "dall-e-3",
        Some(5),
    )
    .await;

    assert!(
        chat_result.is_err(),
        "Chat completions should return error for HTTP 404"
    );
    assert!(
        embeddings_result.is_err(),
        "Embeddings should return error for HTTP 404"
    );
    assert!(
        image_result.is_err(),
        "Image generation should return error for HTTP 404"
    );

    // Remove the previous mock
    server.reset();

    // Test case 2: Invalid JSON response
    let _m = server
        .mock("GET", "/api/v1/query")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("invalid json")
        .create();

    let chat_result = NodeMetricsCollector::retrieve_best_available_nodes_for_chat_completions(
        &server.url(),
        "gpt-4",
        Some(5),
    )
    .await;

    assert!(chat_result.is_err(), "Should return error for invalid JSON");

    // Remove the previous mock
    server.reset();

    // Test case 3: Empty result set
    let empty_response = json!({
        "status": "success",
        "data": {
            "resultType": "vector",
            "result": []
        }
    });

    let _m = server
        .mock("GET", "/api/v1/query")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(serde_json::to_string(&empty_response).unwrap())
        .create();

    let chat_result = NodeMetricsCollector::retrieve_best_available_nodes_for_chat_completions(
        &server.url(),
        "gpt-4",
        Some(5),
    )
    .await;

    assert!(chat_result.is_ok(), "Should handle empty result set");
    assert_eq!(
        chat_result.unwrap().len(),
        0,
        "Should return empty vector for empty result set"
    );
}
