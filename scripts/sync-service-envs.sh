#!/usr/bin/env bash
# sync-service-envs.sh — propagate canonical .local/.env values to the
# service-specific env files each framework reads (Vite expects .env.local
# in the project root, etc.). Atomic writes; only the vars each service
# actually needs get written, never a dump of everything.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# shellcheck source=worktree-env.sh
source "$SCRIPT_DIR/worktree-env.sh"

write_env() {
  local target="$1"
  local content="$2"
  mkdir -p "$(dirname "$target")"
  local tmp
  tmp="$(mktemp)"
  printf '%s' "$content" >"$tmp"
  mv "$tmp" "$target"
  echo "[sync] $target"
}

# tap-trading-api — port, DB, Redis, aggregator HTTP base, log level. Derives
# the WS URL internally from TAP_AGGREGATOR_URL.
write_env "$REPO_ROOT/games/tap-trading/backend/api/.env" "$(cat <<EOF
TAP_API_PORT=${TAP_API_PORT}
TAP_DB_URL=${TAP_DB_URL}
TAP_REDIS_URL=${TAP_REDIS_URL}
TAP_AGGREGATOR_URL=${TAP_AGGREGATOR_URL}
RUST_LOG=info,tower_http=debug
EOF
)"

# tap-trading-oracle-aggregator — its own port + log level.
write_env "$REPO_ROOT/games/tap-trading/backend/oracle-aggregator/.env" "$(cat <<EOF
TAP_AGGREGATOR_PORT=${TAP_AGGREGATOR_PORT}
RUST_LOG=info,tower_http=debug
EOF
)"

# tap-trading-settlement-worker — metrics port, DB URL, aggregator WS URL, log level.
write_env "$REPO_ROOT/games/tap-trading/backend/settlement-worker/.env" "$(cat <<EOF
TAP_WORKER_METRICS_PORT=${TAP_WORKER_METRICS_PORT}
TAP_DB_URL=${TAP_DB_URL}
TAP_AGGREGATOR_WS_URL=${TAP_AGGREGATOR_WS_URL}
RUST_LOG=info
EOF
)"

# tap-trading-ui — vite dev port, api base url, ws base url.
write_env "$REPO_ROOT/games/tap-trading/ui/.env.local" "$(cat <<EOF
TAP_UI_PORT=${TAP_UI_PORT}
VITE_TAP_API_URL=${TAP_API_URL}
VITE_TAP_API_WS_URL=$(echo "${TAP_API_URL}" | sed 's|^http|ws|')/stream
EOF
)"
