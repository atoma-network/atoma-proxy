use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tracing::instrument;

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
}

#[derive(Deserialize)]
struct DashboardList {
    uid: String,
}

#[derive(Deserialize)]
struct Time {
    from: String,
    to: String,
}

#[derive(Deserialize)]
struct Panel {
    targets: Value,
    title: String,
}

#[derive(Deserialize)]
struct InnerDashboard {
    panels: Vec<Panel>,
    time: Time,
    title: String,
}

#[derive(Deserialize)]
pub struct Dashboard {
    dashboard: InnerDashboard,
}

impl Dashboard {
    pub fn title(&self) -> String {
        self.dashboard.title.clone()
    }
}

#[derive(Debug, Serialize)]
struct Query {
    queries: Value,
    from: String,
    to: String,
}

/// Convert a `Dashboard` into a vector of queries
impl From<Dashboard> for Vec<(String, Query)> {
    fn from(dashboard: Dashboard) -> Self {
        let from = dashboard.dashboard.time.from;
        let to = dashboard.dashboard.time.to;
        dashboard
            .dashboard
            .panels
            .into_iter()
            .map(|panel| {
                (
                    panel.title,
                    Query {
                        queries: panel.targets,
                        from: from.clone(),
                        to: to.clone(),
                    },
                )
            })
            .collect()
    }
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
    pub const fn new(url: String, api_token: String, dashboard_tag: String) -> Self {
        Self {
            url,
            api_token,
            dashboard_tag,
        }
    }

    /// Get the UIDs of all dashboards with the specified tag
    ///
    /// # Returns
    ///
    /// A vector of dashboard UIDs
    #[instrument(level = "info", skip_all)]
    pub async fn get_dashboard_uids(&self) -> Result<Vec<String>, GrafanaError> {
        let client = Client::new();
        let request_url = format!("{}/api/search?tag={}", self.url, self.dashboard_tag);
        let response = client
            .get(&request_url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.api_token))
            .send()
            .await?;

        if response.status().is_success() {
            let dashboards = response.json::<Vec<DashboardList>>().await?;
            Ok(dashboards
                .into_iter()
                .map(|dashboard| dashboard.uid)
                .collect())
        } else {
            Err(GrafanaError::FailedRequest(
                response.error_for_status().unwrap_err(),
            ))
        }
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
        let client = Client::new();
        let request_url = format!("{}/api/dashboards/uid/{}", self.url, dashboard_uid);
        let response = client
            .get(&request_url)
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", self.api_token))
            .send()
            .await?;

        if response.status().is_success() {
            let dashboard = response.json::<Dashboard>().await?;
            Ok(dashboard)
        } else {
            Err(GrafanaError::FailedRequest(
                response.error_for_status().unwrap_err(),
            ))
        }
    }

    /// Query data from a Grafana dashboard
    ///
    /// # Arguments
    ///
    /// * `dashboard` - The dashboard to query data from
    ///
    /// # Returns
    ///
    /// A vector of tuples containing the title of the panel and the data
    #[instrument(level = "info", skip_all)]
    pub async fn query_data(
        &self,
        dashboard: Dashboard,
    ) -> Result<Vec<(String, Value)>, GrafanaError> {
        let client = Client::new();
        let request_url = format!("{}/api/ds/query", self.url);
        let queries: Vec<(String, Query)> = dashboard.into();
        let mut results = Vec::new();
        for (title, query) in queries {
            let response = client
                .post(&request_url)
                .header("Accept", "application/json")
                .header("Authorization", format!("Bearer {}", self.api_token))
                .header("Content-Type", "application/json")
                .json(&query)
                .send()
                .await?;

            if response.status().is_success() {
                let dashboard = response.json::<Value>().await?;
                results.push((title, dashboard));
            } else {
                return Err(GrafanaError::FailedRequest(
                    response.error_for_status().unwrap_err(),
                ));
            }
        }
        Ok(results)
    }
}

#[derive(Debug, Error)]
pub enum GrafanaError {
    #[error("Request failed: {0}")]
    FailedRequest(#[from] reqwest::Error),
}
