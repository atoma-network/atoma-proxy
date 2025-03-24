-- Create tasks table
CREATE TABLE IF NOT EXISTS tasks (
    -- Unique identifier for the task
    task_small_id BIGINT PRIMARY KEY,

    -- Unique identifier for the task
    task_id TEXT UNIQUE NOT NULL,

    -- Role of the task
    role SMALLINT NOT NULL,

    -- Name of the model used for the task
    model_name TEXT,

    -- Whether the task is deprecated
    is_deprecated BOOLEAN NOT NULL,

    -- Epoch until which the task is valid
    valid_until_epoch BIGINT,

    -- Epoch at which the task was deprecated
    deprecated_at_epoch BIGINT,

    -- Security level of the task
    security_level INTEGER NOT NULL,

    -- Minimum reputation score for the task
    minimum_reputation_score SMALLINT
);

-- Create nodes table
CREATE TABLE IF NOT EXISTS nodes (
    -- Unique identifier for the node
    node_small_id BIGINT PRIMARY KEY,

    -- Sui blockchain address of the node
    sui_address TEXT NOT NULL,

    -- Public address of the node
    public_address TEXT,

    -- Country of the node
    country TEXT
);

-- Create node_subscriptions table
CREATE TABLE IF NOT EXISTS node_subscriptions (
    -- Unique identifier for the task
    task_small_id BIGINT NOT NULL,

    -- Unique identifier for the node
    node_small_id BIGINT NOT NULL,

    -- Price per one million compute units for the node
    price_per_one_million_compute_units BIGINT NOT NULL,

    -- Maximum number of compute units for the node
    max_num_compute_units BIGINT NOT NULL,

    -- Whether the node is valid
    valid BOOLEAN NOT NULL,

    PRIMARY KEY (task_small_id, node_small_id),
    FOREIGN KEY (task_small_id) REFERENCES tasks (task_small_id),
    FOREIGN KEY (node_small_id) REFERENCES nodes (node_small_id)
);

CREATE INDEX IF NOT EXISTS idx_node_subscriptions_task_small_id_node_small_id ON node_subscriptions (task_small_id, node_small_id);

-- Create users and auth tables
CREATE TABLE IF NOT EXISTS users (
    -- Unique identifier for the user
    id BIGSERIAL PRIMARY KEY,

    -- Username of the user
    username VARCHAR(50) UNIQUE NOT NULL,

    -- Password hash of the user
    password_hash VARCHAR(64) NOT NULL
);

CREATE TABLE IF NOT EXISTS refresh_tokens (
    -- Unique identifier for the refresh token
    id BIGSERIAL PRIMARY KEY,

    -- Token hash of the refresh token
    token_hash VARCHAR(255) UNIQUE NOT NULL,

    -- Unique identifier for the user
    user_id BIGINT NOT NULL,

    FOREIGN KEY (user_id) REFERENCES users (id)
);

CREATE TABLE IF NOT EXISTS api_tokens (
    -- Unique identifier for the API token
    id BIGSERIAL PRIMARY KEY,

    -- Token of the API token
    token VARCHAR(255) UNIQUE NOT NULL,

    -- Unique identifier for the user
    user_id BIGINT NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users (id)
);

-- Create stacks table
CREATE TABLE IF NOT EXISTS stacks (
    -- Unique identifier for the stack
    stack_small_id BIGINT PRIMARY KEY,

    -- Owner of the stack
    owner TEXT NOT NULL,

    -- Unique identifier for the stack
    stack_id TEXT UNIQUE NOT NULL,

    -- Unique identifier for the task
    task_small_id BIGINT NOT NULL,

    -- Unique identifier for the selected node
    selected_node_id BIGINT NOT NULL,

    -- Number of compute units for the stack
    num_compute_units BIGINT NOT NULL,

    -- Price per one million compute units for the stack
    price_per_one_million_compute_units BIGINT NOT NULL,

    -- Number of compute units already processed for the stack
    already_computed_units BIGINT NOT NULL,

    -- Whether the stack is in the settle period
    in_settle_period BOOLEAN NOT NULL,

    -- Total hash of the stack
    total_hash BYTEA NOT NULL,

    -- Number of total messages for the stack
    num_total_messages BIGINT NOT NULL,

    -- Unique identifier for the user
    user_id BIGINT NOT NULL,

    FOREIGN KEY (task_small_id) REFERENCES tasks (task_small_id),
    FOREIGN KEY (user_id) REFERENCES users (id),
    FOREIGN KEY (selected_node_id, task_small_id) REFERENCES node_subscriptions (node_small_id, task_small_id)
);

