#!/bin/bash

# Example wrapper for running a lin-kv workload against the maelstrom_node proxy.
# Requires a Maelstrom release directory and a running shim endpoint.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

MAELSTROM_DIR="${MAELSTROM_DIR:-$HOME/maelstrom}"
SHIM_URL="${SHIM_URL:-http://127.0.0.1:3000}"
TIME_LIMIT="${TIME_LIMIT:-20}"
NODE_COUNT="${NODE_COUNT:-3}"
RATE="${RATE:-50}"
CONCURRENCY="${CONCURRENCY:-6}"

cd "$PROJECT_ROOT"
cargo build --bin maelstrom_node

cd "$MAELSTROM_DIR"
SHIM_URL="$SHIM_URL" ./maelstrom test \
  -w lin-kv \
  --bin "$PROJECT_ROOT/target/debug/maelstrom_node" \
  --time-limit "$TIME_LIMIT" \
  --node-count "$NODE_COUNT" \
  --rate "$RATE" \
  --concurrency "$CONCURRENCY"
