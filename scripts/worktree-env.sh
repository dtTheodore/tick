#!/usr/bin/env bash
# worktree-env.sh — SOURCE this; do not execute.
#
# Derives a deterministic port offset from the absolute repo path so that
# multiple worktrees of this repo can run side-by-side without colliding.
# Same path => same ports, every time, no state file. Mod 200 keeps offsets
# in a safe band while making collisions vanishingly unlikely.

# resolve repo root from this script's location (works regardless of caller cwd).
# BASH_SOURCE[0] is empty under zsh; $0 IS the script path when sourced from zsh
# (and points to the parent shell from bash — covered by the BASH_SOURCE branch).
_src="${BASH_SOURCE[0]:-$0}"
REPO_ROOT="$(cd "$(dirname "$_src")/.." && pwd)"
unset _src
export REPO_ROOT

if [[ -n "${DOPAMINT_WORKTREE_ISOLATION_DISABLED:-}" ]]; then
  OFFSET=0
else
  _hash=$(printf '%s' "$REPO_ROOT" | shasum -a 256 | cut -c1-4)
  OFFSET=$((16#${_hash} % 200))
fi
export DOPAMINT_PORT_OFFSET="$OFFSET"

# Ports — every service that binds a port must read its value from here.
# PLATFORM_DB_PORT / PLATFORM_REDIS_PORT name the shared local infra that Tick's
# DB/cache point at; docker-compose.yml binds them by these names.
export PLATFORM_DB_PORT=$((5432 + OFFSET))
export PLATFORM_REDIS_PORT=$((6379 + OFFSET))
export TAP_UI_PORT=$((5210 + OFFSET))
export TAP_API_PORT=$((3200 + OFFSET))
export TAP_AGGREGATOR_PORT=$((3300 + OFFSET))
export TAP_WORKER_METRICS_PORT=$((3400 + OFFSET))

# Compose project name — keeps `docker compose` containers per-worktree.
# Sanitize: lowercase, only [a-z0-9-].
_basename=$(basename "$REPO_ROOT" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9-]/-/g')
export COMPOSE_PROJECT_NAME="dopamint-${_basename}"

# URLs derived from the ports above — single source of truth so downstream sync
# scripts don't reassemble them.
# Vite binds to IPv6 [::1] by default on macOS; use `localhost` so browsers
# resolve via IPv6. 127.0.0.1 (IPv4-only) would fail to connect.
export PLATFORM_DB_URL="postgres://dopamint:dopamint@127.0.0.1:${PLATFORM_DB_PORT}/dopamint"
export PLATFORM_REDIS_URL="redis://127.0.0.1:${PLATFORM_REDIS_PORT}"
export TAP_UI_URL="http://localhost:${TAP_UI_PORT}"
export TAP_API_URL="http://localhost:${TAP_API_PORT}"
export TAP_AGGREGATOR_URL="http://localhost:${TAP_AGGREGATOR_PORT}"
export TAP_AGGREGATOR_WS_URL="ws://localhost:${TAP_AGGREGATOR_PORT}/stream"
export TAP_DB_URL="${PLATFORM_DB_URL}"
export TAP_REDIS_URL="${PLATFORM_REDIS_URL}"

# Local env file — canonical per-worktree state.
export LOCAL_ENV_FILE="${REPO_ROOT}/.local/.env"
