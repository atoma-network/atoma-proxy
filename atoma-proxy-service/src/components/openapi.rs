use axum::Router;
use utoipa::{
    openapi::security::{Http, HttpAuthScheme, SecurityScheme},
    Modify, OpenApi,
};
use utoipa_swagger_ui::SwaggerUi;

use crate::{
    handlers::{
        auth::{
            GenerateApiTokenOpenApi, GetAllApiTokensOpenApi, GetBalance, GetSuiAddress,
            LoginOpenApi, RegisterOpenApi, RevokeApiTokenOpenApi, UpdateSuiAddress, UsdcPayment,
            GENERATE_API_TOKEN_PATH, GET_ALL_API_TOKENS_PATH, GET_BALANCE_PATH,
            GET_SUI_ADDRESS_PATH, LOGIN_PATH, REGISTER_PATH, REVOKE_API_TOKEN_PATH,
            UPDATE_SUI_ADDRESS_PATH, USDC_PAYMENT_PATH,
        },
        stacks::{
            GetCurrentStacksOpenApi, GetStacksByUserId, GET_ALL_STACKS_FOR_USER_PATH,
            GET_CURRENT_STACKS_PATH,
        },
        stats::{
            GetComputeUnitsProcessed, GetLatency, GetNodeDistribution, GetStatsStacks,
            COMPUTE_UNITS_PROCESSED_PATH, GET_NODES_DISTRIBUTION_PATH, GET_STATS_STACKS_PATH,
            LATENCY_PATH,
        },
        subscriptions::{GetAllSubscriptionsOpenApi, SUBSCRIPTIONS_PATH},
        tasks::{GetAllTasksOpenApi, TASKS_PATH},
    },
    HealthOpenApi, HEALTH_PATH,
};

pub fn openapi_router() -> Router {
    #[derive(OpenApi)]
    #[openapi(
        modifiers(&SecurityAddon),
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
            (path = TASKS_PATH, api = GetAllTasksOpenApi, tags = ["Tasks"]),
            (path = COMPUTE_UNITS_PROCESSED_PATH, api = GetComputeUnitsProcessed, tags = ["Stats"]),
            (path = LATENCY_PATH, api = GetLatency, tags = ["Stats"]),
            (path = GET_STATS_STACKS_PATH, api = GetStatsStacks, tags = ["Stats"]),
            (path = SUBSCRIPTIONS_PATH, api = GetAllSubscriptionsOpenApi, tags = ["Subscriptions"]),
            (path = GET_NODES_DISTRIBUTION_PATH, api = GetNodeDistribution, tags = ["Stats"]),
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

    struct SecurityAddon;

    impl Modify for SecurityAddon {
        fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
            if let Some(components) = openapi.components.as_mut() {
                components.add_security_scheme(
                    "bearerAuth",
                    SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
                )
            }
        }
    }

    // Generate the OpenAPI spec and write it to a file in debug mode
    #[cfg(debug_assertions)]
    {
        use std::fs;
        use std::path::Path;

        let spec =
            serde_yaml::to_string(&ApiDoc::openapi()).expect("Failed to serialize OpenAPI spec");

        let docs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
        fs::create_dir_all(&docs_dir).expect("Failed to create docs directory");

        let spec_path = docs_dir.join("openapi.yml");
        fs::write(&spec_path, spec).expect("Failed to write OpenAPI spec to file");

        println!("OpenAPI spec written to: {:?}", spec_path);
    }

    Router::new()
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
}
