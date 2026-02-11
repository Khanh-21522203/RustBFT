#!/bin/bash
# Submit a test transaction (doc 15 section 8)
set -e

NODE_URL="${1:-http://localhost:26657}"
TX_HEX="${2:-0102030405}"

echo "Submitting transaction to ${NODE_URL}..."
curl -s -X POST "${NODE_URL}" \
  -H 'Content-Type: application/json' \
  -d "{
    \"jsonrpc\": \"2.0\",
    \"method\": \"broadcast_tx\",
    \"params\": {
        \"tx\": \"${TX_HEX}\"
    },
    \"id\": 1
  }" | python3 -m json.tool 2>/dev/null || cat

echo ""
