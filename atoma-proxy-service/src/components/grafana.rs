use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tracing::instrument;

use crate::handlers::stats::PanelResponse;

/// The list of dashboards uids returned from grafana
#[derive(Deserialize)]
struct DashboardList {
    /// The list of dashboards
    uid: String,
}

/// The time struct from grafana
#[derive(Deserialize, Serialize)]
struct Time {
    /// The time range from which to query, in the format like "now-1w"
    from: String,
    /// The time range to which to query, usually it's just "now"
    to: String,
}

/// The panel struct from grafana, but incomplete, because we don't care about everything.
#[derive(Deserialize, Serialize)]
struct Panel {
    /// The targets, that's actually the queries to run
    targets: Value,
    /// The title of the panel
    title: String,
    /// The panel description
    description: Option<String>,
    /// The fields config
    #[serde(rename = "fieldConfig")]
    field_config: Value,
    /// Interval set in grafana
    interval: Option<String>,
    /// Type of the graph
    #[serde(rename = "type")]
    graph_type: String,
}

/// The inner dashboard struct from grafana, but incomplete, because we don't care about everything.
#[derive(Deserialize, Serialize)]
struct InnerDashboard {
    /// Each dashboard can have several panels
    panels: Vec<Panel>,
    /// The time range is for the dashboard
    time: Time,
    /// The title of the dashboard
    title: String,
}

/// Dashboard struct from grafana, but incomplete, because we don't care about everything.
#[derive(Deserialize, Serialize)]
pub struct Dashboard {
    /// The dashboard result dashboard as one of the keys
    dashboard: InnerDashboard,
}

impl Dashboard {
    /// Get the title of the dashboard
    pub fn title(&self) -> String {
        self.dashboard.title.clone()
    }
}

/// Query struct for grafana.
#[derive(Debug, Serialize, Deserialize)]
pub struct Query {
    /// The queries to run (left as a json that was returned from grafana)
    queries: Value,
    /// Time range to query, in the format like "now-1w"
    from: String,
    /// Time range to query, usually it's just "now"
    to: String,
}

/// Convert a `Dashboard` into a vector of queries
impl From<Dashboard> for Vec<PanelResponse> {
    fn from(dashboard: Dashboard) -> Self {
        let Time { from, to } = dashboard.dashboard.time;
        dashboard
            .dashboard
            .panels
            .into_iter()
            .map(|panel| PanelResponse {
                title: panel.title,
                description: panel.description,
                field_config: panel.field_config,
                interval: panel.interval,
                graph_type: panel.graph_type,
                query: Query {
                    queries: panel.targets,
                    from: from.clone(),
                    to: to.clone(),
                },
            })
            .collect()
    }
}

/// A client for interacting with Grafana
///
/// This struct provides methods for querying data from Grafana dashboards.
#[derive(Clone)]
pub struct Grafana {
    /// The URL of the Grafana instance
    url: String,
    /// The API token to use for authentication
    api_token: String,
    /// The tag to use to filter dashboards
    dashboard_tag: String,
    /// The reqwest client to use for requests
    client: Client,
}

impl Grafana {
    /// Create a new Grafana client
    ///
    /// # Arguments
    ///
    /// * `url` - The URL of the Grafana instance
    /// * `api_token` - The API token to use for authentication
    /// * `dashboard_tag` - The tag to use to filter dashboards
    #[must_use]
    pub fn new(url: String, api_token: String, dashboard_tag: String) -> Self {
        Self {
            url,
            api_token,
            dashboard_tag,
            client: Client::new(),
        }
    }

    fn prepare_request(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        builder
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", self.api_token))
            .header("Content-Type", "application/json")
    }
    /// Get the UIDs of all dashboards with the specified tag
    ///
    /// # Returns
    ///
    /// A vector of dashboard UIDs
    #[instrument(level = "info", skip_all)]
    pub async fn get_dashboard_uids(&self) -> Result<Vec<String>, GrafanaError> {
        let request_url = format!("{}/api/search?tag={}", self.url, self.dashboard_tag);
        let response = self
            .prepare_request(self.client.get(&request_url))
            .send()
            .await?;

        let dashboards: Vec<DashboardList> = Self::handle_response(response).await?;
        Ok(dashboards
            .into_iter()
            .map(|dashboard| dashboard.uid)
            .collect())
    }

    /// Get a dashboard by its UID
    ///
    /// # Arguments
    ///
    /// * `dashboard_uid` - The UID of the dashboard to get
    ///
    /// # Returns
    ///
    /// The dashboard with the specified UID
    #[instrument(level = "info", skip(self))]
    pub async fn get_dashboard(&self, dashboard_uid: String) -> Result<Dashboard, GrafanaError> {
        let request_url = format!("{}/api/dashboards/uid/{}", self.url, dashboard_uid);
        let response = self
            .prepare_request(self.client.get(&request_url))
            .send()
            .await?;

        Self::handle_response(response).await
    }

    /// Query data from a Grafana dashboard
    ///
    /// # Arguments
    ///
    /// * `query` - The query to get the data
    ///
    /// # Returns
    ///
    /// The data for the query
    #[instrument(level = "info", skip_all)]
    pub async fn get_query_data(&self, query: Query) -> Result<Value, GrafanaError> {
        let request_url = format!("{}/api/ds/query", self.url);

        let response = self
            .prepare_request(self.client.post(&request_url))
            .json(&query)
            .send()
            .await?;

        Self::handle_response(response).await
    }

    async fn handle_response<T>(response: Response) -> Result<T, GrafanaError>
    where
        T: serde::de::DeserializeOwned,
    {
        match response.status() {
            StatusCode::OK => Ok(response.json::<T>().await?),
            StatusCode::UNAUTHORIZED => Err(GrafanaError::Unauthorized),
            StatusCode::FORBIDDEN => Err(GrafanaError::Forbidden),
            StatusCode::NOT_FOUND => Err(GrafanaError::NotFound),
            _ => Err(GrafanaError::FailedRequest(
                response.error_for_status().unwrap_err(),
            )),
        }
    }
}

#[derive(Debug, Error)]
pub enum GrafanaError {
    #[error("Request failed: {0}")]
    FailedRequest(#[from] reqwest::Error),
    #[error("Authentication failed")]
    Unauthorized,
    #[error("Access forbidden")]
    Forbidden,
    #[error("Dashboard not found")]
    NotFound,
}
