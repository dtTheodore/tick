#!/usr/bin/env bash
# start-headless.sh — start every service in the background without a TTY.
# This is the entry point for AI agents and CI; no live-reload, just nohup
# + PID files + log files under tmp/.
#
#   ./scripts/start-headless.sh           # start everything
#   ./scripts/start-headless.sh --stop    # stop everything
#   ./scripts/start-headless.sh --logs    # tail logs (delegates to logs.sh)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

mode="start"
for arg in "$@"; do
  case "$arg" in
    --stop) mode="stop" ;;
    --logs) exec "$SCRIPT_DIR/logs.sh" "${@:2}" ;;
    -h|--help)
      sed -n '2,12p' "$0"
      exit 0
      ;;
  esac
done

# shellcheck source=worktree-env.sh
source "$SCRIPT_DIR/worktree-env.sh"

PID_DIR="$REPO_ROOT/tmp/pids"
LOG_DIR="$REPO_ROOT/tmp"
mkdir -p "$PID_DIR" "$LOG_DIR"

# --- stop ----------------------------------------------------------------
stop_all() {
  shopt -s nullglob
  for pidfile in "$PID_DIR"/*.pid; do
    local svc; svc="$(basename "$pidfile" .pid)"
    local pid; pid="$(cat "$pidfile" 2>/dev/null || true)"
    if [[ -n "${pid:-}" ]] && kill -0 "$pid" 2>/dev/null; then
      echo "[stop] $svc (pid $pid)"
      kill "$pid" 2>/dev/null || true
      # graceful: SIGTERM then SIGKILL after a short grace period
      for _ in 1 2 3 4 5; do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.5
      done
      kill -0 "$pid" 2>/dev/null && kill -9 "$pid" 2>/dev/null || true
    fi
    rm -f "$pidfile"
  done

  shopt -s nullglob
  for sentinel in "$PID_DIR"/*.compose; do
    local svc; svc="$(basename "$sentinel" .compose)"
    echo "[stop] $svc (docker compose down for matching service)"
    rm -f "$sentinel"
  done
  # One down for everything compose started, idempotent.
  if [[ -f docker-compose.yml ]]; then
    docker compose down >>"$LOG_DIR/postgres.log" 2>&1 || true
  fi

  echo "[stop] done"
}

if [[ "$mode" == "stop" ]]; then
  stop_all
  exit 0
fi

# --- start ---------------------------------------------------------------

"$SCRIPT_DIR/ensure-worktree-coherence.sh" --quiet
"$SCRIPT_DIR/sync-service-envs.sh" >/dev/null

start_proc() {
  local name="$1"; shift
  local logfile="$LOG_DIR/$name.log"
  local pidfile="$PID_DIR/$name.pid"

  if [[ -f "$pidfile" ]] && kill -0 "$(cat "$pidfile")" 2>/dev/null; then
    echo "[start] $name already running (pid $(cat "$pidfile"))"
    return 0
  fi

  : >"$logfile"
  nohup "$@" >>"$logfile" 2>&1 &
  echo $! >"$pidfile"
  echo "[start] $name (pid $(cat "$pidfile")) -> $logfile"
}

# Start a tap-backend service only once its crate exists. Plans C/D/E add the
# aggregator/worker/api incrementally; `cargo run -p <missing>` would otherwise
# fail the headless boot. The crate landing is enough to activate it — no edit
# here required.
start_proc_if_crate() {
  local name="$1" crate_dir="$2"; shift 2
  if [[ -f "games/tap-trading/backend/$crate_dir/Cargo.toml" ]]; then
    start_proc "$name" "$@"
  else
    echo "[skip] $name — crate games/tap-trading/backend/$crate_dir not present yet"
  fi
}

# Start a tap UI process only once the package exists.
start_proc_if_ui_present() {
  local name="$1" sub="$2"; shift 2
  if [[ -f "games/tap-trading/$sub/package.json" ]]; then
    start_proc "$name" "$@"
  else
    echo "[skip] $name — games/tap-trading/$sub not present yet"
  fi
}

# Bring a docker-compose service up in detached mode and wait until it's healthy.
# We track this with a sentinel file under tmp/pids/ so --stop knows to call
# `docker compose down` even though there's no PID we own.
start_compose() {
  local name="$1" service="$2"
  local sentinel="$PID_DIR/$name.compose"

  echo "[start] $name (docker compose up -d $service)"
  docker compose up -d "$service" >>"$LOG_DIR/$name.log" 2>&1
  : >"$sentinel"

  # Wait for the service's compose healthcheck to report healthy.
  local cid
  cid="$(docker compose ps -q "$service")"
  if [[ -z "$cid" ]]; then
    echo "[ready] $name FAILED — docker compose did not report a container id" >&2
    return 1
  fi
  for _ in $(seq 1 30); do
    local status
    status="$(docker inspect -f '{{.State.Health.Status}}' "$cid" 2>/dev/null || echo unknown)"
    if [[ "$status" == "healthy" ]]; then
      echo "[ready] $name (healthy)"
      return 0
    fi
    sleep 1
  done
  echo "[ready] $name FAILED — not healthy within 30s (see $LOG_DIR/$name.log)" >&2
  return 1
}

# Wait for an HTTP endpoint to return any 2xx/3xx/4xx response. Used as a
# liveness probe — reaching the listener at all is enough for now; route-level
# health is checked by integration tests, not here.
wait_http() {
  local name="$1" url="$2" timeout="${3:-30}"
  local start now
  start=$(date +%s)
  while true; do
    if curl -fsS -o /dev/null -w '%{http_code}' --max-time 2 "$url" 2>/dev/null \
        | grep -Eq '^[234]'; then
      echo "[ready] $name ($url)"
      return 0
    fi
    now=$(date +%s)
    if (( now - start > timeout )); then
      echo "[ready] $name FAILED to respond at $url within ${timeout}s — see tmp/$name.log" >&2
      return 1
    fi
    sleep 0.5
  done
}

# Start order: backends first (UI depends on them), then UI.
start_compose postgres postgres
start_compose redis redis

# tap-migrate — synchronous, foreground. Must complete before any service
# that reads Tick tables boots. Logs to tmp/tap-migrate.log; exit code is
# load-bearing — a failure here aborts the rest of start-headless.
echo "[start] tap-migrate (foreground)"
if ! cargo run -p tap-trading-migrate \
      --manifest-path games/tap-trading/backend/Cargo.toml \
      --quiet \
      -- run >>"$LOG_DIR/tap-migrate.log" 2>&1; then
    echo "[start] tap-migrate FAILED — see $LOG_DIR/tap-migrate.log" >&2
    exit 1
fi
echo "[ready] tap-migrate"

start_proc_if_crate tap-aggregator oracle-aggregator cargo run -p tap-trading-oracle-aggregator --manifest-path games/tap-trading/backend/Cargo.toml -q
start_proc_if_crate tap-worker     settlement-worker cargo run -p tap-trading-settlement-worker --manifest-path games/tap-trading/backend/Cargo.toml -q
start_proc_if_crate tap-api        api               cargo run -p tap-trading-api --manifest-path games/tap-trading/backend/Cargo.toml -q

if [[ -f games/tap-trading/backend/api/Cargo.toml ]]; then
  wait_http tap-api "$TAP_API_URL/healthz" 30 || true
fi
if [[ -f games/tap-trading/ui/package.json ]]; then
  wait_http tap-ui "$TAP_UI_URL" 60 || true
fi

start_proc_if_ui_present tap-ui ui bash -lc 'cd games/tap-trading/ui && bun install --silent && bun run dev'

cat <<EOF

[start] services up. tail logs with:
  ./scripts/logs.sh             # all services, color-coded
  ./scripts/logs.sh tap-api
EOF
