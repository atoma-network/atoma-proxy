#!/bin/bash

set -e

# Function to wait for API server
wait_for_api() {
    echo "Waiting for API server to be ready..."
    until kubectl get nodes &>/dev/null; do
        echo "Waiting for API server..."
        sleep 5
    done
}

# Function to apply manifest with retries
apply_with_retry() {
    local manifest=$1
    local max_attempts=3
    local attempt=1

    while [ $attempt -le $max_attempts ]; do
        echo "Attempt $attempt of $max_attempts to apply manifest..."
        if kubectl apply -f "$manifest" --validate=false; then
            return 0
        fi
        echo "Attempt $attempt failed, waiting before retry..."
        sleep 10
        attempt=$((attempt + 1))
    done
    echo "Failed to apply manifest after $max_attempts attempts"
    return 1
}

# Wait for API server
wait_for_api

# Enable metrics server
echo "Enabling metrics server..."
apply_with_retry "https://github.com/kubernetes-sigs/metrics-server/releases/latest/download/components.yaml"

# Enable default storage class
echo "Enabling default storage class..."
apply_with_retry "https://raw.githubusercontent.com/kubernetes/minikube/master/deploy/addons/storageclass/storageclass.yaml"

# Enable storage provisioner
echo "Enabling storage provisioner..."
apply_with_retry "https://raw.githubusercontent.com/kubernetes/minikube/master/deploy/addons/storage-provisioner/storage-provisioner.yaml"

# Wait for metrics server to be ready
echo "Waiting for metrics server to be ready..."
kubectl wait --for=condition=ready pod -l k8s-app=metrics-server -n kube-system --timeout=300s || true

echo "Addons have been installed. Please verify their status with:"
echo "kubectl get pods -n kube-system"