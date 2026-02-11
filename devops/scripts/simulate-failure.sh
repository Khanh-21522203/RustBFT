#!/bin/bash
# Failure simulation scripts (doc 15 section 11)
set -e

ACTION="${1:-help}"

case "$ACTION" in
    crash)
        NODE="${2:-rustbft-node2}"
        echo "=== Killing ${NODE} (simulates crash) ==="
        docker kill "$NODE"
        echo "Verify cluster continues: curl -s http://localhost:26657/health"
        ;;
    restart)
        NODE="${2:-rustbft-node2}"
        echo "=== Restarting ${NODE} ==="
        docker start "$NODE"
        echo "Waiting for catch-up..."
        sleep 10
        echo "Check status: curl -s http://localhost:26857/health"
        ;;
    partition)
        echo "=== Partitioning node2 and node3 ==="
        docker network disconnect rustbft-net rustbft-node2
        docker network disconnect rustbft-net rustbft-node3
        echo "Partition active. Neither side has >2/3."
        echo "To heal: $0 heal"
        ;;
    heal)
        echo "=== Healing partition ==="
        docker network connect rustbft-net rustbft-node2
        docker network connect rustbft-net rustbft-node3
        echo "Partition healed. Commits should resume."
        ;;
    slow)
        NODE="${2:-rustbft-node1}"
        echo "=== Adding 200ms latency to ${NODE} ==="
        docker exec "$NODE" tc qdisc add dev eth0 root netem delay 200ms
        echo "To remove: $0 unslow ${NODE}"
        ;;
    unslow)
        NODE="${2:-rustbft-node1}"
        echo "=== Removing latency from ${NODE} ==="
        docker exec "$NODE" tc qdisc del dev eth0 root
        ;;
    status)
        echo "=== Cluster status ==="
        for port in 26657 26756 26856 26956; do
            echo -n "  Port ${port}: "
            curl -s "http://localhost:${port}/health" 2>/dev/null | python3 -m json.tool 2>/dev/null || echo "unreachable"
        done
        ;;
    *)
        echo "Usage: $0 {crash|restart|partition|heal|slow|unslow|status} [node]"
        echo ""
        echo "  crash [node]    - Kill a node (default: rustbft-node2)"
        echo "  restart [node]  - Restart a killed node"
        echo "  partition       - Partition node2,3 from node0,1"
        echo "  heal            - Heal the partition"
        echo "  slow [node]     - Add 200ms latency to a node"
        echo "  unslow [node]   - Remove latency"
        echo "  status          - Check all nodes"
        ;;
esac
