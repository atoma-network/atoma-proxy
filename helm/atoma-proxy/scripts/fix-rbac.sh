#!/bin/bash

set -e

# Wait for API server to be ready
echo "Waiting for API server to be ready..."
until kubectl get nodes &>/dev/null; do
    echo "Waiting for API server..."
    sleep 5
done

# Create ClusterRole for namespace controller
echo "Creating ClusterRole..."
kubectl apply -f - --validate=false <<EOF
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: namespace-controller
rules:
- apiGroups: ["coordination.k8s.io"]
  resources: ["leases"]
  verbs: ["get", "list", "watch", "create", "update", "patch", "delete"]
EOF

# Create ClusterRoleBinding
echo "Creating ClusterRoleBinding..."
kubectl apply -f - --validate=false <<EOF
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: namespace-controller
subjects:
- kind: ServiceAccount
  name: namespace-controller
  namespace: kube-system
roleRef:
  kind: ClusterRole
  name: namespace-controller
  apiGroup: rbac.authorization.k8s.io
EOF

# Wait for namespace controller to be ready before restarting
echo "Waiting for namespace controller to be ready..."
kubectl wait --for=condition=available deployment/namespace-controller -n kube-system --timeout=300s || true

# Restart the namespace controller
echo "Restarting namespace controller..."
kubectl -n kube-system rollout restart deployment namespace-controller || true

echo "RBAC permissions have been updated. Please wait for the namespace controller to restart..."