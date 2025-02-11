-- This table stores weight coefficients used to calculate overall performance scores
-- for computing resources. These weights determine the relative importance of different
-- hardware metrics in the final performance calculation.
CREATE TABLE performance_weights (
    id SERIAL PRIMARY KEY,

    -- Weight factor for GPU performance in the overall score calculation (0.0 to 1.0)
    gpu_score_weight     DOUBLE PRECISION  NOT NULL,

    -- Weight factor for CPU performance in the overall score calculation (0.0 to 1.0)
    cpu_score_weight     DOUBLE PRECISION NOT NULL,

    -- Weight factor for RAM capacity/performance in the overall score calculation (0.0 to 1.0)
    ram_score_weight     DOUBLE PRECISION NOT NULL,

    -- Weight factor for network performance in the overall score calculation (0.0 to 1.0)
    network_score_weight DOUBLE PRECISION NOT NULL,

    -- The threshold for GPU temperature in the overall score calculation
    gpu_temp_threshold   DOUBLE PRECISION NOT NULL,

    -- The maximum temperature for GPU in the overall score calculation
    gpu_temp_max  DOUBLE PRECISION NOT NULL,

    -- The threshold for GPU power usage in the overall score calculation
    gpu_power_threshold  DOUBLE PRECISION NOT NULL,

    -- The maximum power usage for GPU in the overall score calculation
    gpu_power_max  DOUBLE PRECISION NOT NULL

    -- GPU internals specific weights

    -- GPU VRAM usage specific weight
    gpu_vram_weight DOUBLE PRECISION NOT NULL,

    -- GPU execution availability specific weight
    gpu_exec_avail_weight DOUBLE PRECISION NOT NULL,

    -- GPU temperature specific weight
    gpu_temp_weight DOUBLE PRECISION NOT NULL,

    -- GPU power usage specific weight
    gpu_power_weight DOUBLE PRECISION NOT NULL,
    
    
    
);
