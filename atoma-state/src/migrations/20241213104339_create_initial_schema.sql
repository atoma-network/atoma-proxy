-- Create tasks table
CREATE TABLE IF NOT EXISTS tasks (
    task_small_id BIGINT PRIMARY KEY,
    task_id TEXT UNIQUE NOT NULL,
    role SMALLINT NOT NULL,
    model_name TEXT,
    is_deprecated BOOLEAN NOT NULL,
    valid_until_epoch BIGINT,
    deprecated_at_epoch BIGINT,
    security_level INTEGER NOT NULL,
    minimum_reputation_score SMALLINT
);

-- Create nodes table
CREATE TABLE IF NOT EXISTS nodes (
    node_small_id BIGINT PRIMARY KEY,
    sui_address TEXT NOT NULL,
    public_address TEXT,
    country TEXT
);

-- Create node_subscriptions table
CREATE TABLE IF NOT EXISTS node_subscriptions (
    task_small_id BIGINT NOT NULL,
    node_small_id BIGINT NOT NULL,
    price_per_compute_unit BIGINT NOT NULL,
    max_num_compute_units BIGINT NOT NULL,
    valid BOOLEAN NOT NULL,
    PRIMARY KEY (task_small_id, node_small_id),
    FOREIGN KEY (task_small_id) REFERENCES tasks (task_small_id),
    FOREIGN KEY (node_small_id) REFERENCES nodes (node_small_id)
);

CREATE INDEX IF NOT EXISTS idx_node_subscriptions_task_small_id_node_small_id ON node_subscriptions (task_small_id, node_small_id);

-- Create users and auth tables
CREATE TABLE IF NOT EXISTS users (
    id BIGSERIAL PRIMARY KEY,
    username VARCHAR(50) UNIQUE NOT NULL,
    password_hash VARCHAR(64) NOT NULL
);

CREATE TABLE IF NOT EXISTS refresh_tokens (
    id BIGSERIAL PRIMARY KEY,
    token_hash VARCHAR(255) UNIQUE NOT NULL,
    user_id BIGINT NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users (id)
);

CREATE TABLE IF NOT EXISTS api_tokens (
    id BIGSERIAL PRIMARY KEY,
    token VARCHAR(255) UNIQUE NOT NULL,
    user_id BIGINT NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users (id)
);

-- Create stacks table
CREATE TABLE IF NOT EXISTS stacks (
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
    is_claimed BOOLEAN NOT NULL,
    FOREIGN KEY (stack_small_id) REFERENCES stacks (stack_small_id)
);

-- Create index for stack settlement tickets table
CREATE INDEX IF NOT EXISTS idx_stack_settlement_tickets_stack_small_id ON stack_settlement_tickets (stack_small_id);

-- Create stack attestation disputes table
CREATE TABLE IF NOT EXISTS stack_attestation_disputes (
    stack_small_id BIGINT NOT NULL,
    attestation_commitment BYTEA NOT NULL,
    attestation_node_id INTEGER NOT NULL,
    original_node_id INTEGER NOT NULL,
    original_commitment BYTEA NOT NULL,
    PRIMARY KEY (stack_small_id, attestation_node_id),
    FOREIGN KEY (stack_small_id) REFERENCES stacks (stack_small_id)
);

-- Create node performance tables
CREATE TABLE IF NOT EXISTS node_throughput_performance (
    node_small_id BIGINT PRIMARY KEY,
    queries BIGINT NOT NULL,
    input_tokens BIGINT NOT NULL,
    output_tokens BIGINT NOT NULL,
    time DOUBLE PRECISION NOT NULL,
    FOREIGN KEY (node_small_id) REFERENCES nodes (node_small_id)
);

CREATE TABLE IF NOT EXISTS node_prefill_performance (
    node_small_id BIGINT PRIMARY KEY,
    queries BIGINT NOT NULL,
    tokens BIGINT NOT NULL,
    time DOUBLE PRECISION NOT NULL,
    FOREIGN KEY (node_small_id) REFERENCES nodes (node_small_id)
);

CREATE TABLE IF NOT EXISTS node_decode_performance (
    node_small_id BIGINT PRIMARY KEY,
    queries BIGINT NOT NULL,
    tokens BIGINT NOT NULL,
    time DOUBLE PRECISION NOT NULL,
    FOREIGN KEY (node_small_id) REFERENCES nodes (node_small_id)
);

CREATE TABLE IF NOT EXISTS node_latency_performance (
    node_small_id BIGINT PRIMARY KEY,
    queries BIGINT NOT NULL,
    latency DOUBLE PRECISION NOT NULL,
    FOREIGN KEY (node_small_id) REFERENCES nodes (node_small_id)
);

-- Create node_public_keys table
CREATE TABLE IF NOT EXISTS node_public_keys (
    node_small_id BIGINT PRIMARY KEY,
    epoch BIGINT NOT NULL,
    public_key BYTEA NOT NULL,
    tee_remote_attestation_bytes BYTEA NOT NULL,
    FOREIGN KEY (node_small_id) REFERENCES nodes (node_small_id)
);

-- Create stats tables
CREATE TABLE IF NOT EXISTS stats_compute_units_processed (
    id BIGSERIAL PRIMARY KEY,
    timestamp TIMESTAMPTZ NOT NULL,
    model_name TEXT NOT NULL,
    amount BIGINT NOT NULL,
    requests BIGINT NOT NULL DEFAULT 1,
    time DOUBLE PRECISION NOT NULL,
    UNIQUE (timestamp, model_name),
    CHECK (date_part('minute', timestamp) = 0 AND date_part('second', timestamp) = 0 AND date_part('milliseconds', timestamp) = 0)
);

CREATE INDEX IF NOT EXISTS idx_stats_compute_units_processed ON stats_compute_units_processed (timestamp);

CREATE TABLE IF NOT EXISTS stats_latency (
    id BIGSERIAL PRIMARY KEY,
    timestamp TIMESTAMPTZ UNIQUE NOT NULL,
    latency DOUBLE PRECISION NOT NULL,
    requests BIGINT NOT NULL DEFAULT 1,
    CHECK (date_part('minute', timestamp) = 0 AND date_part('second', timestamp) = 0 AND date_part('milliseconds', timestamp) = 0)
);

CREATE INDEX IF NOT EXISTS idx_stats_latency ON stats_latency (timestamp);

-- Create stats_stacks table
CREATE TABLE IF NOT EXISTS stats_stacks (
    timestamp TIMESTAMPTZ PRIMARY KEY NOT NULL,
    num_compute_units BIGINT NOT NULL DEFAULT 0,
    settled_num_compute_units BIGINT NOT NULL DEFAULT 0,
    CHECK (date_part('minute', timestamp) = 0 AND date_part('second', timestamp) = 0 AND date_part('milliseconds', timestamp) = 0)
);

CREATE INDEX IF NOT EXISTS idx_stats_stacks ON stats_stacks (timestamp);