-- Create indices for stacks table
CREATE INDEX IF NOT EXISTS idx_stacks_owner_address ON stacks (owner);
CREATE INDEX IF NOT EXISTS idx_stacks_task_small_id ON stacks (task_small_id);
CREATE INDEX IF NOT EXISTS idx_stacks_stack_small_id ON stacks (stack_small_id);

-- Create stack settlement tickets table
CREATE TABLE IF NOT EXISTS stack_settlement_tickets (
    -- Unique identifier for the stack
    stack_small_id BIGINT PRIMARY KEY,

    -- Unique identifier for the selected node
    selected_node_id INTEGER NOT NULL,

    -- Number of claimed compute units for the stack
    num_claimed_compute_units INTEGER NOT NULL,

    -- Requested attestation nodes for the stack
    requested_attestation_nodes TEXT NOT NULL,

    -- Committed stack proofs for the stack
    committed_stack_proofs BYTEA NOT NULL,

    -- Stack merkle leaves for the stack
    stack_merkle_leaves BYTEA NOT NULL,

    -- Dispute settled at epoch for the stack
    dispute_settled_at_epoch INTEGER,

    -- Already attested nodes for the stack
    already_attested_nodes TEXT NOT NULL,

    -- Whether the stack is in dispute
    is_in_dispute BOOLEAN NOT NULL,

    -- User refund amount for the stack
    user_refund_amount INTEGER NOT NULL,

    -- Whether the stack is claimed
    is_claimed BOOLEAN NOT NULL,

    FOREIGN KEY (stack_small_id) REFERENCES stacks (stack_small_id)
);

-- Create index for stack settlement tickets table
CREATE INDEX IF NOT EXISTS idx_stack_settlement_tickets_stack_small_id ON stack_settlement_tickets (stack_small_id);

-- Create stack attestation disputes table
CREATE TABLE IF NOT EXISTS stack_attestation_disputes (
    -- Unique identifier for the stack
    stack_small_id BIGINT NOT NULL,

    -- Attestation commitment for the stack
    attestation_commitment BYTEA NOT NULL,

    -- Attestation node ID for the stack
    attestation_node_id INTEGER NOT NULL,

    -- Original node ID for the stack
    original_node_id INTEGER NOT NULL,

    -- Original commitment for the stack
    original_commitment BYTEA NOT NULL,

    PRIMARY KEY (stack_small_id, attestation_node_id),
    FOREIGN KEY (stack_small_id) REFERENCES stacks (stack_small_id)
);

-- Create node performance tables
CREATE TABLE IF NOT EXISTS node_throughput_performance (
    -- Unique identifier for the node
    node_small_id BIGINT PRIMARY KEY,

    -- Number of queries for the node
    queries BIGINT NOT NULL,

    -- Number of input tokens for the node
    input_tokens BIGINT NOT NULL,

    -- Number of output tokens for the node
    output_tokens BIGINT NOT NULL,

    -- Time taken for the node
    time DOUBLE PRECISION NOT NULL,

    FOREIGN KEY (node_small_id) REFERENCES nodes (node_small_id)
);

CREATE TABLE IF NOT EXISTS node_prefill_performance (
    -- Unique identifier for the node
    node_small_id BIGINT PRIMARY KEY,

    -- Number of queries for the node
    queries BIGINT NOT NULL,

    -- Number of tokens for the node
    tokens BIGINT NOT NULL,

    -- Time taken for the node
    time DOUBLE PRECISION NOT NULL,

    FOREIGN KEY (node_small_id) REFERENCES nodes (node_small_id)
);

