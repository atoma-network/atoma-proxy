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
  "max_tokens": 4096,
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

1. Configure environment variables by creating `.env` file. Please ensure you have created the requisite user and database in your postgres instance. Once you've done that, you can use `.env.example` as template for your `.env` file.

```bash
POSTGRES_DB=<YOUR_DB_NAME>
POSTGRES_USER=<YOUR_DB_USER>
POSTGRES_PASSWORD=<YOUR_DB_PASSWORD>

TRACE_LEVEL=info
```

1. Configure `config.toml`, using `config.example.toml` as template:

```toml
[atoma_sui]
http_rpc_node_addr = "https://fullnode.testnet.sui.io:443"                              # Current RPC node address for testnet
atoma_db = "0x741693fc00dd8a46b6509c0c3dc6a095f325b8766e96f01ba73b668df218f859"         # Current ATOMA DB object ID for testnet
atoma_package_id = "0x0c4a52c2c74f9361deb1a1b8496698c7e25847f7ad9abfbd6f8c511e508c62a0" # Current ATOMA package ID for testnet
usdc_package_id = "0xa1ec7fc00a6f40db9693ad1415d0c193ad3906494428cf252621037bd7117e29"  # Current USDC package ID for testnet
request_timeout = { secs = 300, nanos = 0 }                                             # Some reference value
max_concurrent_requests = 10                                                            # Some reference value
limit = 100                                                                             # Some reference value
sui_config_path = "/root/.sui/sui_config/client.yaml"                                       # Path to the Sui client configuration file, by default (on Linux, or MacOS)
sui_keystore_path = "/root/.sui/sui_config/sui.keystore"                                    # Path to the Sui keystore file, by default (on Linux, or MacOS)
cursor_path = "./cursor.toml"

[atoma_state]
# URL of the PostgreSQL database, the <POSTGRES_USER>, <POSTGRES_PASSWORD> and <POSTGRES_DB> variables values should be the exactly same as in the .env file
database_url = "postgresql://<POSTGRES_USER>:<POSTGRES_PASSWORD>@db:5432/<POSTGRES_DB>"

[atoma_service]
service_bind_address = "0.0.0.0:8080" # Address to bind the service to
password = "password" # Password for the service
models = ["<MODEL_NAME>"] # Models supported by proxy (e.g. "meta-llama/Llama-3.3-70B-Instruct")
revisions = ["<REVISION_NAME>"] # Revision of the above models (e.g. "main")
hf_token = "<API_KEY>" # Hugging face api token, required if you want to access a gated model

[atoma_proxy_service]
service_bind_address = "0.0.0.0:8081"

[atoma_auth]
secret_key = "secret_key" # Secret key for the tokens generation
access_token_lifetime = 1 # In minutes
refresh_token_lifetime = 1 # In days
google_client_id="" # Google client id for google login (In case google-oauth feature is enabled)
```

1. Create required directories

```bash
mkdir -p data logs
```

1. Start the containers with the desired inference services

```bash
# Build and start all services
docker compose --profile local up --build

# Or run in detached mode
docker compose --profile local up -d --build
```

#### Container Architecture

The deployment consists of two main services:

- **PostgreSQL**: Manages the database for the Atoma Proxy
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
```

1. HuggingFace Token

- Store HF_TOKEN in .env file
- Never commit .env file to version control
- Consider using Docker secrets for production deployments

1. Sui Configuration

- Ensure Sui configuration files have appropriate permissions
- Keep keystore file secure and never commit to version control
