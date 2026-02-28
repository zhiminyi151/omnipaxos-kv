# OmniPaxos KV (Project 2)

Battle-testing OmniPaxos with Maelstrom/Jepsen-style workloads and nemesis faults.

This repository implements:

- A 3-node OmniPaxos-backed KV store (`server`)
- An HTTP test shim (`shim`) exposing `put/get/cas`
- A Maelstrom proxy node (`maelstrom_node`) that translates stdin/stdout Maelstrom messages to shim HTTP
- Docker-based nemesis for `partition` and `crash/restart`
- Linearizability verification via Maelstrom `lin-kv` (Knossos)
- Bonus recovery verification with persistent storage (`PersistentStorage`)

---

## Architecture

```text
Maelstrom (lin-kv)
   |
   | stdin/stdout JSON
   v
maelstrom_node (N processes)
   |
   | HTTP
   v
shim (single endpoint, default :3000)
   |
   | TCP + bincode
   v
s1 (OmniPaxos server) <--> s2 <--> s3
```

Notes:

- `shim` forwards client commands to `s1`.
- Nemesis operates on Docker containers `s1/s2/s3` (not on `maelstrom_node`).
- Reads are linearizable by design: `Get` is replicated through the OmniPaxos log.

---

## Implemented Project Steps

- [x] **Step 1: Testable Shim** (`put/get/cas` over HTTP)
- [x] **Step 2: Client & Generator** (`maelstrom_node`)
- [x] **Step 3: Fault Injection (Nemesis)** (`partition`, `crash`, `resume`)
- [x] **Step 4: Linearizability Verification** (Maelstrom `lin-kv`)
- [x] **Bonus: Recovery Verification** (persistent storage + crash/restart validation)

---

## Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (stable)
- [Docker](https://www.docker.com/) + `docker compose`
- [Maelstrom](https://github.com/jepsen-io/maelstrom) extracted directory (default: `$HOME/maelstrom`)
- `python3` (used by recovery script JSON parsing)

Optional for local non-Docker build of persistent storage path:

- `clang`, `libclang-dev`, `pkg-config`, `cmake`, `build-essential`

The Dockerfiles already install required native dependencies for RocksDB/bindgen.

---

## Key Binaries

- `server` -> OmniPaxos KV server
- `shim` -> HTTP gateway for testing
- `maelstrom_node` -> workload adapter for Maelstrom
- `client` -> original benchmark client (kept from base repo)

---

## Quick Start

### 1) Baseline (No Faults)

Start cluster + shim:

```bash
docker compose -f build_scripts/docker-compose-nemesis.yml up -d --build
```

Run Maelstrom linearizability test:

```bash
MAELSTROM_DIR="$HOME/maelstrom" \
SHIM_URL="http://127.0.0.1:3000" \
./build_scripts/run-maelstrom-lin-kv.sh
```

Stop stack:

```bash
docker compose -f build_scripts/docker-compose-nemesis.yml down
```

---

### 2) Nemesis Tests (Partition / Crash / Mixed)

One-command run (build + up + nemesis loop + maelstrom):

```bash
./build_scripts/run-nemesis-test.sh
```

Examples:

```bash
# Partition only
NEMESIS_MODE=partition PARTITION_MODE=majority-minority ./build_scripts/run-nemesis-test.sh

# Crash only (target leader)
NEMESIS_MODE=crash CRASH_TARGET=leader ./build_scripts/run-nemesis-test.sh

# Mixed
NEMESIS_MODE=mixed PARTITION_MODE=random-halves CRASH_TARGET=random ./build_scripts/run-nemesis-test.sh
```

---

### 3) Bonus Recovery Verification

Checks that committed data survives crash/restart with persistent storage:

```bash
CRASH_TARGET=s1 KEY_COUNT=8 ./build_scripts/verify-recovery.sh
```

Expected success line:

```text
Recovery verification passed: all committed keys survived crash-restart.
```

You can also test follower recovery:

```bash
CRASH_TARGET=s2 ./build_scripts/verify-recovery.sh
```

---

## Step-by-Step Validation Guide

This section gives a concrete validation method for **each project step** and one complete end-to-end run order.

### Step 1: Testable Shim (`put/get/cas`)

Run local cluster + shim (two terminals):

```bash
# Terminal A
./build_scripts/run-local-cluster-shim.sh

# Terminal B
./build_scripts/run-local-shim.sh
```

Validate API behavior:

```bash
curl -X PUT "http://127.0.0.1:3000/kv/foo" \
  -H "Content-Type: application/json" \
  -d '{"value":"bar"}'

curl "http://127.0.0.1:3000/kv/foo"

curl -X POST "http://127.0.0.1:3000/kv/cas" \
  -H "Content-Type: application/json" \
  -d '{"key":"foo","from":"bar","to":"baz"}'

curl "http://127.0.0.1:3000/kv/foo"
```

Pass criteria:

- `put` returns `{"ok": true}`
- `get` returns expected value
- `cas` with correct `from` sets new value
- subsequent `get` observes updated value

---

### Step 2: Client & Generator (`maelstrom_node`)

Run Maelstrom workload against shim:

```bash
MAELSTROM_DIR="$HOME/maelstrom" \
SHIM_URL="http://127.0.0.1:3000" \
TIME_LIMIT=20 NODE_COUNT=3 RATE=50 CONCURRENCY=6 \
./build_scripts/run-maelstrom-lin-kv.sh
```

Pass criteria:

- Maelstrom starts `maelstrom_node` processes successfully
- Workload executes without process crashes
- Output includes workload stats (ok/fail/info counts)
- If faults are absent, run should end with `valid? true`

What this validates:

- stdin/stdout Maelstrom protocol handling
- HTTP forwarding from `maelstrom_node` to shim
- indeterminate mapping path (timeouts become informational/indeterminate outcomes)

---

### Step 3: Fault Injection (Nemesis)

Run fault campaigns:

```bash
# partition only
NEMESIS_MODE=partition PARTITION_MODE=majority-minority ./build_scripts/run-nemesis-test.sh

# crash only
NEMESIS_MODE=crash CRASH_TARGET=leader ./build_scripts/run-nemesis-test.sh

# mixed
NEMESIS_MODE=mixed PARTITION_MODE=random-halves CRASH_TARGET=random ./build_scripts/run-nemesis-test.sh
```

Pass criteria:

- Logs show fault actions (`partition-start/stop`, `crash`, `resume`)
- Cluster remains testable under load (no permanent hang)
- Maelstrom completes and emits checker output

---

### Step 4: Linearizability Verification

Use baseline and nemesis runs above; inspect final Maelstrom checker output.

Pass criteria:

- `:valid? true`
- `:linearizable {:valid? true ...}`
- no linearizability violation in `:failures`

Result reference:

- See `VERIFICATION_RESULTS.md` for recorded scenario-by-scenario outcomes.

---

### Bonus: Recovery Verification (Persistent Storage + Crash/Restart)

Run recovery script:

```bash
# leader recovery
CRASH_TARGET=s1 KEY_COUNT=8 ./build_scripts/verify-recovery.sh

# follower recovery
CRASH_TARGET=s2 KEY_COUNT=8 ./build_scripts/verify-recovery.sh
```

Pass criteria:

- Script prints `Recovery verification passed: all committed keys survived crash-restart.`
- No key mismatch after restart
- Cluster can continue serving requests after resume

Persistent data proof points:

- server uses `PersistentStorage`
- compose mounts `s1-data/s2-data/s3-data` volumes
- restart does not erase committed values

---

## End-to-End Run Order (Recommended)

From `omnipaxos-kv/`:

1. Baseline no-fault check

```bash
docker compose -f build_scripts/docker-compose-nemesis.yml up -d --build
MAELSTROM_DIR="$HOME/maelstrom" SHIM_URL="http://127.0.0.1:3000" ./build_scripts/run-maelstrom-lin-kv.sh
docker compose -f build_scripts/docker-compose-nemesis.yml down
```

2. Nemesis campaigns

```bash
NEMESIS_MODE=partition PARTITION_MODE=majority-minority ./build_scripts/run-nemesis-test.sh
NEMESIS_MODE=crash CRASH_TARGET=leader ./build_scripts/run-nemesis-test.sh
NEMESIS_MODE=mixed PARTITION_MODE=random-halves CRASH_TARGET=random ./build_scripts/run-nemesis-test.sh
```

3. Bonus recovery checks

```bash
CRASH_TARGET=s1 KEY_COUNT=8 ./build_scripts/verify-recovery.sh
CRASH_TARGET=s2 KEY_COUNT=8 ./build_scripts/verify-recovery.sh
```

4. Record results

- Step 4 results -> `VERIFICATION_RESULTS.md`
- Bonus results -> `BONUS_RECOVERY_RESULTS.md`

---

## Persistent Storage (Bonus)

This project uses OmniPaxos `PersistentStorage` (RocksDB-backed):

- `Cargo.toml` enables `omnipaxos_storage` feature `persistent_storage`
- `server` uses `PersistentStorage<Command>` instead of `MemoryStorage`
- Docker compose mounts persistent volumes:
  - `s1-data`, `s2-data`, `s3-data`
- Storage path configured via:
  - `OMNIPAXOS_STORAGE_PATH=/data/omnipaxos`

---

## Script Parameters

### `run-maelstrom-lin-kv.sh`

- `MAELSTROM_DIR` (default: `$HOME/maelstrom`)
- `SHIM_URL` (default: `http://127.0.0.1:3000`)
- `TIME_LIMIT` (default: `20`)
- `NODE_COUNT` (default: `3`)
- `RATE` (default: `50`)
- `CONCURRENCY` (default: `6`)

### `run-nemesis-test.sh`

- `NEMESIS_MODE`: `partition | crash | mixed` (default: `mixed`)
- `PARTITION_MODE`: `random-halves | isolate-leader | majority-minority`
- `CRASH_TARGET`: `random | leader | s1 | s2 | s3`
- `NEMESIS_INTERVAL` (default: `8`)
- `FAULT_DURATION` (default: `6`)
- `TIME_LIMIT`, `RATE`, `CONCURRENCY` (forwarded to Maelstrom)
- `KEEP_STACK=1` keeps containers running after script exits

### `verify-recovery.sh`

- `CRASH_TARGET`: `s1 | s2 | s3 | random`
- `KEY_COUNT` (default: `8`)
- `RECOVERY_WAIT_SECS` (default: `8`)
- `READ_RETRIES` (default: `20`)
- `READ_RETRY_DELAY_SECS` (default: `1`)
- `READ_CURL_TIMEOUT_SECS` (default: `2`)

---

## Verification Results

See `VERIFICATION_RESULTS.md` for full logs and stats.

Summary from executed scenarios:

- Baseline: `valid? true`, linearizable
- Partition-only: `valid? true`, linearizable
- Crash-only: `valid? true`, linearizable
- Mixed: `valid? true`, linearizable

---

## Troubleshooting

### 1) `Unable to find libclang` while building

If building outside Docker with persistent storage:

```bash
sudo apt update
sudo apt install -y clang libclang-dev pkg-config cmake build-essential
```

### 2) Recovery script fails after leader crash

- Ensure latest scripts are used (`nemesis.sh`, `verify-recovery.sh`)
- Increase wait/retry knobs:

```bash
CRASH_TARGET=s1 RECOVERY_WAIT_SECS=12 READ_RETRIES=40 ./build_scripts/verify-recovery.sh
```

### 3) IDE says too many active git changes

Large generated files (`target`, `build_scripts/logs`, etc.) can degrade Git UI features.
Use `.gitignore` and clean/stash temporary artifacts.

---

## Repository Layout

- `src/server/` -> OmniPaxos KV server + network
- `src/shim/` -> HTTP shim
- `src/maelstrom_node/` -> Maelstrom adapter
- `build_scripts/` -> compose/nemesis/test runners
- `VERIFICATION_RESULTS.md` -> linearizability run summary
- `BONUS_RECOVERY_PLAN.md` -> recovery implementation plan
- `BONUS_RECOVERY_RESULTS.md` -> recovery experiment template
- `benchmarks/` -> original benchmark tooling
