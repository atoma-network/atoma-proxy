# Atoma Proxy infrastructure

<img src="https://github.com/atoma-network/atoma-node/blob/update-read-me/atoma-assets/atoma-pfp.jpg" alt="Logo" height="500"/>

[![Discord](https://img.shields.io/discord/1172593757586214964?label=Discord&logo=discord&logoColor=white)]
[![Twitter](https://img.shields.io/twitter/follow/Atoma_Network?style=social)](https://x.com/Atoma_Network)
[![Documentation](https://img.shields.io/badge/docs-gitbook-blue)](https://atoma.gitbook.io/atoma-docs)
[![License](https://img.shields.io/github/license/atoma-network/atoma-node)](LICENSE)

## Introduction

Atoma Proxy is a critical component of the Atoma Network that enables:

- **Load Balancing**: Efficient distribution of AI workloads across the network's compute nodes
- **Request Routing**: Intelligent routing of inference requests to the most suitable nodes based on model availability, load, and performance
- **High Availability**: Ensuring continuous service through redundancy and failover mechanisms
- **Network Optimization**: Minimizing latency and maximizing throughput for AI inference requests

This repository contains the proxy infrastructure that helps coordinate and optimize the Atoma Network's distributed compute resources. By deploying an Atoma proxy, you can:

1. Help manage and distribute AI workloads efficiently across the network;
2. Contribute to the network's reliability and performance;
3. Support the development of a more resilient and scalable AI infrastructure.

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

2. Configure environment variables by creating `.env` file, use `.env.example` for reference:

```bash
POSTGRES_DB=<YOUR_DB_NAME>
POSTGRES_USER=<YOUR_DB_USER>
POSTGRES_PASSWORD=<YOUR_DB_PASSWORD>

TRACE_LEVEL=info
```

3. Configure `config.toml`, using `config.example.toml` as template:

```toml
[atoma_sui]
http_rpc_node_addr = "https://fullnode.testnet.sui.io:443"                              # Current RPC node address for testnet
atoma_db = "0x741693fc00dd8a46b6509c0c3dc6a095f325b8766e96f01ba73b668df218f859"         # Current ATOMA DB object ID for testnet
atoma_package_id = "0x0c4a52c2c74f9361deb1a1b8496698c7e25847f7ad9abfbd6f8c511e508c62a0" # Current ATOMA package ID for testnet
usdc_package_id = "0xe0b1d6458f349d4bc71ef119694866f1d6ee6915b43f8cc05a5d44a49e3e1f0f"  # Current USDC package ID for testnet
request_timeout = { secs = 300, nanos = 0 }                                             # Some reference value
max_concurrent_requests = 10                                                            # Some reference value
limit = 100                                                                             # Some reference value
sui_config_path = "~/.sui/sui_config/client.yaml"                                       # Path to the Sui client configuration file, by default (on Linux, or MacOS)
sui_keystore_path = "~/.sui/sui_config/sui.keystore"                                    # Path to the Sui keystore file, by default (on Linux, or MacOS)
cursor_path = "./cursor.toml"

[atoma_state]
# URL of the PostgreSQL database, it SHOULD be the same as the `ATOMA_STATE_DATABASE_URL` variable value in the .env file
database_url = "postgresql://POSTGRES_USER:POSTGRES_PASSWORD@db:5432/POSTGRES_DB"

[atoma_service]
service_bind_address = "0.0.0.0:8080" # Address to bind the service to
models = [
  "meta-llama/Llama-3.2-3B-Instruct",
  "meta-llama/Llama-3.2-1B-Instruct",
] # Models supported by proxy
revisions = ["main", "main"] # Revision of the above models
hf_token = "<YOUR_HF_TOKEN>" # Hugging face api token, required if you want to access a gated model
```

4. Create required directories

```bash
mkdir -p data logs
```

5. Start the containers with the desired inference services

```bash
# Build and start all services
docker compose up --build

# Or run in detached mode
docker compose up -d --build
```

#### Container Architecture

The deployment consists of two main services:

- **PostgreSQL**: Manages the database for the Atoma Proxy
- **Atoma Proxy**: Manages the proxy operations and connects to the Atoma Network

#### Service URLs

- Atoma Proxy: `http://localhost:8080` (configured via ATOMA_PROXY_PORT)

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
docker compose logs atoma-proxy

# Follow logs
docker compose logs -f
```

Stop services:

```bash
docker compose down
```

#### Troubleshooting

1. Check if services are running:

```bash
docker compose ps
```

2. Test Atoma Proxy service:

```bash
curl http://localhost:8080/health
```

5. View container networks:

```bash
docker network ls
docker network inspect atoma-network
```

#### Security Considerations

1. Firewall Configuration

```bash
# Allow Atoma Proxy port
sudo ufw allow 8080/tcp
```

2. HuggingFace Token

- Store HF_TOKEN in .env file
- Never commit .env file to version control
- Consider using Docker secrets for production deployments

3. Sui Configuration

- Ensure Sui configuration files have appropriate permissions
- Keep keystore file secure and never commit to version control

### Traefik Configuration

The deployment includes Traefik as a reverse proxy to handle HTTPS traffic.

#### SSL/TLS Certificates

Traefik automatically handles SSL/TLS certificates through Let's Encrypt. To configure:

1. Set your domain in .env:

PROXY_DOMAIN=api.atomacloud.com
ACME_EMAIL=email@example.com
