# Atoma Proxy Infrastructure

![Atoma Banner](https://github.com/atoma-network/atoma-node/blob/main/atoma-assets/atoma-banner.png)

[![Discord](https://img.shields.io/discord/1172593757586214964?label=Discord&logo=discord&logoColor=white)](https://discord.com/channels/1172593757586214964/1258484557083054081)
[![Twitter](https://img.shields.io/twitter/follow/Atoma_Network?style=social)](https://x.com/Atoma_Network)
[![Documentation](https://img.shields.io/badge/docs-mintify-blue?logo=mintify)](https://docs.atoma.network)
[![License](https://img.shields.io/github/license/atoma-network/atoma-node)](LICENSE)

## Introduction

Atoma Proxy is a critical component of the Atoma Network that enables:

- **Load Balancing**: Efficient distribution of AI workloads across the network's compute nodes
- **Request Routing**: Intelligent routing of inference requests to the most suitable nodes based on model availability, load, and performance
- **High Availability**: Ensuring continuous service through redundancy and failover mechanisms
- **Network Optimization**: Minimizing latency and maximizing throughput for AI inference requests
- **Security**: Secure authentication and authorization of API requests

This repository contains the proxy infrastructure that helps coordinate and optimize the Atoma Network's distributed compute resources. By deploying an Atoma proxy, you can:

1. Help manage and distribute AI workloads efficiently across the network;
1. Contribute to the network's reliability and performance;
1. Support the development of a more resilient and scalable AI infrastructure.

Currently, the Atoma Proxy is powering Atoma's cloud web service, available at [cloud.atoma.network](https://cloud.atoma.network). By registering an account, you can obtain an API key and start using Atoma's AI services. For example, to request a chat completions from a `meta-llama/Llama-3.3-70B-Instruct` model, you can use the following request:

```bash
curl -X POST https://api.atoma.network/v1/chat/completions \
-H "Authorization: Bearer YOUR_API_KEY" \
-H "Content-Type: application/json" \
-d '{
  "model": "meta-llama/Llama-3.3-70B-Instruct",
  "messages": [{"role": "user", "content": "Tell me a joke"}],
  "max_completion_tokens": 4096,
  "stream": true
}'
```

You can further deploy your own Atoma Proxy locally to power your own AI services. Please refer to the [Deployment Guide](#deploying-an-atoma-proxy) section for more information.

### Community Links

- 🌐 [Official Website](https://www.atoma.network)
- 📖 [Documentation](https://atoma.gitbook.io/atoma-docs)
- 🐦 [Twitter](https://x.com/Atoma_Network)
- 💬 [Discord](https://discord.com/channels/1172593757586214964/1258484557083054081)

## Deploying an Atoma Proxy

### Install the Sui client locally

The first step in setting up an Atoma node is installing the Sui client locally. Please refer to the [Sui installation guide](https://docs.sui.io/build/install) for more information.

Once you have the Sui client installed, locally, you need to connect to a Sui RPC node to be able to interact with the Sui blockchain and therefore the Atoma smart contract. Please refer to the [Connect to a Sui Network guide](https://docs.sui.io/guides/developer/getting-started/connect) for more information.

You then need to create a wallet and fund it with some testnet SUI. Please refer to the [Sui wallet guide](https://docs.sui.io/guides/developer/getting-started/get-address) for more information. If you are plan to run the Atoma node on Sui's testnet, you can request testnet SUI tokens by following the [docs](https://docs.sui.io/guides/developer/getting-started/get-coins).

### Register with the Atoma Testnet smart contract

Please refer to the [setup script](https://github.com/atoma-network/atoma-contracts/blob/main/sui/dev/setup.py) to register with the Atoma Testnet smart contract. This will assign you a node badge and a package ID, which you'll need to configure in the `config.toml` file.

### Docker Deployment

#### Prerequisites

- Docker and Docker Compose installed
- Sui wallet configuration

#### Quickstart

1. Clone the repository

```bash
git clone https://github.com/atoma-network/atoma-proxy.git
cd atoma-proxy
```

2. Configure environment variables by creating `.env` file. Please ensure you have created the requisite user and database in your postgres instance. Once you've done that, you can use `.env.example` as template for your `.env` file.

```bash
cp .env.example .env
```

Please ensure you have created the requisite user and database in your postgres instance, as

```bash
POSTGRES_DB=<YOUR_DB_NAME>
POSTGRES_USER=<YOUR_DB_USER>
POSTGRES_PASSWORD=<YOUR_DB_PASSWORD>

TRACE_LEVEL=info
```

3. Configure `config.toml`, using `config.example.toml` as template.

## Configuration Reference

### Sui Configuration (`[atoma_sui]`)
| Parameter | Description | Default |
|-----------|-------------|---------|
| `http_rpc_node_addr` | Sui RPC node endpoint for testnet network | `https://fullnode.testnet.sui.io:443` |
| `atoma_db` | ATOMA database object ID on testnet | `0x741693...` |
| `atoma_package_id` | ATOMA smart contract package ID on testnet | `0x0c4a52...` |
| `usdc_package_id` | USDC smart contract package ID on testnet | `0xa1ec7f...` |
| `request_timeout` | Maximum time to wait for RPC requests | `300 seconds` |
| `max_concurrent_requests` | Maximum number of simultaneous RPC requests | `10` |
| `limit` | Maximum number of items per page for paginated responses | `100` |
| `sui_config_path` | Path to Sui client configuration file | `/root/.sui/sui_config/client.yaml` |
| `sui_keystore_path` | Path to Sui keystore containing account keys | `/root/.sui/sui_config/sui.keystore` |
| `cursor_path` | Path to store the event cursor state | `./cursor.toml` |

### State Configuration (`[atoma_state]`)
| Parameter | Description | Example |
|-----------|-------------|---------|
| `database_url` | PostgreSQL connection string (must match values in .env) | `postgresql://<POSTGRES_USER>:<POSTGRES_PASSWORD>@db:5432/<POSTGRES_DB>` |

### Service Configuration (`[atoma_service]`)
| Parameter | Description | Example |
|-----------|-------------|---------|
| `service_bind_address` | HTTP service binding address and port | `0.0.0.0:8080` |
| `password` | Authentication password for the service API | `password` |
| `models` | List of supported LLM models | `["meta-llama/Llama-3.3-70B-Instruct"]` |
| `revisions` | Model revision/version tags | `["main"]` |
| `hf_token` | Hugging Face API token for gated/private models | Required |

### Proxy Service Configuration (`[atoma_proxy_service]`)
| Parameter | Description | Default |
|-----------|-------------|---------|
| `service_bind_address` | Proxy service binding address and port | `0.0.0.0:8081` |

### Authentication Configuration (`[atoma_auth]`)
| Parameter | Description | Default |
|-----------|-------------|---------|
| `secret_key` | JWT signing key for token generation | `secret_key` |
| `access_token_lifetime` | Access token validity duration in minutes | `1` |
| `refresh_token_lifetime` | Refresh token validity duration in days | `1` |
| `google_client_id` | Google OAuth client ID (required for google-oauth feature) | `""` |

### Example Configuration

```toml
[atoma_sui]
http_rpc_node_addr = "https://fullnode.testnet.sui.io:443"
atoma_db = "0x741693fc00dd8a46b6509c0c3dc6a095f325b8766e96f01ba73b668df218f859"
atoma_package_id = "0x0c4a52c2c74f9361deb1a1b8496698c7e25847f7ad9abfbd6f8c511e508c62a0"
usdc_package_id = "0xa1ec7fc00a6f40db9693ad1415d0c193ad3906494428cf252621037bd7117e29"
request_timeout = { secs = 300, nanos = 0 }
max_concurrent_requests = 10
limit = 100
sui_config_path = "/root/.sui/sui_config/client.yaml"
sui_keystore_path = "/root/.sui/sui_config/sui.keystore"
cursor_path = "./cursor.toml"

[atoma_state]
database_url = "postgresql://atoma_user:strong_password@db:5432/atoma_db"

[atoma_service]
service_bind_address = "0.0.0.0:8080"
password = "your_strong_service_password"
models = ["meta-llama/Llama-3.3-70B-Instruct"]
revisions = ["main"]
hf_token = "hf_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"

[atoma_proxy_service]
service_bind_address = "0.0.0.0:8081"

[atoma_auth]
secret_key = "your_secure_secret_key"
access_token_lifetime = 60    # 60 minutes
refresh_token_lifetime = 7    # 7 days
google_client_id = "123456789-abcdefghijklmnopqrstuvwxyz.apps.googleusercontent.com"
```

### Important Notes:
- Replace placeholder values (`<POSTGRES_USER>`, `<POSTGRES_PASSWORD>`, `<POSTGRES_DB>`) with actual values from your `.env` file
- Ensure the `hf_token` is valid if using gated models
- Use strong passwords and secret keys in production
- The database URL must match the PostgreSQL configuration in your environment
- Sui paths should match your deployment environment

4. Create required directories

```bash
mkdir -p data logs
```

5. Start the containers with the desired inference services. In order to run a local deployment of the proxy, run

```bash
# Build and start all services
docker compose --profile local up --build

# Or run in detached mode
docker compose --profile local up -d --build
```

If instead, one wishes to deploy the proxy as a cloud service, then run

```bash
# Build and start all services
docker compose --profile cloud up --build

# Or run in detached mode
docker compose --profile cloud up -d --build
```

#### Container Architecture

The deployment consists of two main services:

- **Grafana**: Manages the dashboard UI for the Atoma Proxy
- **PostgreSQL**: Manages the database for the Atoma Proxy
- **Traefik**: Manages the reverse proxy for the Atoma Proxy APIs, and the dashboard UI
- **Loki**: Manages the logging for the Atoma Proxy
- **Prometheus**: Manages the metrics for the Atoma Proxy
- **Open Collector**: Manages the tracing for the Atoma Proxy
- **Tempo**: Manages the tracing for the Atoma Proxy
- **zkLogin backend and frontend**: Manages a zkLogin instance serving for login and registration of users for the Atoma Proxy, using Google OAuth and zkLogin
- **Atoma Proxy**: Manages the proxy operations and connects to the Atoma Network

#### Profiles

- local - this is for targeting the local deployment of the proxy
- cloud - this is when the proxy is being deployed as a service. It has a zklogin (google oauth) feature enabled, which is not available for the local option.

#### Service URLs

- Atoma Proxy: `http://localhost:8080` (configured via ATOMA_PROXY_PORT). This is the main service that you will use to interact with the Atoma Network, via an
  OpenAI-compatible API.
- Atoma Proxy Service: `http://localhost:8081` (configured via ATOMA_SERVICE_PORT). You can use this URL to authenticate locally. If you plan to have a custom
  service (with custom domain), this service allows users to register, authenticate and get an API keys to the Atoma Proxy.

#### Volume Mounts

- Logs: `./logs:/app/logs`
- PostgreSQL database: `./data:/var/lib/postgresql/data`

#### Managing the Deployment

Check service status:

```bash
docker compose ps
```

View logs:

```bash
# All services
docker compose logs

# Specific service
docker compose logs atoma-proxy-cloud # Cloud
docker compose logs atoma-proxy-local # Local

# Follow logs
docker compose logs -f
```

Stop services:

```bash
docker compose --profile cloud down   # Cloud
docker compose --profile local down # Local
```

#### Testing

Since the `AtomaStateManager` instance relies on a PostgreSQL database, we need to have a local docker instance running to run the tests. You can spawn one using the `docker-compose.test.yaml` file:

> **Note**
> Please ensure that you don't have any other postgres instance running on your machine, as this might cause conflicts.

```bash
docker compose -f docker-compose.test.yaml up --build -d
```

It might be necessary that you clean up the database before or after running the tests. You can do so by running:

```bash
docker compose -f docker-compose.test.yaml down
```

and remove the specific postgres volumes:

```bash
docker system prune -af --volumes
```

#### Troubleshooting

1. Check if services are running:

```bash
docker compose ps
```

1. Test Atoma Proxy service:

```bash
curl http://localhost:8080/health
```

1. View container networks:

```bash
docker network ls
docker network inspect <NETWORK_NAME>
```

#### Security Considerations

1. Firewall Configuration

```bash
# Allow Atoma Proxy port
sudo ufw allow 8080/tcp

# Allow Atoma Proxy P2P port
sudo ufw allow 8083/tcp
```

1. HuggingFace Token

- Store HF_TOKEN in .env file
- Never commit .env file to version control
- Consider using Docker secrets for production deployments

1. Sui Configuration

- Ensure Sui configuration files have appropriate permissions
- Keep keystore file secure and never commit to version control
