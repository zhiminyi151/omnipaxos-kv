#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
COMPOSE_FILE="${COMPOSE_FILE:-$SCRIPT_DIR/docker-compose-nemesis.yml}"
NEMESIS_MODE="${NEMESIS_MODE:-mixed}" # partition | crash | mixed
PARTITION_MODE="${PARTITION_MODE:-random-halves}"
CRASH_TARGET="${CRASH_TARGET:-random}" # random | s1 | s2 | s3
NEMESIS_INTERVAL="${NEMESIS_INTERVAL:-8}"
FAULT_DURATION="${FAULT_DURATION:-6}"
BOOT_WAIT_SECS="${BOOT_WAIT_SECS:-8}"
KEEP_STACK="${KEEP_STACK:-0}"
NEMESIS_PID=""

cleanup() {
  if [[ -n "$NEMESIS_PID" ]] && kill -0 "$NEMESIS_PID" >/dev/null 2>&1; then
    kill "$NEMESIS_PID" >/dev/null 2>&1 || true
    wait "$NEMESIS_PID" 2>/dev/null || true
  fi

  "$SCRIPT_DIR/nemesis.sh" clear >/dev/null 2>&1 || true

  if [[ "$KEEP_STACK" != "1" ]]; then
    docker compose -f "$COMPOSE_FILE" down >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT INT TERM

run_nemesis_loop() {
  while true; do
    action="$NEMESIS_MODE"
    if [[ "$NEMESIS_MODE" == "mixed" ]]; then
      if (( RANDOM % 2 == 0 )); then
        action="partition"
      else
        action="crash"
      fi
    fi

    case "$action" in
      partition)
        "$SCRIPT_DIR/nemesis.sh" partition-start "$PARTITION_MODE"
        sleep "$FAULT_DURATION"
        "$SCRIPT_DIR/nemesis.sh" partition-stop
        ;;
      crash)
        "$SCRIPT_DIR/nemesis.sh" crash "$CRASH_TARGET"
        sleep "$FAULT_DURATION"
        "$SCRIPT_DIR/nemesis.sh" resume
        ;;
      *)
        echo "Invalid NEMESIS_MODE=$NEMESIS_MODE (use partition|crash|mixed)" >&2
        exit 1
        ;;
    esac
    sleep "$NEMESIS_INTERVAL"
  done
}

echo "Building images..."
docker compose -f "$COMPOSE_FILE" build
echo "Starting cluster + shim..."
docker compose -f "$COMPOSE_FILE" up -d
sleep "$BOOT_WAIT_SECS"

echo "Starting nemesis loop: mode=$NEMESIS_MODE interval=${NEMESIS_INTERVAL}s fault_duration=${FAULT_DURATION}s"
run_nemesis_loop &
NEMESIS_PID=$!

echo "Running Maelstrom lin-kv against SHIM_URL=${SHIM_URL:-http://127.0.0.1:3000}"
MAELSTROM_DIR="${MAELSTROM_DIR:-$HOME/maelstrom}" \
SHIM_URL="${SHIM_URL:-http://127.0.0.1:3000}" \
TIME_LIMIT="${TIME_LIMIT:-30}" \
NODE_COUNT="${NODE_COUNT:-3}" \
RATE="${RATE:-50}" \
CONCURRENCY="${CONCURRENCY:-6}" \
"$SCRIPT_DIR/run-maelstrom-lin-kv.sh"

echo "Nemesis test run completed."
