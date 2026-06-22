#!/usr/bin/env bash
# ensure-worktree-coherence.sh — hard-fail safety check.
#
# Validates that the canonical env file in .local/.env is internally
# consistent with the ports the worktree-env.sh script computes RIGHT NOW.
# If a developer copied .env from another worktree, or ports drifted, this
# is what catches it before any service binds the wrong port.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

quiet=false
for arg in "$@"; do
  case "$arg" in
    --quiet) quiet=true ;;
  esac
done

log()  { $quiet || echo "$@"; }
fail() { echo "[coherence] $*" >&2; exit 1; }

# shellcheck source=worktree-env.sh
source "$SCRIPT_DIR/worktree-env.sh"

canonical_env="$REPO_ROOT/.local/.env"
[[ -f "$canonical_env" ]] || fail "missing $canonical_env (run ./scripts/init-worktree-dev.sh)"

# helper — read a key from the canonical env file (no shell expansion of values)
read_env() {
  local key="$1"
  grep -E "^${key}=" "$canonical_env" | head -n1 | cut -d= -f2-
}

check_port() {
  local key="$1" expected="$2"
  local actual
  actual="$(read_env "$key")"
  [[ -n "$actual" ]] || fail "$key not set in $canonical_env"
  [[ "$actual" == "$expected" ]] || \
    fail "$key in $canonical_env is $actual but worktree-env.sh expects $expected — run ./scripts/init-worktree-dev.sh"
}

check_port PLATFORM_DB_PORT           "$PLATFORM_DB_PORT"
check_port PLATFORM_REDIS_PORT        "$PLATFORM_REDIS_PORT"
check_port TAP_UI_PORT                "$TAP_UI_PORT"
check_port TAP_API_PORT               "$TAP_API_PORT"
check_port TAP_AGGREGATOR_PORT        "$TAP_AGGREGATOR_PORT"
check_port TAP_WORKER_METRICS_PORT    "$TAP_WORKER_METRICS_PORT"

# URLs must reference the same ports — guard against hand-edits.
db_url="$(read_env PLATFORM_DB_URL)"
[[ "$db_url" == "$PLATFORM_DB_URL" ]] || \
  fail "PLATFORM_DB_URL is $db_url but worktree-env.sh expects $PLATFORM_DB_URL"

redis_url="$(read_env PLATFORM_REDIS_URL)"
[[ "$redis_url" == "$PLATFORM_REDIS_URL" ]] || \
  fail "PLATFORM_REDIS_URL is $redis_url but worktree-env.sh expects $PLATFORM_REDIS_URL"

tap_api_url="$(read_env TAP_API_URL)"
[[ "$tap_api_url" == "$TAP_API_URL" ]] || \
  fail "TAP_API_URL is $tap_api_url but worktree-env.sh expects $TAP_API_URL"

tap_agg_url="$(read_env TAP_AGGREGATOR_WS_URL)"
[[ "$tap_agg_url" == "$TAP_AGGREGATOR_WS_URL" ]] || \
  fail "TAP_AGGREGATOR_WS_URL is $tap_agg_url but worktree-env.sh expects $TAP_AGGREGATOR_WS_URL"

tap_db_url="$(read_env TAP_DB_URL)"
[[ "$tap_db_url" == "$TAP_DB_URL" ]] || \
  fail "TAP_DB_URL is $tap_db_url but worktree-env.sh expects $TAP_DB_URL"

# Compose project name must match — prevents a stray .env from another worktree
# pointing docker at the wrong containers.
compose_name="$(read_env COMPOSE_PROJECT_NAME)"
[[ "$compose_name" == "$COMPOSE_PROJECT_NAME" ]] || \
  fail "COMPOSE_PROJECT_NAME is $compose_name but worktree-env.sh expects $COMPOSE_PROJECT_NAME"

log "[coherence] ok (offset=$DOPAMINT_PORT_OFFSET)"
