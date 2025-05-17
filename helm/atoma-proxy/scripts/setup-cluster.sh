#!/bin/bash

set -e

# Default values
CLUSTER_TYPE="minikube"
CLOUD_PROVIDER=""
CLUSTER_NAME="atoma-proxy"
REGION="us-west-2"

# Help message
show_help() {
    echo "Usage: $0 [options]"
    echo "Options:"
    echo "  -t, --type TYPE       Cluster type (minikube/kind/eks/gke/aks) [default: minikube]"
    echo "  -p, --provider PROV   Cloud provider (aws/gcp/azure) [required for cloud clusters]"
    echo "  -n, --name NAME       Cluster name [default: atoma-proxy]"
    echo "  -r, --region REGION   Region for cloud clusters [default: us-west-2]"
    echo "  -h, --help           Show this help message"
    exit 1
}

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -t|--type)
            CLUSTER_TYPE="$2"
            shift 2
            ;;
        -p|--provider)
            CLOUD_PROVIDER="$2"
            shift 2
            ;;
        -n|--name)
            CLUSTER_NAME="$2"
            shift 2
            ;;
        -r|--region)
            REGION="$2"
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

    # Check for cluster-specific tools
    case $CLUSTER_TYPE in
        minikube)
            if ! command -v minikube &> /dev/null; then
                missing_tools+=("minikube")
            fi
            ;;
        kind)
            if ! command -v kind &> /dev/null; then
                missing_tools+=("kind")
            fi
            ;;
        eks)
            if ! command -v aws &> /dev/null; then
                missing_tools+=("aws-cli")
            fi
            if ! command -v eksctl &> /dev/null; then
                missing_tools+=("eksctl")
            fi
            ;;
        gke)
            if ! command -v gcloud &> /dev/null; then
                missing_tools+=("gcloud")
            fi
            ;;
        aks)
            if ! command -v az &> /dev/null; then
                missing_tools+=("azure-cli")
            fi
            ;;
    esac

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
        echo "  - minikube: https://minikube.sigs.k8s.io/docs/start/"
        echo "  - kind: https://kind.sigs.k8s.io/docs/user/quick-start/#installation"
        echo "  - aws-cli: https://aws.amazon.com/cli/"
        echo "  - eksctl: https://eksctl.io/introduction/installation/"
        echo "  - gcloud: https://cloud.google.com/sdk/docs/install"
        echo "  - azure-cli: https://docs.microsoft.com/en-us/cli/azure/install-azure-cli"
        exit 1
    fi
}

# Setup local development cluster
setup_local_cluster() {
    case $CLUSTER_TYPE in
        minikube)
            echo "Setting up Minikube cluster..."
            # Start minikube with profile name instead of --name flag
            minikube start -p $CLUSTER_NAME --driver=docker --cpus=4 --memory=8g --disk-size=50g

            # Wait for minikube to be ready
            echo "Waiting for minikube to be ready..."
            minikube status -p $CLUSTER_NAME

            # Enable addons
            echo "Enabling minikube addons..."
            minikube addons enable ingress -p $CLUSTER_NAME
            minikube addons enable metrics-server -p $CLUSTER_NAME

            # Update kubeconfig
            echo "Updating kubeconfig..."
            minikube update-context -p $CLUSTER_NAME

            # Set up kubectl to use minikube
            echo "Setting up kubectl configuration..."
            export KUBECONFIG=~/.kube/config
            minikube kubectl -- config use-context $CLUSTER_NAME

            # Verify the context
            if ! kubectl config current-context | grep -q "$CLUSTER_NAME"; then
                echo "Error: Failed to set minikube context"
                exit 1
            fi

            # Test kubectl connection
            echo "Testing kubectl connection..."
            kubectl cluster-info
            ;;
        kind)
            echo "Setting up Kind cluster..."
            cat <<EOF | kind create cluster --name $CLUSTER_NAME --config=-
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
nodes:
- role: control-plane
  kubeadmConfigPatches:
  - |
    kind: InitConfiguration
    nodeRegistration:
      kubeletExtraArgs:
        node-labels: "ingress-ready=true"
  extraPortMappings:
  - containerPort: 80
    hostPort: 80
    protocol: TCP
  - containerPort: 443
    hostPort: 443
    protocol: TCP
EOF
            ;;
    esac
}

