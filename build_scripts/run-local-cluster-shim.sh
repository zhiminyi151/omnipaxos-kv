#!/bin/bash

# Run from anywhere: cd into script directory first.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

cluster_size=3
rust_log="${RUST_LOG:-info}"

# Clean up child processes
interrupt() {
    pkill -P $$
}
trap "interrupt" SIGINT

mkdir -p "./logs"
cluster_config_path="./cluster-config.toml"

for ((i = 1; i <= cluster_size; i++)); do
    server_config_path="./server-${i}-shim-config.toml"
    RUST_LOG="$rust_log" SERVER_CONFIG_FILE="$server_config_path" CLUSTER_CONFIG_FILE="$cluster_config_path" cargo run --manifest-path="../Cargo.toml" --bin server &
done

wait
