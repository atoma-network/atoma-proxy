use axum::Router;
use serde_json::json;
use utoipa::{
    openapi::security::{Http, HttpAuthScheme, SecurityScheme},
    Modify, OpenApi,
};
use utoipa_swagger_ui::SwaggerUi;

use crate::server::handlers::embeddings::{
    ConfidentialEmbeddingsOpenApi, CONFIDENTIAL_EMBEDDINGS_PATH,
};
use crate::server::handlers::image_generations::{
    ConfidentialImageGenerationsOpenApi, CONFIDENTIAL_IMAGE_GENERATIONS_PATH,
};
use crate::server::handlers::{
    chat_completions::ChatCompletionsOpenApi,
    chat_completions::CHAT_COMPLETIONS_PATH,
    embeddings::EmbeddingsOpenApi,
    embeddings::EMBEDDINGS_PATH,
    image_generations::ImageGenerationsOpenApi,
    image_generations::IMAGE_GENERATIONS_PATH,
    models::{ModelsOpenApi, MODELS_PATH},
    nodes::NodesOpenApi,
};
use crate::server::handlers::{
    chat_completions::{ConfidentialChatCompletionsOpenApi, CONFIDENTIAL_CHAT_COMPLETIONS_PATH},
    nodes::NODES_PATH,
};
use crate::server::http_server::{HealthOpenApi, HEALTH_PATH};

pub fn openapi_routes() -> Router {
    #[derive(OpenApi)]
    #[openapi(
        modifiers(&SpeakeasyExtension, &SecurityAddon),
        nest(
            (path = CHAT_COMPLETIONS_PATH, api = ChatCompletionsOpenApi, tags = ["Chat"]),
            (path = CONFIDENTIAL_CHAT_COMPLETIONS_PATH, api = ConfidentialChatCompletionsOpenApi, tags = ["Confidential Chat"]),
            (path = CONFIDENTIAL_EMBEDDINGS_PATH, api = ConfidentialEmbeddingsOpenApi, tags = ["Confidential Embeddings"]),
            (path = CONFIDENTIAL_IMAGE_GENERATIONS_PATH, api = ConfidentialImageGenerationsOpenApi, tags = ["Confidential Images"]),
            (path = EMBEDDINGS_PATH, api = EmbeddingsOpenApi, tags = ["Embeddings"]),
            (path = HEALTH_PATH, api = HealthOpenApi, tags = ["Health"]),
            (path = IMAGE_GENERATIONS_PATH, api = ImageGenerationsOpenApi, tags = ["Images"]), 
            (path = MODELS_PATH, api = ModelsOpenApi, tags = ["Models"]),
            (path = NODES_PATH, api = NodesOpenApi, tags = ["Nodes"]),
        ),
        tags(
            (name = "Chat", description = "OpenAI's API chat completions v1 endpoint"),
            (name = "Confidential Chat", description = "Atoma's API confidential chat completions v1 endpoint"),
            (name = "Confidential Embeddings", description = "Atoma's API confidential embeddings v1 endpoint"),
            (name = "Confidential Images", description = "Atoma's API confidential images v1 endpoint"),
            (name = "Embeddings", description = "OpenAI's API embeddings v1 endpoint"),
            (name = "Health", description = "Health check"),
            (name = "Images", description = "OpenAI's API images v1 endpoint"),
            (name = "Models", description = "OpenAI's API models v1 endpoint"),
            (name = "Nodes", description = "Nodes Management"),
            (name = "Node Public Key Selection", description = "Node public key selection")
        ),
        servers(
            (url = "https://api.atomacloud.com"),
        )
    )]
    struct ApiDoc;

    #[derive(Default)]
    struct SpeakeasyExtension;

    impl Modify for SpeakeasyExtension {
        fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
            // Create new Extensions if none exist
            let extensions = openapi.extensions.get_or_insert_with(Default::default);

            // Add the x-speakeasy-name-override
            extensions.insert(
                "x-speakeasy-name-override".to_string(),
                json!([
                    {
                        "operationId": "chat_completions_create",
                        "methodNameOverride": "create"
                    },
                    {
                        "operationId": "chat_completions_create_stream",
                        "methodNameOverride": "create_stream"
                    },
                    {
                        "operationId": "confidential_chat_completions_create",
                        "methodNameOverride": "create"
                    },
                    {
                        "operationId": "embeddings_create",
                        "methodNameOverride": "create"
                    },
                    {
                        "operationId": "confidential_embeddings_create",
                        "methodNameOverride": "create"
                    },
                    {
                        "operationId": "image_generations_create",
                        "methodNameOverride": "generate"
                    },
                    {
                        "operationId": "confidential_image_generations_create",
                        "methodNameOverride": "generate"
                    }
                ]),
            );
        }
    }

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

    // Generate the OpenAPI spec and write it to a file
    #[cfg(debug_assertions)]
    {
        use std::fs;
        use std::path::Path;

        // Generate OpenAPI spec
        let spec =
            serde_yaml::to_string(&ApiDoc::openapi()).expect("Failed to serialize OpenAPI spec");

        // Ensure the docs directory exists
        let docs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
        fs::create_dir_all(&docs_dir).expect("Failed to create docs directory");

        // Write the spec to the file
        let spec_path = docs_dir.join("openapi.yml");
        fs::write(&spec_path, spec).expect("Failed to write OpenAPI spec to file");

        println!("OpenAPI spec written to: {:?}", spec_path);
    }

    Router::new()
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
}
