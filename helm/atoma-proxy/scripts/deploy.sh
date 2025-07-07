#!/bin/bash

set -e

# Default values
ENV="dev"
NAMESPACE=""
VALUES_FILE=""
RELEASE_NAME=""
PASSWORD=""

# Check for required tools
check_prerequisites() {
    local missing_tools=()

    # Check for kubectl
    if ! command -v kubectl &> /dev/null; then
        missing_tools+=("kubectl")
    fi

    # Check for helm
    if ! command -v helm &> /dev/null; then
        missing_tools+=("helm")
    fi

    # If any tools are missing, show error and exit
    if [ ${#missing_tools[@]} -ne 0 ]; then
        echo "Error: The following required tools are not installed:"
        for tool in "${missing_tools[@]}"; do
            echo "  - $tool"
        done
        echo ""
        echo "Please install the missing tools and try again."
        echo "You can install them using:"
        echo "  - kubectl: https://kubernetes.io/docs/tasks/tools/install-kubectl/"
        echo "  - helm: https://helm.sh/docs/intro/install/"
        exit 1
    fi

    # Check kubectl configuration
    echo "Checking kubectl configuration..."

    # Check if kubeconfig exists
    if [ -z "$KUBECONFIG" ]; then
        if [ ! -f "$HOME/.kube/config" ]; then
            echo "Error: No kubeconfig file found."
            echo "Please ensure you have a valid kubeconfig file at ~/.kube/config"
            echo "You can get this file from your cluster administrator or cloud provider."
            exit 1
        fi
    fi

    # Check current context
    if ! kubectl config current-context &> /dev/null; then
        echo "Error: No current context set in kubectl configuration."
        echo "Available contexts:"
        kubectl config get-contexts
        echo ""
        echo "Please set a context using:"
        echo "  kubectl config use-context <context-name>"
        exit 1
    fi

    # Check cluster connection
    if ! kubectl cluster-info &> /dev/null; then
        echo "Error: Cannot connect to the Kubernetes cluster."
        echo "Current context: $(kubectl config current-context)"
        echo ""
        echo "Please check:"
        echo "  1. Your cluster is running"
        echo "  2. Your kubeconfig file is correct"
        echo "  3. You have network access to the cluster"
        echo "  4. Your credentials are valid"
        echo ""
        echo "You can verify your configuration with:"
        echo "  kubectl config view"
        echo "  kubectl cluster-info"
        exit 1
    fi

    echo "✓ kubectl is properly configured"
}

# Help message
show_help() {
    echo "Usage: $0 [options]"
    echo "Options:"
    echo "  -e, --env ENV         Environment to deploy (dev/prod) [default: dev]"
    echo "  -n, --namespace NS    Kubernetes namespace [default: atoma-proxy-{env}]"
    echo "  -r, --release NAME    Helm release name [default: atoma-proxy-{env}]"
    echo "  -p, --password PASS   Password for PostgreSQL and Grafana"
    echo "  -h, --help           Show this help message"
    exit 1
}

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -e|--env)
            ENV="$2"
            shift 2
            ;;
        -n|--namespace)
            NAMESPACE="$2"
            shift 2
            ;;
        -r|--release)
            RELEASE_NAME="$2"
            shift 2
            ;;
        -p|--password)
            PASSWORD="$2"
            shift 2
            ;;
        -h|--help)
            show_help
            ;;
        *)
            echo "Unknown option: $1"
            show_help
            ;;
    esac
done

# Check prerequisites
echo "Checking prerequisites..."
check_prerequisites

# Validate environment
if [[ "$ENV" != "dev" && "$ENV" != "prod" ]]; then
    echo "Error: Environment must be either 'dev' or 'prod'"
    exit 1
fi

# Set default values if not provided
if [ -z "$NAMESPACE" ]; then
    NAMESPACE="atoma-proxy-$ENV"
fi

if [ -z "$RELEASE_NAME" ]; then
    RELEASE_NAME="atoma-proxy-$ENV"
fi

# Get the directory where the script is located
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )"
CHART_DIR="$(dirname "$SCRIPT_DIR")"

# Set values file based on environment
VALUES_FILE="$CHART_DIR/values-$ENV.yaml"

# Check if values file exists
if [ ! -f "$VALUES_FILE" ]; then
    echo "Error: Values file $VALUES_FILE not found"
    exit 1
fi

# Add Helm repositories if not already added
echo "Adding Helm repositories..."
helm repo add bitnami https://charts.bitnami.com/bitnami
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
helm repo add grafana https://grafana.github.io/helm-charts
helm repo update

# Update dependencies
echo "Updating Helm dependencies..."
cd "$CHART_DIR"
helm dependency update

# Prepare password arguments
PASSWORD_ARGS=""
if [ ! -z "$PASSWORD" ]; then
    PASSWORD_ARGS="--set postgresql.auth.password=$PASSWORD --set grafana.admin.password=$PASSWORD"
fi

# Deploy the chart
echo "Deploying $RELEASE_NAME to namespace $NAMESPACE..."
helm upgrade --install $RELEASE_NAME . \
    --namespace $NAMESPACE \
    --create-namespace \
    -f "$VALUES_FILE" \
    $PASSWORD_ARGS

echo "Deployment completed successfully!"
echo "Release: $RELEASE_NAME"
echo "Namespace: $NAMESPACE"
echo "Environment: $ENV"