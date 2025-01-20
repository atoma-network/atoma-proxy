use serde::Deserialize;

/// A query params for latency requests. Since the latencies are on hourly basis. It will return last `LatencyQuery::hours` hours of latencies.
#[derive(Deserialize)]
pub struct LatencyQuery {
    pub hours: usize,
}

/// A query params for compute units processed requests. Since the compute units are on hourly basis. It will return last `ComputeUnitsProcessedQuery::hours` hours of compute units processed.
#[derive(Deserialize)]
pub struct ComputeUnitsProcessedQuery {
    pub hours: usize,
}

/// A query params for stats for stacks requests. Since the stats are on hourly basis. It will return last `StatsStackQuery::hours` hours of stacks stats.
#[derive(Deserialize)]
pub struct StatsStackQuery {
    pub hours: usize,
}
