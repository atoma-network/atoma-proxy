# Atoma Proxy Helm Chart

This Helm chart deploys the Atoma Proxy application along with its dependencies, including PostgreSQL, Prometheus, Grafana, Loki, and Tempo.

## Prerequisites

- Kubernetes cluster (v1.19+)
- Helm 3.x
- cert-manager (for SSL certificates)
- nginx-ingress-controller
- kubectl configured to communicate with your cluster

## Quick Start

1. Add the required Helm repositories:
```bash
helm repo add bitnami https://charts.bitnami.com/bitnami
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
helm repo add grafana https://grafana.github.io/helm-charts
helm repo update
```

2. Install the chart:
```bash
# For development
./scripts/deploy.sh -e dev -p your-password

# For production
./scripts/deploy.sh -e prod -p your-secure-password
```

## Configuration

The chart can be configured using values files or command-line arguments. The main configuration files are:

- `values.yaml`: Default configuration
- `values-dev.yaml`: Development environment configuration
- `values-prod.yaml`: Production environment configuration

### Key Configuration Parameters

#### Main Application
```yaml
atomaProxy:
  image:
    repository: ghcr.io/atoma-network/atoma-proxy
    tag: latest
  replicas: 1
  resources:
    requests:
      memory: "512Mi"
      cpu: "250m"
    limits:
      memory: "1Gi"
      cpu: "500m"
```

#### PostgreSQL
```yaml
postgresql:
  enabled: true
  auth:
    database: atoma_proxy
    username: atoma_proxy
    password: ""  # Set via --set or secrets management
  primary:
    persistence:
      size: 10Gi
```

#### Monitoring Stack
```yaml
prometheus:
  enabled: true
  server:
    persistentVolume:
      size: 10Gi

grafana:
  enabled: true
  persistence:
    size: 10Gi
  admin:
    password: ""  # Set via --set or secrets management

loki:
  enabled: true
  persistence:
    size: 10Gi

tempo:
  enabled: true
  persistence:
    size: 10Gi
```

## Deployment Script

The `scripts/deploy.sh` script provides an easy way to deploy the application:

```bash
./scripts/deploy.sh [options]

Options:
  -e, --env ENV         Environment to deploy (dev/prod) [default: dev]
  -n, --namespace NS    Kubernetes namespace [default: atoma-proxy-{env}]
  -r, --release NAME    Helm release name [default: atoma-proxy-{env}]
  -p, --password PASS   Password for PostgreSQL and Grafana
  -h, --help           Show this help message
```

## Environment-Specific Configurations

### Development
- Uses staging SSL certificates
- Lower resource limits
- Debug logging
- Single replica
- Smaller storage volumes
- Dev-specific hostnames

### Production
- Uses production SSL certificates
- Higher resource limits
- Info logging
- Multiple replicas
- Larger storage volumes
- Production hostnames
- Secure password management

## Monitoring and Logging

The chart includes a complete monitoring stack:

- **Prometheus**: Metrics collection
- **Grafana**: Metrics visualization
- **Loki**: Log aggregation
- **Tempo**: Distributed tracing

Access the monitoring tools at:
- Grafana: `https://grafana.{domain}`
- Prometheus: `https://prometheus.{domain}`
- Loki: `https://loki.{domain}`
- Tempo: `https://tempo.{domain}`

## Security Considerations

1. **Passwords**: Always set secure passwords for production using:
   ```bash
   --set postgresql.auth.password=your-secure-password \
   --set grafana.admin.password=your-secure-password
   ```

2. **SSL Certificates**: The chart uses cert-manager for SSL certificate management:
   - Development: `letsencrypt-staging`
   - Production: `letsencrypt-prod`

3. **Resource Limits**: Adjust resource limits based on your cluster capacity and application needs.

## Troubleshooting

1. **Pod Issues**:
   ```bash
   kubectl get pods -n atoma-proxy-{env}
   kubectl describe pod <pod-name> -n atoma-proxy-{env}
   kubectl logs <pod-name> -n atoma-proxy-{env}
   ```

2. **Ingress Issues**:
   ```bash
   kubectl get ingress -n atoma-proxy-{env}
   kubectl describe ingress <ingress-name> -n atoma-proxy-{env}
   ```

3. **Database Issues**:
   ```bash
   kubectl get pods -n atoma-proxy-{env} -l app.kubernetes.io/name=postgresql
   kubectl logs <postgres-pod> -n atoma-proxy-{env}
   ```

## Maintenance

1. **Updating the Chart**:
   ```bash
   helm repo update
   helm upgrade atoma-proxy-{env} . -f values-{env}.yaml
   ```

2. **Backup**:
   - PostgreSQL data is stored in persistent volumes
   - Regular backups should be configured for production

3. **Scaling**:
   - Adjust replicas in values file
   - Scale horizontally: `kubectl scale deployment atoma-proxy-{env} --replicas=N`

## Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Submit a pull request

## License

This chart is licensed under the same license as the Atoma Proxy application.