#!/usr/bin/env bash
# logs.sh — color-coded tail of headless service logs.
#
#   ./scripts/logs.sh                 # all services, follow
#   ./scripts/logs.sh tap-api         # only tap-api
#   ./scripts/logs.sh --no-follow     # dump current logs and exit

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

COLORS=(36 33 32 35 34 31)  # cyan, yellow, green, magenta, blue, red
follow=true
service_filter=""

for arg in "$@"; do
  case "$arg" in
    --no-follow) follow=false ;;
    --help|-h)
      sed -n '2,8p' "$0"
      exit 0
      ;;
    *) service_filter="$arg" ;;
  esac
done

tail_cmd=(tail)
$follow && tail_cmd=(tail -f)

shopt -s nullglob
color_idx=0
for pidfile in "$REPO_ROOT"/tmp/pids/*.pid; do
  svc="$(basename "$pidfile" .pid)"
  logfile="$REPO_ROOT/tmp/${svc}.log"
  if [[ -n "$service_filter" && "$svc" != "$service_filter" ]]; then continue; fi
  [[ -f "$logfile" ]] || continue

  color="${COLORS[$((color_idx % ${#COLORS[@]}))]}"
  color_idx=$((color_idx + 1))
  "${tail_cmd[@]}" "$logfile" \
    | sed "s/^/$(printf '\033[%sm' "$color")[${svc}]$(printf '\033[0m') /" &
done

if (( color_idx == 0 )); then
  echo "no service logs found in tmp/ — is anything running? (./scripts/start-headless.sh)" >&2
  exit 1
fi

wait
