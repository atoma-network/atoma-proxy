-- Add migration script here
CREATE TABLE node_metrics (
    -- Unique identifier for the node
    node_small_id                  BIGINT NOT NULL,

    -- Timestamp for the node metrics
    timestamp                      BIGINT NOT NULL,
    
    -- CPU metrics
    cpu_usage                      REAL NOT NULL,  -- Percentage (0-100)

    -- Number of CPUs
    num_cpus                       INTEGER NOT NULL,
    
    -- RAM metrics (in bytes)
    ram_used                       BIGINT NOT NULL,

    -- Total RAM available in bytes
    ram_total                      BIGINT NOT NULL,

    -- Current swap memory usage in bytes
    ram_swap_used                  BIGINT NOT NULL,

    -- Total swap memory available in bytes
    ram_swap_total                 BIGINT NOT NULL,
    
    -- Network metrics (in bytes)
    -- Total bytes received over network
    network_rx                     BIGINT NOT NULL,    -- Received bytes

    -- Total bytes transmitted over network
    network_tx                     BIGINT NOT NULL,    -- Transmitted bytes
    
    -- GPU metrics

    -- Number of GPUs
    num_gpus                       INTEGER NOT NULL,

    -- Memory used per GPU in bytes
    gpu_memory_used                BIGINT[] NOT NULL,   -- Bytes

    -- Total memory available per GPU in bytes
    gpu_memory_total               BIGINT[] NOT NULL,   -- Bytes

    -- Free memory available per GPU in bytes
    gpu_memory_free                BIGINT[] NOT NULL,   -- Bytes

    -- Percentage of time the GPU was reading or writing
    gpu_percentage_time_read_write FLOAT[] NOT NULL,    -- Percentage (0-100)

    -- Percentage of time the GPU was executing
    gpu_percentage_time_execution  FLOAT[] NOT NULL,    -- Percentage (0-100)

    -- Temperature of the GPU in Celsius
    gpu_temperatures               FLOAT[] NOT NULL,    -- Celsius

    -- Power usage of the GPU in watts
    gpu_power_usages               FLOAT[] NOT NULL,
    
    -- Metadata
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    
    -- Primary key constraint
    PRIMARY KEY (node_small_id, timestamp),
    
    -- Indexes for common queries
    CONSTRAINT fk_node
        FOREIGN KEY (node_small_id)
        REFERENCES nodes(node_small_id)
        ON DELETE CASCADE
);

-- Create index for time-series queries
CREATE INDEX idx_node_metrics_timestamp ON node_metrics (node_small_id, timestamp);