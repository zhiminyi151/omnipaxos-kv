#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STATE_FILE="$SCRIPT_DIR/.nemesis-state.env"
SERVERS=(s1 s2 s3)
LEADER_CONTAINER="${LEADER_CONTAINER:-s1}"

PARTITIONED_NODE=""
CRASHED_NODE=""

if [[ -f "$STATE_FILE" ]]; then
  # shellcheck source=/dev/null
  source "$STATE_FILE"
fi

save_state() {
  cat > "$STATE_FILE" <<EOF
PARTITIONED_NODE=${PARTITIONED_NODE}
CRASHED_NODE=${CRASHED_NODE}
EOF
}

resolve_network() {
  local net
  net="$(docker inspect -f '{{range $k,$v := .NetworkSettings.Networks}}{{println $k}}{{end}}' s1 | awk 'NF {print; exit}')"
  if [[ -z "$net" ]]; then
    echo "Could not resolve Docker network from container s1" >&2
    exit 1
  fi
  echo "$net"
}

pick_random_server() {
  local idx=$((RANDOM % ${#SERVERS[@]}))
  echo "${SERVERS[$idx]}"
}

disconnect_node() {
  local network="$1"
  local node="$2"
  docker network disconnect "$network" "$node" >/dev/null 2>&1 || true
}

reconnect_node() {
  local network="$1"
  local node="$2"
  docker network connect --alias "$node" "$network" "$node" >/dev/null 2>&1 || true
}

ensure_container_exists() {
  local node="$1"
  if ! docker inspect "$node" >/dev/null 2>&1; then
    echo "Container $node not found. Is compose stack up?" >&2
    exit 1
  fi
}

restart_shim_if_needed() {
  local resumed_node="$1"
  if [[ "$resumed_node" != "$LEADER_CONTAINER" ]]; then
    return 0
  fi
  if docker inspect shim >/dev/null 2>&1; then
    docker restart shim >/dev/null
    echo "resume: restarted shim after leader recovery"
  fi
}

cmd="${1:-status}"
arg="${2:-}"
network="$(resolve_network)"

case "$cmd" in
  partition-start)
    ensure_container_exists s1
    if [[ -n "$PARTITIONED_NODE" ]]; then
      echo "partition already active on $PARTITIONED_NODE"
      exit 0
    fi

    mode="${arg:-random-halves}"
    case "$mode" in
      isolate-leader)
        PARTITIONED_NODE="$LEADER_CONTAINER"
        ;;
      random-halves)
        PARTITIONED_NODE="$(pick_random_server)"
        ;;
      majority-minority)
        PARTITIONED_NODE="s3"
        ;;
      *)
        echo "Unknown partition mode: $mode" >&2
        echo "Use: isolate-leader | random-halves | majority-minority" >&2
        exit 1
        ;;
    esac

    disconnect_node "$network" "$PARTITIONED_NODE"
    save_state
    echo "partition-start: isolated $PARTITIONED_NODE from $network"
    ;;

  partition-stop)
    if [[ -z "$PARTITIONED_NODE" ]]; then
      echo "partition-stop: no active partition"
      exit 0
    fi

    reconnect_node "$network" "$PARTITIONED_NODE"
    echo "partition-stop: reconnected $PARTITIONED_NODE to $network"
    PARTITIONED_NODE=""
    save_state
    ;;

  crash)
    ensure_container_exists s1
    if [[ -n "$CRASHED_NODE" ]]; then
      echo "crash: already crashed node $CRASHED_NODE"
      exit 0
    fi

    target="${arg:-random}"
    if [[ "$target" == "random" ]]; then
      target="$(pick_random_server)"
    elif [[ "$target" == "leader" ]]; then
      target="$LEADER_CONTAINER"
    elif [[ "$target" != "s1" && "$target" != "s2" && "$target" != "s3" ]]; then
      echo "Unknown crash target: $target" >&2
      echo "Use: random | leader | s1 | s2 | s3" >&2
      exit 1
    fi

    CRASHED_NODE="$target"
    docker kill "$CRASHED_NODE" >/dev/null
    save_state
    echo "crash: killed $CRASHED_NODE"
    ;;

  resume)
    if [[ -z "$CRASHED_NODE" ]]; then
      echo "resume: no crashed node to resume"
      exit 0
    fi

    docker start "$CRASHED_NODE" >/dev/null
    echo "resume: started $CRASHED_NODE"
    restart_shim_if_needed "$CRASHED_NODE"
    CRASHED_NODE=""
    save_state
    ;;

  clear)
    "$0" partition-stop || true
    "$0" resume || true
    rm -f "$STATE_FILE"
    echo "clear: state reset"
    ;;

  status)
    echo "network=$network"
    echo "partitioned_node=${PARTITIONED_NODE:-none}"
    echo "crashed_node=${CRASHED_NODE:-none}"
    ;;

  *)
    cat <<EOF
Usage:
  $0 status
  $0 partition-start [random-halves|isolate-leader|majority-minority]
  $0 partition-stop
  $0 crash [random|leader|s1|s2|s3]
  $0 resume
  $0 clear
EOF
    exit 1
    ;;
esac