# Setup cloud cluster
setup_cloud_cluster() {
    case $CLUSTER_TYPE in
        eks)
            if [ -z "$CLOUD_PROVIDER" ]; then
                echo "Error: Cloud provider must be specified for EKS cluster"
                exit 1
            fi
            echo "Setting up EKS cluster..."
            eksctl create cluster \
                --name $CLUSTER_NAME \
                --region $REGION \
                --node-type t3.medium \
                --nodes 3 \
                --nodes-min 1 \
                --nodes-max 5 \
                --managed
            ;;
        gke)
            if [ -z "$CLOUD_PROVIDER" ]; then
                echo "Error: Cloud provider must be specified for GKE cluster"
                exit 1
            fi
            echo "Setting up GKE cluster..."
            gcloud container clusters create $CLUSTER_NAME \
                --zone $REGION \
                --machine-type e2-medium \
                --num-nodes 3 \
                --enable-ip-alias \
                --enable-autoscaling \
                --min-nodes 1 \
                --max-nodes 5
            gcloud container clusters get-credentials $CLUSTER_NAME --zone $REGION
            ;;
        aks)
            if [ -z "$CLOUD_PROVIDER" ]; then
                echo "Error: Cloud provider must be specified for AKS cluster"
                exit 1
            fi
            echo "Setting up AKS cluster..."
            az aks create \
                --name $CLUSTER_NAME \
                --resource-group $CLUSTER_NAME \
                --location $REGION \
                --node-count 3 \
                --node-vm-size Standard_D2s_v3 \
                --enable-cluster-autoscaler \
                --min-count 1 \
                --max-count 5
            az aks get-credentials --name $CLUSTER_NAME --resource-group $CLUSTER_NAME
            ;;
    esac
}

# Install required components
install_components() {
    echo "Installing required components..."

    # Install cert-manager
    echo "Installing cert-manager..."
    kubectl apply -f https://github.com/cert-manager/cert-manager/releases/download/v1.12.0/cert-manager.yaml
    kubectl wait --for=condition=available --timeout=300s deployment/cert-manager -n cert-manager

    # Clean up existing ingress-nginx if it exists
    echo "Checking for existing ingress-nginx installation..."
    if helm list -n ingress-nginx | grep -q "ingress-nginx"; then
        echo "Removing existing ingress-nginx installation..."
        helm uninstall ingress-nginx -n ingress-nginx
    fi

    # Force delete ingress-nginx namespace and wait for it to be gone
    echo "Cleaning up ingress-nginx namespace..."
    kubectl delete namespace ingress-nginx --force --grace-period=0 || true

    # Wait for namespace to be fully deleted
    echo "Waiting for ingress-nginx namespace to be deleted..."
    while kubectl get namespace ingress-nginx &> /dev/null; do
        echo "Still waiting for ingress-nginx namespace to be deleted..."
        sleep 5
    done

    # Additional cleanup of any remaining resources
    echo "Cleaning up any remaining ingress-nginx resources..."
    kubectl delete clusterrole ingress-nginx --ignore-not-found
    kubectl delete clusterrolebinding ingress-nginx --ignore-not-found
    kubectl delete serviceaccount ingress-nginx -n ingress-nginx --ignore-not-found
    kubectl delete service ingress-nginx-controller -n ingress-nginx --ignore-not-found
    kubectl delete service ingress-nginx-controller-admission -n ingress-nginx --ignore-not-found
    kubectl delete deployment ingress-nginx-controller -n ingress-nginx --ignore-not-found
    kubectl delete validatingwebhookconfiguration ingress-nginx-admission --ignore-not-found
    kubectl delete ingressclass nginx --ignore-not-found
    kubectl delete ingressclass nginx-ingress --ignore-not-found
    kubectl delete ingressclass ingress-nginx --ignore-not-found

    # Install nginx-ingress
    echo "Installing nginx-ingress..."
    helm repo add ingress-nginx https://kubernetes.github.io/ingress-nginx
    helm repo update
    helm install ingress-nginx ingress-nginx/ingress-nginx \
        --namespace ingress-nginx \
        --create-namespace \
        --set controller.service.type=LoadBalancer \
        --set controller.service.externalTrafficPolicy=Local \
        --set controller.service.enableHttps=false \
        --set controller.ingressClassResource.name=nginx \
        --set controller.ingressClassResource.enabled=true \
        --set controller.ingressClassResource.default=true

    # Install metrics-server if not using minikube
    if [ "$CLUSTER_TYPE" != "minikube" ]; then
        echo "Installing metrics-server..."
        kubectl apply -f https://github.com/kubernetes-sigs/metrics-server/releases/latest/download/components.yaml
    fi
}

# Main execution
echo "Starting cluster setup..."

# Check prerequisites
check_prerequisites

# Setup cluster
if [[ "$CLUSTER_TYPE" == "minikube" || "$CLUSTER_TYPE" == "kind" ]]; then
    setup_local_cluster
else
    setup_cloud_cluster
fi

# Install components
install_components

echo "Cluster setup completed successfully!"
echo "Cluster type: $CLUSTER_TYPE"
echo "Cluster name: $CLUSTER_NAME"
echo ""
echo "You can now deploy the application using:"
echo "./deploy.sh -e dev -p your-password"