-- Add migration script here
CREATE TABLE node_metrics (
    node_small_id                  BIGINT NOT NULL,
    timestamp                      BIGINT NOT NULL,
    
    -- CPU metrics
    cpu_usage                      REAL NOT NULL,  -- Percentage (0-100)
    num_cpus                       INTEGER NOT NULL,
    
    -- RAM metrics (in bytes)
    ram_used                       BIGINT NOT NULL,
    ram_total                      BIGINT NOT NULL,
    ram_swap_used                  BIGINT NOT NULL,
    ram_swap_total                 BIGINT NOT NULL,
    
    -- Network metrics (in bytes)
    network_rx                     BIGINT NOT NULL,    -- Received bytes
    network_tx                     BIGINT NOT NULL,    -- Transmitted bytes
    
    -- GPU metrics
    num_gpus                       INTEGER NOT NULL,
    gpu_memory_used                BIGINT[] NOT NULL,   -- Bytes
    gpu_memory_total               BIGINT[] NOT NULL,   -- Bytes
    gpu_memory_free                BIGINT[] NOT NULL,   -- Bytes
    gpu_percentage_time_read_write FLOAT[] NOT NULL,    -- Percentage (0-100)
    gpu_percentage_time_execution  FLOAT[] NOT NULL,    -- Percentage (0-100)
    gpu_temperatures               FLOAT[] NOT NULL,    -- Celsius
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