CREATE TABLE IF NOT EXISTS node_decode_performance (
    -- Unique identifier for the node
    node_small_id BIGINT PRIMARY KEY,

    -- Number of queries for the node
    queries BIGINT NOT NULL,

    -- Number of tokens for the node
    tokens BIGINT NOT NULL,

    -- Time taken for the node
    time DOUBLE PRECISION NOT NULL,
    FOREIGN KEY (node_small_id) REFERENCES nodes (node_small_id)
);

CREATE TABLE IF NOT EXISTS node_latency_performance (
    -- Unique identifier for the node
    node_small_id BIGINT PRIMARY KEY,

    -- Number of queries for the node
    queries BIGINT NOT NULL,

    -- Latency for the node
    latency DOUBLE PRECISION NOT NULL,
    FOREIGN KEY (node_small_id) REFERENCES nodes (node_small_id)
);

-- Create node_public_keys table
CREATE TABLE IF NOT EXISTS node_public_keys (
    -- Unique identifier for the node
    node_small_id BIGINT PRIMARY KEY,

    -- Epoch for the node
    epoch BIGINT NOT NULL,

    -- Key rotation counter for the node
    key_rotation_counter BIGINT NOT NULL,

    -- Public key for the node
    public_key BYTEA NOT NULL,

    -- Tee remote attestation bytes for the node
    tee_remote_attestation_bytes BYTEA NOT NULL,

    -- Whether the node is valid
    is_valid BOOLEAN NOT NULL,
    FOREIGN KEY (node_small_id) REFERENCES nodes (node_small_id)
);

-- Create stats tables
CREATE TABLE IF NOT EXISTS stats_compute_units_processed (
    -- Unique identifier for the stats
    id BIGSERIAL PRIMARY KEY,

    -- Timestamp for the stats
    timestamp TIMESTAMPTZ NOT NULL,

    -- Model name for the stats
    model_name TEXT NOT NULL,

    -- Amount for the stats 
    amount BIGINT NOT NULL,

    -- Requests for the stats
    requests BIGINT NOT NULL DEFAULT 1,

    -- Time for the stats
    time DOUBLE PRECISION NOT NULL,
    
    UNIQUE (timestamp, model_name),
    CHECK (date_part('minute', timestamp) = 0 AND date_part('second', timestamp) = 0 AND date_part('milliseconds', timestamp) = 0)
);

CREATE INDEX IF NOT EXISTS idx_stats_compute_units_processed ON stats_compute_units_processed (timestamp);

CREATE TABLE IF NOT EXISTS stats_latency (
    -- Unique identifier for the stats
    id BIGSERIAL PRIMARY KEY,

    -- Timestamp for the stats
    timestamp TIMESTAMPTZ UNIQUE NOT NULL,

    -- Latency for the stats
    latency DOUBLE PRECISION NOT NULL,

    -- Requests for the stats
    requests BIGINT NOT NULL DEFAULT 1,
    CHECK (date_part('minute', timestamp) = 0 AND date_part('second', timestamp) = 0 AND date_part('milliseconds', timestamp) = 0)
);

CREATE INDEX IF NOT EXISTS idx_stats_latency ON stats_latency (timestamp);

-- Create stats_stacks table
CREATE TABLE IF NOT EXISTS stats_stacks (
    -- Unique identifier for the stats
    timestamp TIMESTAMPTZ PRIMARY KEY NOT NULL,

    -- Number of compute units for the stats
    num_compute_units BIGINT NOT NULL DEFAULT 0,

    -- Settled number of compute units for the stats
    settled_num_compute_units BIGINT NOT NULL DEFAULT 0,
    
    CHECK (date_part('minute', timestamp) = 0 AND date_part('second', timestamp) = 0 AND date_part('milliseconds', timestamp) = 0)
);

CREATE INDEX IF NOT EXISTS idx_stats_stacks ON stats_stacks (timestamp);
