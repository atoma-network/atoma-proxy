use axum::Router;
use serde_json::json;
use utoipa::{
    openapi::security::{Http, HttpAuthScheme, SecurityScheme},
    Modify, OpenApi,
};
use utoipa_swagger_ui::SwaggerUi;

#[cfg(feature = "google-oauth")]
use crate::handlers::auth::{GoogleOAuth, GOOGLE_OAUTH_PATH};
use crate::{
    handlers::{
        auth::{
            GenerateApiTokenOpenApi, GetAllApiTokensOpenApi, GetBalance, GetSuiAddress,
            GetUserProfile, GetZkSalt, LoginOpenApi, RegisterOpenApi, RevokeApiTokenOpenApi,
            UpdateSuiAddress, UsdcPayment, GENERATE_API_TOKEN_PATH, GET_ALL_API_TOKENS_PATH,
            GET_BALANCE_PATH, GET_SUI_ADDRESS_PATH, GET_USER_PROFILE_PATH, GET_ZK_SALT_PATH,
            LOGIN_PATH, REGISTER_PATH, REVOKE_API_TOKEN_PATH, UPDATE_SUI_ADDRESS_PATH,
            USDC_PAYMENT_PATH,
        },
        stacks::{
            GetCurrentStacksOpenApi, GetStacksByUserId, GET_ALL_STACKS_FOR_USER_PATH,
            GET_CURRENT_STACKS_PATH,
        },
        stats::{
            GetComputeUnitsProcessed, GetGraphData, GetGraphs, GetLatency, GetNodeDistribution,
            GetStatsStacks, COMPUTE_UNITS_PROCESSED_PATH, GET_GRAPHS_PATH, GET_GRAPH_DATA_PATH,
            GET_NODES_DISTRIBUTION_PATH, GET_STATS_STACKS_PATH, LATENCY_PATH,
        },
        subscriptions::{GetAllSubscriptionsOpenApi, SUBSCRIPTIONS_PATH},
        tasks::{GetAllTasksOpenApi, TASKS_PATH},
    },
    HealthOpenApi, HEALTH_PATH,
};

#[allow(clippy::too_many_lines)]
pub fn openapi_router() -> Router {
    #[derive(OpenApi)]
    #[openapi(
        modifiers(&SecurityAddon, &SpeakeasyExtension),
        nest(
            (path = HEALTH_PATH, api = HealthOpenApi, tags = ["Health"]),
            (path = GENERATE_API_TOKEN_PATH, api = GenerateApiTokenOpenApi, tags = ["Auth"]),
            (path = REVOKE_API_TOKEN_PATH, api = RevokeApiTokenOpenApi, tags = ["Auth"]),
            (path = REGISTER_PATH, api = RegisterOpenApi, tags = ["Auth"]),
            (path = LOGIN_PATH, api = LoginOpenApi, tags = ["Auth"]),
            (path = GET_ALL_API_TOKENS_PATH, api = GetAllApiTokensOpenApi, tags = ["Auth"]),
            (path = UPDATE_SUI_ADDRESS_PATH, api = UpdateSuiAddress, tags = ["Auth"]),
            (path = USDC_PAYMENT_PATH, api = UsdcPayment, tags = ["Auth"]),
            (path = GET_SUI_ADDRESS_PATH, api = GetSuiAddress, tags = ["Auth"]),
            (path = GET_CURRENT_STACKS_PATH, api = GetCurrentStacksOpenApi, tags = ["Stacks"]),
            (path = GET_ALL_STACKS_FOR_USER_PATH, api = GetStacksByUserId, tags = ["Stacks"]),
            (path = GET_BALANCE_PATH, api = GetBalance, tags = ["Auth"]),
            (path = GET_USER_PROFILE_PATH, api = GetUserProfile, tags = ["Auth"]),
            (path = GET_ZK_SALT_PATH, api = GetZkSalt, tags = ["Auth"]),
            (path = TASKS_PATH, api = GetAllTasksOpenApi, tags = ["Tasks"]),
            (path = COMPUTE_UNITS_PROCESSED_PATH, api = GetComputeUnitsProcessed, tags = ["Stats"]),
            (path = LATENCY_PATH, api = GetLatency, tags = ["Stats"]),
            (path = GET_STATS_STACKS_PATH, api = GetStatsStacks, tags = ["Stats"]),
            (path = SUBSCRIPTIONS_PATH, api = GetAllSubscriptionsOpenApi, tags = ["Subscriptions"]),
            (path = GET_NODES_DISTRIBUTION_PATH, api = GetNodeDistribution, tags = ["Stats"]),
            (path = GET_GRAPHS_PATH, api = GetGraphs, tags = ["Stats"]),
            (path = GET_GRAPH_DATA_PATH, api = GetGraphData, tags = ["Stats"]),
        ),
        tags(
            (name = "Health", description = "Health check endpoints"),
            (name = "Auth", description = "Authentication and API token management"),
            (name = "Tasks", description = "Atoma's Tasks management"),
            (name = "Subscriptions", description = "Node task subscriptions management"),
            (name = "Stacks", description = "Stacks management"),
            (name = "Stats", description = "Stats and metrics"),
        ),
        servers(
            (url = "http://localhost:8081", description = "Local server"),
        )
    )]
    struct ApiDoc;

    #[cfg(feature = "google-oauth")]
    #[derive(OpenApi)]
    #[openapi(
        modifiers(&SecurityAddon),
        nest(
            (path = GOOGLE_OAUTH_PATH, api = GoogleOAuth, tags = ["Auth"]),
        ),
        tags(
            (name = "Auth", description = "Authentication and API token management"),
        ),
        servers(
            (url = "http://localhost:8081", description = "Local server"),
        )
    )]
    struct GoogleOAuthApiDoc;

    struct SecurityAddon;

    #[derive(Default)]
    struct SpeakeasyExtension;

    impl Modify for SecurityAddon {
        fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
            if let Some(components) = openapi.components.as_mut() {
                components.add_security_scheme(
                    "bearerAuth",
                    SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
                );
            }
        }
    }

    impl Modify for SpeakeasyExtension {
        fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
            let extensions = openapi.extensions.get_or_insert_with(Default::default);

            extensions.insert(
                "x-speakeasy-retries".to_string(),
                json!({
                    "strategy": "backoff",
                    "backoff": {
                        "initialInterval": 500,
                        "maxInterval": 60000,
                        "maxElapsedTime": 3_600_000,
                        "exponent": 1.5
                    },
                    "statusCodes": ["5XX"],
                    "retryConnectionErrors": true
                }),
            );
        }
    }

    let openapi = ApiDoc::openapi();
    #[cfg(feature = "google-oauth")]
    let openapi = openapi.merge_from(GoogleOAuthApiDoc::openapi());

    // Generate the OpenAPI spec and write it to a file in debug mode
    #[cfg(debug_assertions)]
    {
        use std::fs;
        use std::path::Path;

        let spec = serde_yaml::to_string(&openapi).expect("Failed to serialize OpenAPI spec");

        let docs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
        fs::create_dir_all(&docs_dir).expect("Failed to create docs directory");

        let spec_path = docs_dir.join("openapi.yml");
        fs::write(&spec_path, spec).expect("Failed to write OpenAPI spec to file");

        println!("OpenAPI spec written to: {spec_path:?}");
    }

    Router::new().merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", openapi))
}
