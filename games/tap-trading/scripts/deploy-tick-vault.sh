#!/usr/bin/env bash
# Deploy the tick_vault Move package to the active Sui network and create one
# GameVault<Quote>, capturing the package id, shared vault id, and SettlerCap id
# into .local/tick-vault.env (sourced by the settlement worker + e2e harness)
# and tmp/tick-vault-deploy.json.
#
# Quote coin:
#   - Default: publishes the bundled tick_e2e_coin (6-decimal test stablecoin)
#     so the e2e needs no faucet, and instantiates GameVault<E2E_COIN>.
#   - Production: set TICK_QUOTE_TYPE=<pkg>::usdc::USDC (Circle native USDC) to
#     reuse an existing coin; the e2e-coin publish is then skipped. The vault
#     code is unchanged — that is the entire point of the <Quote> generic.
#
# Caps (override via env): generous on testnet, tuned for capital in Phase 5.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
MOVE_DIR="$REPO_ROOT/games/tap-trading/move"
ENV_OUT="$REPO_ROOT/.local/tick-vault.env"
MANIFEST="$REPO_ROOT/tmp/tick-vault-deploy.json"

PER_CELL_MAX_LIABILITY="${PER_CELL_MAX_LIABILITY:-1000000000000}" # 1M units
MAX_DIRECTIONAL_IMBALANCE_BPS="${MAX_DIRECTIONAL_IMBALANCE_BPS:-10000}" # 100%
TREASURY_MIN_BUFFER_BPS="${TREASURY_MIN_BUFFER_BPS:-1000}" # 10%
MAX_MULTIPLIER_BPS="${MAX_MULTIPLIER_BPS:-1000000}" # 100x
GAS_BUDGET="${GAS_BUDGET:-300000000}"

SETTLER_ADDRESS="${TICK_SETTLER_ADDRESS:-$(sui client active-address)}"
RPC_URL="$(sui client active-env >/dev/null 2>&1 && sui client envs --json 2>/dev/null | python3 -c 'import json,sys; d=json.load(sys.stdin); a=d[1] if isinstance(d,list) else d.get("active"); envs=d[0] if isinstance(d,list) else d.get("envs",[]); print(next((e["rpc"] for e in envs if e.get("alias")==a), "https://fullnode.testnet.sui.io:443"))' 2>/dev/null || echo https://fullnode.testnet.sui.io:443)"

obj_of() { # <publish-or-call-json> <substring-of-objectType>  -> first created objectId
  python3 - "$1" "$2" <<'PY'
import json,sys
d=json.loads(sys.argv[1]); needle=sys.argv[2]
for c in d.get("objectChanges",[]):
    if c.get("type")=="created" and needle in c.get("objectType",""):
        print(c["objectId"]); break
PY
}
pkg_of() { python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(next(c["packageId"] for c in d["objectChanges"] if c["type"]=="published"))' "$1"; }
status_of() { python3 -c 'import json,sys; print(json.loads(sys.argv[1])["effects"]["status"]["status"])' "$1"; }

echo "→ settler=$SETTLER_ADDRESS rpc=$RPC_URL"

QUOTE_TYPE="${TICK_QUOTE_TYPE:-}"
E2E_COIN_PKG=""
E2E_COIN_TREASURY=""
if [ -z "$QUOTE_TYPE" ]; then
  echo "→ publishing tick_e2e_coin (no TICK_QUOTE_TYPE set)"
  OUT=$(cd "$MOVE_DIR/tick_e2e_coin" && sui client publish --gas-budget 200000000 --json)
  [ "$(status_of "$OUT")" = success ] || { echo "e2e coin publish failed"; exit 1; }
  E2E_COIN_PKG=$(pkg_of "$OUT")
  E2E_COIN_TREASURY=$(obj_of "$OUT" "TreasuryCap")
  QUOTE_TYPE="${E2E_COIN_PKG}::e2e_coin::E2E_COIN"
  echo "  E2E_COIN_PKG=$E2E_COIN_PKG"
fi

echo "→ publishing tick_vault"
OUT=$(cd "$MOVE_DIR/tick_vault" && sui client publish --gas-budget "$GAS_BUDGET" --json)
[ "$(status_of "$OUT")" = success ] || { echo "vault publish failed"; exit 1; }
VAULT_PKG=$(pkg_of "$OUT")
echo "  TICK_VAULT_PKG=$VAULT_PKG"

echo "→ create_vault<$QUOTE_TYPE>"
OUT=$(sui client call --package "$VAULT_PKG" --module vault --function create_vault \
  --type-args "$QUOTE_TYPE" \
  --args "$SETTLER_ADDRESS" "$PER_CELL_MAX_LIABILITY" "$MAX_DIRECTIONAL_IMBALANCE_BPS" \
         "$TREASURY_MIN_BUFFER_BPS" "$MAX_MULTIPLIER_BPS" \
  --gas-budget 100000000 --json)
[ "$(status_of "$OUT")" = success ] || { echo "create_vault failed"; exit 1; }
VAULT_ID=$(obj_of "$OUT" "GameVault")
SETTLER_CAP=$(obj_of "$OUT" "SettlerCap")
ADMIN_CAP=$(obj_of "$OUT" "AdminCap")
echo "$OUT" > "$MANIFEST"

cat > "$ENV_OUT" <<EOF
# Tick on-chain vault deploy artifacts. Written by deploy-tick-vault.sh.
TICK_VAULT_PKG=$VAULT_PKG
TICK_VAULT_ID=$VAULT_ID
TICK_SETTLER_CAP_ID=$SETTLER_CAP
TICK_ADMIN_CAP_ID=$ADMIN_CAP
TICK_SETTLER_ADDRESS=$SETTLER_ADDRESS
TICK_QUOTE_TYPE=$QUOTE_TYPE
TICK_E2E_COIN_PKG=$E2E_COIN_PKG
TICK_E2E_COIN_TREASURY=$E2E_COIN_TREASURY
SUI_RPC_URL=$RPC_URL
WALRUS_STORE_EPOCHS=${WALRUS_STORE_EPOCHS:-5}
EOF

echo "✓ wrote $ENV_OUT and $MANIFEST"
echo "  TICK_VAULT_ID=$VAULT_ID"
echo "  TICK_SETTLER_CAP_ID=$SETTLER_CAP"
