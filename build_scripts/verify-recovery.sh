#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COMPOSE_FILE="${COMPOSE_FILE:-$SCRIPT_DIR/docker-compose-nemesis.yml}"
SHIM_URL="${SHIM_URL:-http://127.0.0.1:3000}"
BOOT_WAIT_SECS="${BOOT_WAIT_SECS:-8}"
RECOVERY_WAIT_SECS="${RECOVERY_WAIT_SECS:-8}"
CRASH_TARGET="${CRASH_TARGET:-s1}" # s1 | s2 | s3 | random
KEY_PREFIX="${KEY_PREFIX:-recovery}"
KEY_COUNT="${KEY_COUNT:-8}"
READ_RETRIES="${READ_RETRIES:-20}"
READ_RETRY_DELAY_SECS="${READ_RETRY_DELAY_SECS:-1}"
READ_CURL_TIMEOUT_SECS="${READ_CURL_TIMEOUT_SECS:-2}"
KEEP_STACK="${KEEP_STACK:-0}"
NEMESIS_STATE_FILE="$SCRIPT_DIR/.nemesis-state.env"

declare -a EXPECTED_VALUES=()

cleanup() {
  "$SCRIPT_DIR/nemesis.sh" clear >/dev/null 2>&1 || true
  if [[ "$KEEP_STACK" != "1" ]]; then
    docker compose -f "$COMPOSE_FILE" down >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT INT TERM

put_key() {
  local key="$1"
  local value="$2"
  curl -fsS -X PUT \
    -H "Content-Type: application/json" \
    -d "{\"value\":\"$value\"}" \
    "$SHIM_URL/kv/$key" >/dev/null
}

get_value() {
  local key="$1"
  local attempt
  local response
  local value
  for attempt in $(seq 1 "$READ_RETRIES"); do
    response="$(curl -sS --max-time "$READ_CURL_TIMEOUT_SECS" "$SHIM_URL/kv/$key" || true)"
    value="$(python3 -c '
import json,sys
payload = sys.argv[1]
try:
    obj = json.loads(payload)
except Exception:
    print("__PARSE_ERROR__")
    raise SystemExit(0)
if obj.get("ok") is True:
    print(obj.get("value") or "")
else:
    print("__NOT_READY__")
' "$response")"
    if [[ "$value" != "__PARSE_ERROR__" && "$value" != "__NOT_READY__" ]]; then
      echo "$value"
      return 0
    fi
    sleep "$READ_RETRY_DELAY_SECS"
  done
  echo "failed to read key=$key after $READ_RETRIES retries" >&2
  return 1
}

echo "Building images..."
docker compose -f "$COMPOSE_FILE" build

echo "Starting cluster + shim..."
docker compose -f "$COMPOSE_FILE" up -d
sleep "$BOOT_WAIT_SECS"

echo "Writing baseline keys..."
for i in $(seq 1 "$KEY_COUNT"); do
  key="${KEY_PREFIX}-${i}"
  value="v${i}-before-crash"
  EXPECTED_VALUES+=("$value")
  put_key "$key" "$value"
done

echo "Crashing node: $CRASH_TARGET"
"$SCRIPT_DIR/nemesis.sh" crash "$CRASH_TARGET"
sleep "$RECOVERY_WAIT_SECS"

CRASHED_NODE=""
if [[ -f "$NEMESIS_STATE_FILE" ]]; then
  # shellcheck source=/dev/null
  source "$NEMESIS_STATE_FILE"
fi

echo "Resuming crashed node..."
"$SCRIPT_DIR/nemesis.sh" resume

if [[ "${CRASHED_NODE:-}" == "s1" ]]; then
  # Keep followers running; they have their own reconnect loop to node 1.
  # Restarting followers here can create extra connection churn.
  echo "Leader was crashed; waiting for followers to reconnect to recovered leader..."
fi

sleep "$RECOVERY_WAIT_SECS"

echo "Verifying key values after restart..."
for i in $(seq 1 "$KEY_COUNT"); do
  key="${KEY_PREFIX}-${i}"
  expected="${EXPECTED_VALUES[$((i - 1))]}"
  actual="$(get_value "$key")"
  if [[ "$actual" != "$expected" ]]; then
    echo "Mismatch for key=$key expected=$expected actual=$actual" >&2
    exit 1
  fi
done

echo "Recovery verification passed: all committed keys survived crash-restart."
