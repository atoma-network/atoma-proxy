# Atoma Proxy Helm Chart

This Helm chart deploys the Atoma Proxy application along with its dependencies, including PostgreSQL, Prometheus, Grafana, Loki, and Tempo.

## Prerequisites

- Kubernetes cluster (v1.19+)
- Helm 3.x
- cert-manager (for SSL certificates)
- nginx-ingress-controller
- kubectl configured to communicate with your cluster

## Installation

### Step-by-Step Installation

1. **Start Minikube** (if using Minikube):
   ```bash
   minikube start -p atoma-proxy \
     --driver=docker \
     --cpus=4 \
     --memory=8g \
     --disk-size=50g \
     --force \
     --addons=ingress,metrics-server
   ```

2. **Create the namespace**:
   ```bash
   kubectl create namespace atoma-proxy
   ```

3. **Add required Helm repositories**:
   ```bash
   helm repo add bitnami https://charts.bitnami.com/bitnami
   helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
   helm repo add grafana https://grafana.github.io/helm-charts
   helm repo update
   ```

4. **Navigate to the Helm chart directory and build dependencies**:
   ```bash
   cd helm/atoma-proxy
   helm dependency build
   ```

5. **Install the chart with development values**:
   ```bash
   helm install atoma-proxy . -f values-dev.yaml -n atoma-proxy
   ```

6. **Start Minikube tunnel** (in a separate terminal) for ingress access:
   ```bash
   minikube tunnel -p atoma-proxy
   ```

7. **Verify the installation**:
   ```bash
   # Check all pods are running
   kubectl get pods -n atoma-proxy

   # Check ingress resources
   kubectl get ingress -n atoma-proxy

   # Check services
   kubectl get svc -n atoma-proxy
   ```

### Quick Installation

Alternatively, you can use the deployment script:
```bash
./scripts/deploy.sh -e dev -p your-password
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

## Quickstart Checklist (including Apple Silicon/M1/M2/M3)

1. **Build and Push a Multi-Arch Image (if on Apple Silicon):**
   ```bash
   docker buildx build --platform linux/amd64,linux/arm64 -t ghcr.io/atoma-network/atoma-proxy:latest --push .
   ```
   > If you are on an M1/M2/M3 Mac, you must use a multi-arch image or an arm64 image. Otherwise, your pod will fail with `rosetta error: failed to open elf at /lib64/ld-linux-x86-64.so.2`.

2. **Start Minikube:**
   ```bash
   minikube start -p atoma-proxy \
     --driver=docker \
     --cpus=4 \
     --memory=8g \
     --disk-size=50g \
     --force \
     --addons=ingress,metrics-server
   ```

3. **Create the namespace:**
   ```bash
   kubectl create namespace atoma-proxy
   ```

4. **Add required Helm repositories:**
   ```bash
   helm repo add bitnami https://charts.bitnami.com/bitnami
   helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
   helm repo add grafana https://grafana.github.io/helm-charts
   helm repo update
   ```

5. **Build Helm dependencies:**
   ```bash
   cd helm/atoma-proxy
   helm dependency build
   ```

6. **Install the chart with development values:**
   ```bash
   helm install atoma-proxy . -f values-dev.yaml -n atoma-proxy
   ```

7. **Start Minikube tunnel (for ingress):**
   ```bash
   minikube tunnel -p atoma-proxy
   ```

8. **Verify all pods are running:**
   ```bash
   kubectl get pods -n atoma-proxy
   ```

9. **If using OTEL Collector, ensure the deployment and service are present:**
   ```bash
   kubectl get deployment,svc -n atoma-proxy | grep otel-collector
   ```

10. **If Atoma Proxy pod is not running, check logs and describe:**
    ```bash
    kubectl logs -n atoma-proxy -l app.kubernetes.io/name=atoma-proxy --tail=50
    kubectl describe pod -n atoma-proxy -l app.kubernetes.io/name=atoma-proxy
    ```

11. **If you see `ImagePullBackOff`, ensure your image tag is correct and public, or use an image pull secret for private images.**

12. **If you see `rosetta error: failed to open elf at /lib64/ld-linux-x86-64.so.2`, you need a multi-arch image. See step 1.**

13. **Check Prometheus targets:**
    ```bash
    kubectl port-forward svc/atoma-proxy-prometheus-server 9090:80 -n atoma-proxy
    # Then open http://localhost:9090/targets in your browser
    ```

14. **Check for Atoma metrics in Prometheus/Grafana:**
    - In Prometheus or Grafana, search for metrics with the prefix `atoma_`.

---

## Debugging & Verification

- **Check all pods in the namespace:**
  ```bash
  kubectl get pods -n atoma-proxy
  ```
- **Check logs for a specific pod:**
  ```bash
  kubectl logs -n atoma-proxy <pod-name>
  ```
- **Describe a pod for events and status:**
  ```bash
  kubectl describe pod -n atoma-proxy <pod-name>
  ```
- **Check OTEL Collector deployment and service:**
  ```bash
  kubectl get deployment,svc -n atoma-proxy | grep otel-collector
  ```
- **Check if Atoma Proxy pod is running:**
  ```bash
  kubectl get pods -n atoma-proxy -l app.kubernetes.io/name=atoma-proxy
  ```
- **Check for image pull errors:**
  ```bash
  kubectl describe pod -n atoma-proxy -l app.kubernetes.io/name=atoma-proxy
  ```
- **Check Prometheus targets:**
  ```bash
  kubectl port-forward svc/atoma-proxy-prometheus-server 9090:80 -n atoma-proxy
  # Open http://localhost:9090/targets
  ```
- **Check for Atoma metrics in Prometheus/Grafana:**
  - In Prometheus or Grafana, search for metrics with the prefix `atoma_`.

  **Create Grafana secrets**
  ```
  kubectl create secret generic grafana-admin \
  --from-literal=admin-user=admin \
  --from-literal=admin-password=admin \
  -n atoma-proxy-dev
  ```

  **Create OpenRouter++
  ```
  kubectl create secret generic atoma-proxy-dev-open-router \
  --from-literal=open_router.json='{"api_key": "your-api-key"}' \
  -n atoma-proxy-dev
  ```