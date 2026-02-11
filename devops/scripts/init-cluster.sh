#!/bin/bash
# Initialize a local 4-node cluster (doc 15 section 6)
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DEVOPS_DIR="$(dirname "$SCRIPT_DIR")"

echo "=== Generating keypairs for 4 nodes ==="
for i in 0 1 2 3; do
    KEY_FILE="${DEVOPS_DIR}/config/node${i}/node_key.json"
    if [ -f "$KEY_FILE" ]; then
        echo "  node${i}: key already exists, skipping"
    else
        echo "  node${i}: generating keypair..."
        # In production, use: rustbft-node keygen --output "$KEY_FILE"
        # For now, the node will auto-generate on first start
        echo "  node${i}: will be auto-generated on first start"
    fi
done

echo ""
echo "=== Starting cluster ==="
cd "$DEVOPS_DIR"
docker-compose up -d

echo ""
echo "=== Waiting for nodes to start ==="
sleep 5

echo ""
echo "=== Checking health ==="
for port in 26657 26756 26856 26956; do
    echo -n "  Port ${port}: "
    curl -s "http://localhost:${port}/health" 2>/dev/null || echo "not ready"
done

echo ""
echo "=== Cluster initialized ==="
echo "  Prometheus: http://localhost:9090"
echo "  Grafana:    http://localhost:3000 (admin/admin)"
echo "  Node 0 RPC: http://localhost:26657"
echo "  Node 1 RPC: http://localhost:26756"
echo "  Node 2 RPC: http://localhost:26856"
echo "  Node 3 RPC: http://localhost:26956"
